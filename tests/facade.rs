//! The typed facade (ADR 8.5): each verb's function returns what its
//! command prints with `--json`, and exits as it does.
//!
//! A test binary of its own, without libtest's harness: the facade's
//! `session new` and `session resume` start the process from the running
//! program, `<program> host`, which `main` hands to `brnr::run`, as a
//! program that uses the facade does. The tests run one at a time, in one
//! environment set before any thread starts: the facade reads it, as the
//! commands it runs do. It takes libtest's `--list`, `--ignored`, `--exact`
//! and filters, for `cargo test` and the ADR inventory.

mod common;

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::ops::ControlFlow;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::process::{ExitCode, Output};
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use brnr::{Event, Exit, Target, config, elicitation, event, permission, process, prompt, queue};
use brnr::{session, skill};
use serde::Serialize;
use serde_json::{Value, json};

use common::{AGENT, Env, code_is, stderr};

type Test = (&'static str, fn(&Env));

const TESTS: &[Test] = &[
    ("adr_0008_the_facade_has_process_list_and_stop", process_verbs),
    ("adr_0008_the_facade_has_session_new_and_resume", session_start_verbs),
    ("adr_0008_the_facade_has_session_status_list_fork_close_and_delete", session_verbs),
    ("adr_0008_the_facade_has_prompt_send_cancel_and_commands", prompt_verbs),
    ("adr_0008_the_facade_has_queue_list_show_drop_and_clear", queue_verbs),
    ("adr_0008_the_facade_has_permission_requests_show_allow_and_reject", permission_verbs),
    ("adr_0008_the_facade_has_the_elicitation_verbs", elicitation_verbs),
    ("adr_0008_the_facade_has_config_get_and_set", config_verbs),
    ("adr_0008_the_facade_has_event_log_watch_and_wait", event_verbs),
    ("adr_0008_the_facade_has_doctor_and_skill", tool_verbs),
    ("adr_0008_a_facade_error_has_the_commands_code", errors),
    // Last: it deletes the transcripts the others left.
    ("adr_0008_the_facade_has_session_prune", prune_verbs),
];

/// The number the next session the test agent opens comes after: each
/// process's own, so that no two processes' sessions share an id.
static FIRST: AtomicU32 = AtomicU32::new(100);

fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    // A process the facade started: this program, as `<program> host`.
    if args.first().is_some_and(|a| a == "host") {
        return brnr::run(args);
    }
    let args: Vec<String> = args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
    let has = |flag: &str| args.iter().any(|a| a == flag);
    let mut filters = Vec::new();
    let mut words = args.iter();
    while let Some(word) = words.next() {
        match word.as_str() {
            "--test-threads" | "--format" | "--color" | "--logfile" | "--skip" | "-Z" => {
                words.next();
            }
            w if w.starts_with('-') => {}
            w => filters.push(w),
        }
    }
    let chosen = |name: &str| {
        filters.is_empty()
            || filters.iter().any(|f| if has("--exact") { name == *f } else { name.contains(*f) })
    };
    let tests: Vec<&Test> =
        if has("--ignored") { Vec::new() } else { TESTS.iter().filter(|t| chosen(t.0)).collect() };
    if has("--list") {
        tests.iter().for_each(|(name, _)| println!("{name}: test"));
        return ExitCode::SUCCESS;
    }
    let env = Env::new("facade");
    let vars = [
        ("BRNR_DIR", env.dir.join("run")),
        ("BRNR_HOME", env.dir.join("home")),
        ("BRNR_CONFIG", env.config()),
        ("PROMPT_LOG", env.dir.join("prompts")),
        ("CALL_LOG", env.dir.join("calls")),
        ("CHILD_PID", env.dir.join("child.pid")),
    ];
    for (key, value) in vars {
        // SAFETY: no other thread runs yet, so none reads the environment.
        unsafe { std::env::set_var(key, value) };
    }
    // SAFETY: as above. The processes the facade starts number their
    // sessions from here (the test agent reads it).
    unsafe { std::env::set_var("FIRST_SESSION", "900") };
    std::env::set_current_dir(&env.dir).unwrap();
    println!("\nrunning {} tests", tests.len());
    let mut failed = Vec::new();
    for (name, test) in &tests {
        let ok = catch_unwind(AssertUnwindSafe(|| test(&env))).is_ok();
        println!("test {name} ... {}", if ok { "ok" } else { "FAILED" });
        if !ok {
            failed.push(*name);
        }
    }
    let result = if failed.is_empty() { "ok" } else { "FAILED" };
    println!(
        "\ntest result: {result}. {} passed; {} failed",
        tests.len() - failed.len(),
        failed.len()
    );
    if failed.is_empty() { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}

// ---- comparing -----------------------------------------------------------

/// Fields whose values change between two runs of a command: time passing,
/// and the ids of what each run made. Their types are compared.
const CHANGING: &[&str] =
    &["uptime", "turn_elapsed", "timeout_left", "ts", "last_active", "before"];

/// Asserts that `lib`, what a facade function returned, is `cli`, what its
/// command printed with `--json`: the same fields at every level, with the
/// same values, but those in [`CHANGING`] and `vary`, whose types are the
/// same.
fn same<T: Serialize>(lib: &T, cli: &Value, vary: &[&str]) {
    let lib = serde_json::to_value(lib).unwrap();
    let mut problems = Vec::new();
    compare(&lib, cli, "", vary, &mut problems);
    assert!(problems.is_empty(), "{problems:#?}\nlib: {lib:#}\ncli: {cli:#}");
}

fn compare(lib: &Value, cli: &Value, at: &str, vary: &[&str], problems: &mut Vec<String>) {
    let key = at.rsplit('.').next().unwrap_or(at).trim_end_matches("[]");
    if CHANGING.contains(&key) || vary.contains(&key) {
        if kind(lib) != kind(cli) && !lib.is_null() && !cli.is_null() {
            problems.push(format!("{at}: {lib} and {cli} are of other types"));
        }
        return;
    }
    match (lib, cli) {
        (Value::Object(l), Value::Object(c)) => {
            let keys: Vec<&String> =
                l.keys().chain(c.keys().filter(|k| !l.contains_key(*k))).collect();
            for k in keys {
                match (l.get(k), c.get(k)) {
                    (Some(l), Some(c)) => compare(l, c, &format!("{at}.{k}"), vary, problems),
                    (l, _) => problems.push(format!(
                        "{at}.{k}: only in {}",
                        if l.is_some() { "lib" } else { "cli" }
                    )),
                }
            }
        }
        (Value::Array(l), Value::Array(c)) if l.len() == c.len() => {
            for (l, c) in l.iter().zip(c) {
                compare(l, c, &format!("{at}[]"), vary, problems);
            }
        }
        (l, c) if l == c => {}
        (l, c) => problems.push(format!("{at}: {l} != {c}")),
    }
}

fn kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "list",
        Value::Object(_) => "object",
    }
}

/// `brnr <args> --json` (before any `--`): its exit status and what it
/// printed.
fn cli(env: &Env, args: &[&str]) -> (i32, Value) {
    let out = env.run(&with_json(args));
    (out.status.code().unwrap_or(-1), printed(&out))
}

fn with_json<'a>(args: &[&'a str]) -> Vec<&'a str> {
    let at = args.iter().position(|a| *a == "--").unwrap_or(args.len());
    let mut args = args.to_vec();
    args.insert(at, "--json");
    args
}

fn printed(out: &Output) -> Value {
    let text = String::from_utf8_lossy(&out.stdout);
    match text.trim() {
        "" => Value::Null,
        text => {
            serde_json::from_str(text).unwrap_or_else(|e| panic!("{e}: {text}\n{}", stderr(out)))
        }
    }
}

/// Asserts that a read-only facade function returns what its command
/// prints, read between two runs of the command that print the same, so
/// that nothing changed meanwhile.
fn reads<T: Serialize>(env: &Env, args: &[&str], lib: impl Fn() -> T, vary: &[&str]) {
    for _ in 0..20 {
        let (status, before) = cli(env, args);
        assert_eq!(status, 0, "{args:?}: {before}");
        let got = lib();
        let (_, after) = cli(env, args);
        if strip(&before) == strip(&after) {
            same(&got, &before, vary);
            return;
        }
        thread::sleep(Duration::from_millis(100));
    }
    panic!("{args:?} printed something else each time");
}

/// `value` without the fields that change as time passes.
fn strip(value: &Value) -> Value {
    match value {
        Value::Object(o) => Value::Object(
            o.iter()
                .filter(|(k, _)| !CHANGING.contains(&k.as_str()))
                .map(|(k, v)| (k.clone(), strip(v)))
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.iter().map(strip).collect()),
        v => v.clone(),
    }
}

/// Asserts that a facade function that changes something returned what its
/// command, run on something like it, printed: and exited as it did.
fn exits<T: Serialize>(lib: &Exit<T>, cli: (i32, Value), vary: &[&str]) {
    assert_eq!(lib.status() as i32, cli.0, "{}", cli.1);
    match lib.value() {
        Some(value) => same(value, &cli.1, vary),
        None => assert_eq!(cli.1, Value::Null),
    }
}

// ---- sessions to run them on --------------------------------------------

/// A new process, started by the command line, with a session that has
/// sent `prompt` (and waited for its turn, with `wait`): the session.
fn start(env: &Env, prompt: &str, wait: bool) -> String {
    let first = FIRST.fetch_add(10, Relaxed).to_string();
    let mut args = vec!["session", "new", "--json", "--prompt", prompt];
    if wait {
        args.push("--wait");
    }
    args.extend(["--", AGENT]);
    let out = env.brnr(&args).env("FIRST_SESSION", &first).output().unwrap();
    assert!(out.status.success(), "session new failed: {}", stderr(&out));
    printed(&out)["session"].as_str().unwrap().to_owned()
}

/// Two sessions like each other, one for the command and one for the
/// facade: each its own process's, which sent `prompt`, then waited for
/// what `wait` names, if anything.
fn pair(env: &Env, prompt: &str, wait: Option<&str>) -> (String, String) {
    let both = [start(env, prompt, wait == Some("turn")), start(env, prompt, wait == Some("turn"))];
    for s in &both {
        if let Some(what @ ("permission" | "elicitation" | "idle")) = wait {
            env.ok(&["event", "wait", s, "--for", what, "--timeout", "10"]);
        }
    }
    let [a, b] = both;
    (a, b)
}

/// The pid of `session`'s process.
fn pid_of(env: &Env, session: &str) -> u32 {
    cli(env, &["session", "status", session]).1["pid"].as_u64().unwrap() as u32
}

fn agent() -> Option<Vec<String>> {
    Some(vec![AGENT.to_owned()])
}

// ---- the tests -----------------------------------------------------------

fn process_verbs(env: &Env) {
    let (a, b) = pair(env, "reply hi", Some("turn"));
    reads(env, &["process", "list"], || process::list().unwrap(), &[]);
    let (pa, pb) = (pid_of(env, &a), pid_of(env, &b));
    let lib = process::stop(pb).unwrap();
    let cli = cli(env, &["process", "stop", &pa.to_string()]);
    same(&lib, &cli.1, &["pid"]);
    assert_eq!(lib.pid, pb);
    code_is(&serde_json::to_value(process::stop(1).unwrap_err()).unwrap(), "process_not_found");
}

fn session_start_verbs(env: &Env) {
    // A process the facade starts runs this program as its host.
    let mut flags = session::StartFlags::default();
    (flags.prompt, flags.wait, flags.agent) = (Some("reply hi".into()), true, agent());
    let lib = session::new(&flags).unwrap();
    let first = FIRST.fetch_add(10, Relaxed).to_string();
    let args = with_json(&["session", "new", "--prompt", "reply hi", "--wait", "--", AGENT]);
    let out = env.brnr(&args).env("FIRST_SESSION", &first).output().unwrap();
    exits(&lib, (out.status.code().unwrap(), printed(&out)), &["session", "pid"]);
    let Some(session::Started::Turn(turn)) = lib.value() else { panic!("{lib:?}") };
    assert_eq!((turn.reply.as_str(), turn.stop_reason.as_deref()), ("hi", Some("end_turn")));
    let (lib_session, cli_session) =
        (turn.session.clone(), printed(&out)["session"].as_str().unwrap().to_owned());

    // With --pid, in the facade's process and the command's.
    let mut flags = session::StartFlags::default();
    flags.pid = Some(pid_of(env, &lib_session));
    let lib = session::new(&flags).unwrap();
    let cli_pid = pid_of(env, &cli_session).to_string();
    exits(&lib, cli(env, &["session", "new", "--pid", &cli_pid]), &["session", "pid"]);
    assert!(matches!(lib, Exit::Success(session::Started::Opened(_))), "{lib:?}");

    // Closed, each is resumed, the facade's in a process it starts.
    session::close(&lib_session).unwrap();
    env.ok(&["session", "close", &cli_session]);
    let mut flags = session::StartFlags::default();
    flags.prompt = Some("reply again".into());
    let lib = session::resume(&lib_session, &flags).unwrap();
    let cli = cli(env, &["session", "resume", &cli_session, "--prompt", "reply again"]);
    exits(&lib, cli, &["session", "pid", "message"]);
    let Some(session::Started::Opened(opened)) = lib.value() else { panic!("{lib:?}") };
    assert_eq!(opened.session, lib_session);

    // A usage error is the facade's own code (ADR 8.5): the command exits 2.
    let mut flags = session::StartFlags::default();
    (flags.timeout, flags.agent) = (Some(5), agent());
    let err = session::new(&flags).unwrap_err();
    code_is(&serde_json::to_value(&err).unwrap(), "usage_invalid");
    assert_eq!(env.run(&["session", "new", "--timeout", "5", "--", AGENT]).status.code(), Some(2));
}

fn session_verbs(env: &Env) {
    let (a, b) = pair(env, "reply hi", Some("turn"));
    reads(env, &["session", "status", &a], || session::status(&a).unwrap(), &[]);
    reads(env, &["session", "list"], || session::list(&Default::default()).unwrap(), &[]);
    let mut flags = session::ListFlags::default();
    (flags.include, flags.agent) = (vec!["inactive".into()], agent());
    let args = ["session", "list", "--include", "inactive", "--", AGENT];
    reads(env, &args, || session::list(&flags).unwrap(), &[]);

    let lib = session::fork(&b).unwrap();
    let cli = cli(env, &["session", "fork", &a]);
    same(&lib, &cli.1, &["session", "forked_from"]);
    assert_eq!(lib.forked_from, b);

    let lib = session::close(&b).unwrap();
    same(&lib, &cli_close(env, &a), &["session"]);
    let lib = session::delete(&b, &Default::default()).unwrap();
    let cli = self::cli(env, &["session", "delete", &a]);
    exits(&lib, cli, &["session", "recorded_in"]);
    assert!(matches!(&lib, Exit::Success(d) if d.deleted && d.session == b), "{lib:?}");
}

fn cli_close(env: &Env, session: &str) -> Value {
    cli(env, &["session", "close", session]).1
}

fn prompt_verbs(env: &Env) {
    let (a, b) = pair(env, "reply hi", Some("turn"));
    reads(env, &["prompt", "commands", &a], || prompt::commands(&a).unwrap(), &[]);

    let mut flags = prompt::SendFlags::default();
    flags.prompt = Some("reply done".into());
    let lib = prompt::send(&b, &flags).unwrap();
    exits(
        &lib,
        cli(env, &["prompt", "send", &a, "--prompt", "reply done"]),
        &["session", "message"],
    );
    env.ok(&["event", "wait", &a, "--timeout", "10"]);
    env.ok(&["event", "wait", &b, "--timeout", "10"]);
    flags.wait = true;
    let lib = prompt::send(&b, &flags).unwrap();
    let cli = cli(env, &["prompt", "send", &a, "--prompt", "reply done", "--wait"]);
    exits(&lib, cli, &["session", "message"]);
    // A prompt that is `-` is sent as it is, not read from stdin.
    flags.prompt = Some("-".into());
    let Exit::Success(prompt::Sent::Turn(turn)) = prompt::send(&b, &flags).unwrap() else {
        panic!()
    };
    assert_eq!(turn.stop_reason.as_deref(), Some("end_turn"));
    assert!(env.prompts().iter().any(|p| p == "-"), "{:?}", env.prompts());

    // Context, which has no message id.
    flags = prompt::SendFlags::default();
    (flags.prompt, flags.mode) =
        (Some("note".into()), Some(prompt::Mode::Context { replace: false }));
    let lib = prompt::send(&b, &flags).unwrap();
    exits(
        &lib,
        self::cli(env, &["prompt", "send", &a, "--prompt", "note", "--context"]),
        &["session"],
    );

    // A turn that hangs, a message held behind it: cancelled, it is dropped.
    for s in [&a, &b] {
        env.ok(&["prompt", "send", s, "--prompt", "hang"]);
        env.ok(&["prompt", "send", s, "--prompt", "later"]);
    }
    let lib = prompt::cancel(&b, &Default::default()).unwrap();
    same(&lib, &self::cli(env, &["prompt", "cancel", &a]).1, &["session", "message"]);
    assert_eq!(lib.dropped.len(), 1, "{lib:?}");
}

fn queue_verbs(env: &Env) {
    fs::create_dir_all(env.dir.join("src")).unwrap();
    fs::write(env.dir.join("src/api.rs"), "pub fn api() {}\n").unwrap();
    let (a, b) = pair(env, "hang", None);
    for s in [&a, &b] {
        for text in ["one", "two", "three"] {
            env.ok(&["prompt", "send", s, "--prompt", text, "--file", "src/api.rs"]);
        }
        env.ok(&["prompt", "send", s, "--prompt", "note", "--context"]);
    }
    reads(env, &["queue", "list", &a], || queue::list(&a).unwrap(), &[]);
    let held = queue::list(&b).unwrap().held;
    let cli_held = cli(env, &["queue", "list", &a]).1["held"].clone();
    let (lib_first, cli_first) =
        (held[0].message.clone(), cli_held[0]["message"].as_str().unwrap().to_owned());
    reads(env, &["queue", "show", &a, &cli_first], || queue::show(&a, &cli_first).unwrap(), &[]);

    let lib = queue::drop(&b, &lib_first).unwrap();
    same(&lib, &cli(env, &["queue", "drop", &a, &cli_first]).1, &["session", "message"]);
    let mut flags = queue::ClearFlags::default();
    flags.context = true;
    let lib = queue::clear(&b, &flags).unwrap();
    same(&lib, &cli(env, &["queue", "clear", &a, "--context"]).1, &["session", "message"]);
    let lib = queue::clear(&b, &Default::default()).unwrap();
    same(&lib, &cli(env, &["queue", "clear", &a]).1, &["session", "message"]);
    assert_eq!(lib.dropped.len(), 2, "{lib:?}");
}

fn permission_verbs(env: &Env) {
    let (a, b) = pair(env, "perm edit", Some("permission"));
    reads(env, &["permission", "requests"], || permission::requests(None).unwrap(), &[]);
    reads(env, &["permission", "requests", &a], || permission::requests(Some(&a)).unwrap(), &[]);
    reads(env, &["permission", "show", &a, "p1"], || permission::show(&a, "p1").unwrap(), &[]);
    let lib = permission::allow(&b, "p1", &Default::default()).unwrap();
    same(&lib, &cli(env, &["permission", "allow", &a, "p1"]).1, &["session"]);

    let (a, b) = pair(env, "perm edit", Some("permission"));
    let mut flags = permission::AnswerFlags::default();
    flags.option_id = Some("reject".into());
    let lib = permission::reject(&b, "p1", &flags).unwrap();
    same(
        &lib,
        &cli(env, &["permission", "reject", &a, "p1", "--option-id", "reject"]).1,
        &["session"],
    );
    code_is(
        &serde_json::to_value(permission::show(&b, "p1").unwrap_err()).unwrap(),
        "request_not_found",
    );
}

fn elicitation_verbs(env: &Env) {
    let (a, b) = pair(env, "elicit form", Some("elicitation"));
    reads(env, &["elicitation", "requests"], || elicitation::requests(None).unwrap(), &[]);
    reads(env, &["elicitation", "requests", &a], || elicitation::requests(Some(&a)).unwrap(), &[]);
    let show = || elicitation::show(&Target::Session(a.clone()), "e1").unwrap();
    reads(env, &["elicitation", "show", &a, "e1"], show, &[]);
    let pid = pid_of(env, &a);
    assert!(elicitation::show(&Target::Pid(pid), "e1").is_err(), "e1 is in a session");

    let mut flags = elicitation::AcceptFlags::default();
    flags.field = vec![("name".into(), "Ada".into()), ("color".into(), "green".into())];
    let lib = elicitation::accept(&Target::Session(b.clone()), "e1", &flags).unwrap();
    let args = ["elicitation", "accept", &a, "e1", "--field", "name=Ada", "--field", "color=green"];
    same(&lib, &cli(env, &args).1, &["session"]);

    // The fields as JSON, as `-` reads them on stdin.
    let (a, b) = pair(env, "elicit form", Some("elicitation"));
    let fields = json!({ "name": "Ada", "color": "green" });
    let mut flags = elicitation::AcceptFlags::default();
    flags.fields = fields.as_object().cloned();
    let lib = elicitation::accept(&Target::Session(b.clone()), "e1", &flags).unwrap();
    let out = env.run_with_stdin(
        &["elicitation", "accept", &a, "e1", "-", "--json"],
        fields.to_string().as_bytes(),
    );
    same(&lib, &printed(&out), &["session"]);

    let (a, b) = pair(env, "elicit form", Some("elicitation"));
    let lib = elicitation::decline(&Target::Session(b.clone()), "e1").unwrap();
    same(&lib, &cli(env, &["elicitation", "decline", &a, "e1"]).1, &["session"]);
    let (a, b) = pair(env, "elicit form", Some("elicitation"));
    let lib = elicitation::cancel(&Target::Session(b.clone()), "e1").unwrap();
    same(&lib, &cli(env, &["elicitation", "cancel", &a, "e1"]).1, &["session"]);
}

fn config_verbs(env: &Env) {
    let (a, b) = pair(env, "reply hi", Some("turn"));
    reads(env, &["config", "get", &a], || config::get(&a, &Default::default()).unwrap(), &[]);
    let mut flags = config::GetFlags::default();
    flags.model = true;
    reads(env, &["config", "get", &a, "--model"], || config::get(&a, &flags).unwrap(), &[]);
    let mut flags = config::SetFlags::default();
    flags.model = Some("large".into());
    let lib = config::set(&b, &flags).unwrap();
    same(&lib, &cli(env, &["config", "set", &a, "--model", "large"]).1, &["session"]);
}

fn event_verbs(env: &Env) {
    let (a, b) = pair(env, "reply hi", Some("turn"));
    // event log: each event as the command prints it, one per line.
    let log = |flags: &event::LogFlags| {
        let mut events = Vec::new();
        event::log(&a, flags, |e| {
            events.push(e);
            ControlFlow::Continue(())
        })
        .unwrap();
        events
    };
    let lines = |out: Output| -> Vec<Value> {
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    };
    let cli_log = lines(env.run(&["event", "log", &a, "--json"]));
    let lib_log = log(&Default::default());
    same(&lib_log, &Value::Array(cli_log.clone()), &[]);
    assert!(lib_log.iter().any(|e| e.event == "turn_ended"), "{lib_log:?}");
    let mut flags = event::LogFlags::default();
    (flags.last, flags.events) = (Some(1), vec!["all".into()]);
    let cli_all = lines(env.run(&["event", "log", &a, "--last", "1", "--events", "all", "--json"]));
    same(&log(&flags), &Value::Array(cli_all), &[]);

    // event log, broken off after its first event.
    let mut first = Vec::new();
    event::log(&a, &Default::default(), |e| {
        first.push(e);
        ControlFlow::Break(())
    })
    .unwrap();
    same(&first, &Value::Array(cli_log[..1].to_vec()), &[]);

    // event wait --for idle and turn.
    let mut flags = event::WaitFlags::default();
    let lib = event::wait(&a, &flags).unwrap();
    exits(&lib, cli(env, &["event", "wait", &a]), &[]);
    assert!(matches!(&lib, Exit::Success(event::Waited::Idle(i)) if i.state == "idle"), "{lib:?}");
    flags.r#for = event::For::Turn;
    let mut timed = flags.clone();
    timed.timeout = Some(1);
    exits(
        &event::wait(&a, &timed).unwrap(),
        cli(env, &["event", "wait", &a, "--for", "turn", "--timeout", "1"]),
        &[],
    );
    assert!(matches!(event::wait(&a, &timed).unwrap(), Exit::TimedOut));

    // event watch, live, until the session closes: the events the log has.
    let (tx, rx) = mpsc::channel();
    let target = Target::Session(b.clone());
    let before = subscriptions(env);
    let watching = thread::spawn(move || {
        let mut events = Vec::new();
        let exit = event::watch(&target, &Default::default(), |e| {
            let _ = tx.send(e.event.clone());
            events.push(e);
            ControlFlow::Continue(())
        });
        (exit, events)
    });
    env.wait_subscriptions(before + 1);
    let turn = thread::spawn({
        let b = b.clone();
        move || {
            let mut flags = event::WaitFlags::default();
            flags.r#for = event::For::Turn;
            event::wait(&b, &flags)
        }
    });
    env.wait_subscriptions(before + 2);
    env.ok(&["prompt", "send", &b, "--prompt", "reply watched"]);
    let ended = turn.join().unwrap().unwrap();
    let cli_turn = cli(env, &["event", "log", &b, "--last", "1", "--events", "turn_ended"]);
    let Exit::Success(event::Waited::Turn(ended)) = ended else { panic!("{ended:?}") };
    same(&ended, &cli_turn.1, &[]);
    assert!(rx.recv_timeout(Duration::from_secs(10)).is_ok(), "nothing watched");
    env.ok(&["session", "close", &b]);
    let (exit, watched) = watching.join().unwrap();
    assert_eq!(exit.unwrap(), Exit::Success(()));
    let logged: HashMap<u64, Value> =
        lines(env.run(&["event", "log", &b, "--events", "all", "--json"]))
            .into_iter()
            .filter_map(|e| Some((e["seq"].as_u64()?, e)))
            .collect();
    assert!(watched.iter().any(|e| e.event == "session_closed"), "{watched:?}");
    for e in &watched {
        let seq = e.get("seq").and_then(Value::as_u64).unwrap();
        same(e, &logged[&seq], &[]);
    }

    // event watch, broken off at its only event, returns then, its command
    // ended and its connection to the process closed, though no event
    // comes after (ADR 10.4).
    let target = Target::Pid(pid_of(env, &a));
    let mut flags = event::WatchFlags::default();
    flags.events = vec!["turn_ended".into()];
    let before = subscriptions(env);
    let fds = open_fds();
    thread::scope(|scope| {
        scope.spawn(|| {
            env.wait_subscriptions(before + 1);
            env.ok(&["prompt", "send", &a, "--prompt", "reply hi"]);
        });
        let mut got = Vec::new();
        let exit = event::watch(&target, &flags, |e| {
            got.push(e);
            ControlFlow::Break(())
        });
        assert_eq!((exit.unwrap(), got.len()), (Exit::Success(()), 1));
        assert_eq!(got[0].event, "turn_ended");
    });
    assert_eq!(open_fds(), fds, "the watch's connection is still open");

    // The same, the closure panicking at its only event: the panic goes on
    // to the caller, the command stopped first.
    let before = subscriptions(env);
    let fds = open_fds();
    thread::scope(|scope| {
        scope.spawn(|| {
            env.wait_subscriptions(before + 1);
            env.ok(&["prompt", "send", &a, "--prompt", "reply hi"]);
        });
        // Quiet: this panic is the test's.
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let panicked = catch_unwind(AssertUnwindSafe(|| {
            event::watch(&target, &flags, |_| panic!("the caller's closure panics"))
        }));
        std::panic::set_hook(hook);
        assert!(panicked.is_err());
    });
    assert_eq!(open_fds(), fds, "the watch's connection is still open after a panic");

    // event wait --for exit: the process's `process_exited`, once its last
    // session closes.
    let (a, b) = pair(env, "reply hi", Some("turn"));
    let before = subscriptions(env);
    let (lib, cli) = thread::scope(|scope| {
        let lib = scope.spawn(|| {
            let mut flags = event::WaitFlags::default();
            flags.r#for = event::For::Exit;
            event::wait(&b, &flags)
        });
        let cli = scope.spawn(|| cli(env, &["event", "wait", &a, "--for", "exit"]));
        env.wait_subscriptions(before + 2);
        env.ok(&["session", "close", &a]);
        env.ok(&["session", "close", &b]);
        (lib.join().unwrap().unwrap(), cli.join().unwrap())
    });
    exits(&lib, cli, &["pid", "host_id", "seq"]);
    assert!(matches!(&lib, Exit::Success(event::Waited::Exit(e)) if e.event == "process_exited"));
}

/// This process's open file descriptors.
fn open_fds() -> usize {
    fs::read_dir("/dev/fd").unwrap().count()
}

/// The subscriptions the processes have taken so far, each in their host
/// logs.
fn subscriptions(env: &Env) -> usize {
    env.logged();
    env.subscriptions()
}

fn tool_verbs(env: &Env) {
    reads(
        env,
        &["doctor"],
        || brnr::doctor(&Default::default()).unwrap().into_value().unwrap(),
        &["message"],
    );
    let text = |args: &[&str]| String::from_utf8(env.run(args).stdout).unwrap();
    assert_eq!(brnr::skill(None).unwrap(), text(&["skill"]));
    assert_eq!(brnr::skill(Some("approvals")).unwrap(), text(&["skill", "approvals"]));
    code_is(
        &serde_json::to_value(brnr::skill(Some("nope")).unwrap_err()).unwrap(),
        "usage_invalid",
    );
    let mut flags = skill::InstallFlags::default();
    flags.dir = vec![env.dir.join("lib-skills").to_string_lossy().into_owned()];
    skill::install(&flags).unwrap();
    let cli_dir = env.dir.join("cli-skills");
    env.ok(&["skill", "install", "--dir", &cli_dir.to_string_lossy()]);
    let files = |dir: &Path| -> Vec<(String, String)> {
        let mut files: Vec<_> = ["SKILL.md", "references/approvals.md", "references/setup.md"]
            .iter()
            .map(|f| (f.to_string(), fs::read_to_string(dir.join("brnr").join(f)).unwrap()))
            .collect();
        files.sort();
        files
    };
    assert_eq!(files(&env.dir.join("lib-skills")), files(&cli_dir));
}

fn errors(env: &Env) {
    let err = session::status("sess-gone").unwrap_err();
    let (status, cli) = cli(env, &["session", "status", "sess-gone"]);
    assert_eq!(status, 1);
    same(&err, &cli["error"], &[]);
    code_is(&serde_json::to_value(&err).unwrap(), "session_not_found");
    assert_eq!(err.code(), brnr::Code::session_not_found);
    // An event of no session in a watch of one that isn't there: the error,
    // before any event.
    let err =
        event::watch(&Target::Session("sess-gone".into()), &Default::default(), |_: Event| {
            ControlFlow::Continue(())
        })
        .unwrap_err();
    code_is(&serde_json::to_value(&err).unwrap(), "session_not_found");
}

fn prune_verbs(env: &Env) {
    let (a, b) = pair(env, "reply hi", Some("turn"));
    env.ok(&["session", "close", &a]);
    env.ok(&["session", "close", &b]);
    let mut flags = session::PruneFlags::default();
    flags.dry_run = true;
    let dry = || session::prune(0, &flags).unwrap().into_value().unwrap();
    reads(env, &["session", "prune", "--older-than", "0", "--dry-run"], dry, &[]);
    let would = dry();
    assert!(would.sessions.iter().any(|s| s.session == a), "{would:?}");
    // Nothing is as old as a day: nothing goes.
    let lib = session::prune(86_400, &Default::default()).unwrap();
    exits(&lib, cli(env, &["session", "prune", "--older-than", "86400"]), &[]);
    // Everything closed goes, and is gone.
    let lib = session::prune(0, &Default::default()).unwrap();
    assert!(
        matches!(&lib, Exit::Success(p) if !p.dry_run && p.sessions.iter().any(|s| s.session == b))
    );
    let (status, left) = cli(env, &["session", "prune", "--older-than", "0", "--dry-run"]);
    assert_eq!((status, &left["sessions"]), (0, &json!([])), "{left}");
}
