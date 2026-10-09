//! Headless sessions (`brnr start`) and editors' (`brnr acp`) against a fake
//! ACP agent (fake_agent.py). Each test gets its own runtime, state and config
//! directories, and kills whatever it leaves running.

mod common;

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::os::unix::net::UnixStream;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::thread::sleep;
use std::time::Duration;

use common::*;
use serde_json::{Value, json};

// ---- starting ----------------------------------------------------------

/// The process's `started` record: the first in its host log.
fn started_record(env: &Env) -> Value {
    let dir = env.dir.join("home/hosts");
    let wait = || fs::read_dir(&dir).is_ok_and(|mut d| d.next().is_some());
    assert!(wait_for(Duration::from_secs(5), wait), "no host log");
    let log = fs::read_dir(&dir).unwrap().next().unwrap().unwrap().path();
    let text = fs::read_to_string(log).unwrap();
    let record: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(record["event"]["event"], "started", "{record}");
    record["event"].clone()
}

/// A `brnr start` that goes away before the start commits stops the
/// process at once, whatever it is doing (here, waiting 30 s for the
/// session), and the agent never gets the prompt.
#[test]
fn adr_0007_abandoned_start_sends_no_prompt() {
    let env = Env::new("abandon").agent("NEW_DELAY", "30");
    let mut start = env
        .brnr(&start_args(&["--prompt", "run the migration"]))
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    assert!(wait_for(Duration::from_secs(5), || !env.hosts().is_empty()), "no host");
    start.kill().unwrap();
    start.wait().unwrap();

    assert!(wait_for(Duration::from_secs(15), || env.hosts().is_empty()), "host kept running");
    assert!(env.prompts().is_empty(), "agent got {:?}", env.prompts());
    let log = fs::read_dir(env.dir.join("home/hosts")).unwrap().next().unwrap().unwrap().path();
    assert!(fs::read_to_string(log).unwrap().contains("start-abandoned"));
}

/// A `brnr start` that goes once the process has decided to report ready,
/// but before the report is written, hasn't been told the session: the
/// write fails, the start is abandoned as before the commit, and the agent
/// never gets the prompt (ADR 7). (`BRNR_TEST_READY=hold` holds the commit
/// there until brnr start has gone.)
#[test]
fn adr_0007_start_gone_as_ready_is_written_sends_no_prompt() {
    let env = Env::new("abandon-ready");
    let mut start = env
        .brnr(&start_args(&["--prompt", "run the migration"]))
        .env("BRNR_TEST_READY", "hold")
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let held = || host_logs(&env).contains("test-ready-held");
    assert!(wait_for(Duration::from_secs(10), held), "never held: {}", host_logs(&env));
    start.kill().unwrap();
    start.wait().unwrap();

    assert!(wait_for(Duration::from_secs(15), || env.hosts().is_empty()), "host kept running");
    assert!(env.prompts().is_empty(), "agent got {:?}", env.prompts());
    assert!(host_logs(&env).contains("start-abandoned"), "{}", host_logs(&env));
}

/// A `brnr start --wait` that goes once it has the ready report leaves the
/// session running, its prompt sent: the start committed (ADR 7, P14).
#[test]
fn adr_0007_start_gone_after_ready_leaves_the_session_running() {
    let env = Env::new("after-ready");
    let mut start = env
        .brnr(&start_args(&["--wait", "--prompt", "hang on"]))
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut said = String::new();
    BufReader::new(start.stderr.take().unwrap()).read_line(&mut said).unwrap();
    assert!(said.starts_with("started sess-1"), "{said}");
    start.kill().unwrap();
    start.wait().unwrap();

    assert!(wait_for(Duration::from_secs(5), || !env.prompts().is_empty()), "no prompt");
    assert_eq!(env.prompts(), ["hang on"]);
    sleep(Duration::from_millis(500));
    assert_eq!(env.hosts().len(), 1, "host stopped");
    assert!(!host_logs(&env).contains("start-abandoned"), "{}", host_logs(&env));
    env.stop();
}

/// Everything the process does is in the one request it was started with,
/// which its `started` record holds; its argv is only `brnr host`.
#[test]
fn adr_0008_start_hands_over_one_request() {
    let env = Env::new("request");
    env.write_config(
        "[profiles.default]\nstrict = true\n\n[profiles.default.headless]\nmode = \"plan\"\n",
    );
    env.start(&["--set", "model=large", "--stop-when-idle", "60", "--prompt", "hello"]);
    let started = started_record(&env);
    let request = &started["request"];
    assert_eq!(request["agent"][0], AGENT, "{request}");
    assert_eq!(request["strict"], true);
    assert_eq!(request["log"], "all");
    let headless = &request["role"]["headless"];
    // The flags' settings and the profile's, apart (ADR 58).
    assert_eq!(headless["defaults"]["mode"], "plan", "{request}");
    assert_eq!(headless["settings"]["config"]["model"], "large");
    assert!(headless["settings"]["mode"].is_null() && headless["defaults"]["config"] == json!({}));
    assert_eq!(headless["stop_when_idle"], 60);
    assert_eq!(headless["start_timeout"], 120);
    assert_eq!(headless["prompt"]["text"], "hello");
    assert!(headless["events"].as_array().unwrap().is_empty(), "subscribed without --wait");
    let ps = Command::new("ps").args(["-o", "args=", "-p", &env.host_pid().to_string()]).output();
    let args = String::from_utf8_lossy(&ps.unwrap().stdout).trim().to_owned();
    assert!(args.ends_with("brnr host"), "ps: {args}");
}

/// `brnr host` isn't run by hand, and takes no flags.
#[test]
fn adr_0008_host_is_not_run_by_hand() {
    let env = Env::new("byhand");
    let err = env.fails(&["host", "--prompt", "hi", "--", AGENT]);
    assert!(err.contains("not by hand"), "{err}");
    let err = env.fails(&["host"]);
    assert!(err.contains("brnr start --foreground"), "{err}");
    assert!(env.hosts().is_empty());
}

/// The process reads its request to EOF before doing anything: one cut short
/// (its starter died writing it) or of the wrong shape is refused, and the
/// agent never started.
#[test]
fn adr_0008_a_bad_request_is_refused() {
    let env = Env::new("badreq");
    let host = |request: &[u8]| {
        let (ours, theirs) = UnixStream::pair().unwrap();
        let fd = theirs.as_raw_fd();
        let mut cmd = env.brnr(&["host"]);
        cmd.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped());
        // SAFETY: between fork and exec the closure only calls dup2(2), which is
        // async-signal-safe, and allocates nothing.
        unsafe {
            cmd.pre_exec(move || {
                (libc::dup2(fd, 3) >= 0).then_some(()).ok_or_else(std::io::Error::last_os_error)
            })
        };
        let mut child = cmd.spawn().unwrap();
        drop(theirs);
        child.stdin.take().unwrap().write_all(request).unwrap();
        let out = child.wait_with_output().unwrap();
        let mut report = String::new();
        BufReader::new(ours).read_line(&mut report).unwrap();
        (out.status.code(), stderr(&out), report)
    };
    let (code, err, report) = host(br#"{"profile":null,"agent":["#);
    assert_eq!(code, Some(2), "{err}");
    assert!(err.contains("invalid start request"), "{err}");
    assert_eq!(report, "", "nobody to tell");
    let (code, err, report) = host(br#"{"role":{"headless":{}}}"#);
    assert_eq!(code, Some(2), "{err}");
    let report: Value = serde_json::from_str(&report).unwrap();
    assert_eq!(report["ok"], false);
    assert!(report["error"].as_str().unwrap().contains("invalid start request"), "{report}");
    assert!(env.hosts().is_empty() && env.calls().is_empty());
}

/// When the session doesn't open in time, `brnr start` says so and the
/// host stops instead of carrying on unseen.
#[test]
fn adr_0007_start_timeout_stops_the_host() {
    let env = Env::new("timeout").agent("NEW_DELAY", "4");
    let out = env
        .brnr(&start_args(&["--prompt", "run the migration"]))
        .env("BRNR_START_TIMEOUT", "1")
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(stderr(&out).contains("timed out"), "{}", stderr(&out));

    assert!(wait_for(Duration::from_secs(15), || env.hosts().is_empty()), "host kept running");
    sleep(Duration::from_secs(4));
    assert!(env.prompts().is_empty(), "agent got {:?}", env.prompts());
}

#[test]
fn adr_0007_empty_prompt_is_refused() {
    let env = Env::new("empty");
    let out = env.run(&start_args(&["--prompt", " "]));
    assert!(!out.status.success());
    assert!(stderr(&out).contains("empty"), "{}", stderr(&out));
    assert!(env.hosts().is_empty());
}

/// The prompt reaches the agent without ever being on a command line, where
/// `ps` shows it and Linux caps a single argument at 128 KiB. Nor is the
/// agent's command: `pkill -f <adapter>` would take the process with it.
#[test]
fn adr_0008_prompt_is_not_on_the_command_line() {
    let env = Env::new("argv");
    env.start(&["--prompt", "deploy with sk-SECRET-123"]);
    let ps = Command::new("ps").args(["-o", "args=", "-p", &env.host_pid().to_string()]).output();
    let args = String::from_utf8_lossy(&ps.unwrap().stdout).into_owned();
    assert!(args.contains("brnr"), "ps: {args}");
    assert!(!args.contains("sk-SECRET"), "prompt visible in ps: {args}");
    assert!(!args.contains("fake_agent"), "agent visible in ps: {args}");
    assert!(wait_for(Duration::from_secs(5), || !env.prompts().is_empty()));
    assert_eq!(env.prompts(), ["deploy with sk-SECRET-123"]);
}

/// More than Linux takes in one argument, read from start's stdin before it
/// launches the process, and handed over in the request.
#[test]
fn adr_0007_large_prompt_from_stdin() {
    let env = Env::new("bigprompt");
    let prompt = "x".repeat(300_000);
    let out = env.run_with_stdin(&start_args(&["--prompt", "-"]), prompt.as_bytes());
    assert!(out.status.success(), "start failed: {}", stderr(&out));
    assert!(wait_for(Duration::from_secs(5), || !env.prompts().is_empty()));
    assert_eq!(env.prompts()[0].len(), prompt.len());
    let started = started_record(&env);
    assert_eq!(
        started["request"]["role"]["headless"]["prompt"]["text"].as_str().unwrap().len(),
        300_000
    );
}

// ---- stopping ----------------------------------------------------------

/// An agent that stops reading its stdin must not wedge the host: it still
/// answers, and `brnr stop` still ends it.
#[test]
fn stalled_agent_can_still_be_stopped() {
    let env = Env::new("stall").agent("STALL", "1");
    env.start(&[]);
    let host = env.host_pid();
    let big = vec![b'x'; 1 << 20];
    let out = env.run_with_stdin(&["send", "sess-1", "-"], &big);
    assert!(out.status.success(), "send: {}", stderr(&out));

    let out = env.run(&["status", "sess-1"]);
    assert!(out.status.success(), "status: {}", stderr(&out));
    let out = env.run(&["stop", &host.to_string()]);
    assert!(out.status.success(), "stop: {}", stderr(&out));
    assert!(wait_for(Duration::from_secs(15), || !alive(host)), "host still running");
}

/// `brnr stop` escalates to the agent's whole process group.
#[test]
fn stop_kills_the_agents_children() {
    let env = Env::new("stubborn").agent("STUBBORN", "all");
    env.start(&[]);
    let (host, child) = (env.host_pid(), env.child_pid());
    assert!(env.run(&["stop", &env.pid()]).status.success());
    assert!(wait_for(Duration::from_secs(15), || !alive(host)), "host still running");
    assert!(wait_for(Duration::from_secs(2), || !alive(child)), "agent's child survived");
}

/// A child that outlives an agent which stopped cleanly is killed too.
#[test]
fn stop_kills_children_left_behind() {
    let env = Env::new("orphan").agent("STUBBORN", "child");
    env.start(&[]);
    let (host, child) = (env.host_pid(), env.child_pid());
    assert!(env.run(&["stop", &env.pid()]).status.success());
    assert!(wait_for(Duration::from_secs(15), || !alive(host)), "host still running");
    assert!(wait_for(Duration::from_secs(2), || !alive(child)), "agent's child survived");
}

/// An agent that moved out of its own process group is still stopped:
/// signalling the group alone would miss it.
#[test]
fn stop_reaches_an_agent_out_of_its_group() {
    let env = Env::new("leave").agent("LEAVE_GROUP", "1").agent("STUBBORN", "all");
    env.start(&[]);
    let host = env.host_pid();
    assert!(env.run(&["stop", &env.pid()]).status.success());
    assert!(wait_for(Duration::from_secs(20), || !alive(host)), "host still running");
}

/// A start that fails after the session opened (setting its mode) fails in
/// the foreground too: it says why, and doesn't exit 0.
#[test]
fn adr_0009_foreground_start_failure_is_reported() {
    let env = Env::new("fg-mode");
    let out = env.run(&start_args(&["--foreground", "--mode", "bogus"]));
    assert!(!out.status.success(), "exited 0: {}", stderr(&out));
    assert!(stderr(&out).contains("setting mode bogus failed: no mode bogus"), "{}", stderr(&out));
}

/// Closing the last session ends a foreground process as it should: no
/// claim that the session never started.
#[test]
fn adr_0009_foreground_close_of_the_last_session() {
    let env = Env::new("fg-close");
    let mut fg = env
        .brnr(&start_args(&["--foreground", "--quiet"]))
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    assert!(
        wait_for(Duration::from_secs(10), || env.ok(&["list"]).contains("sess-1")),
        "no session"
    );
    env.ok(&["close", "sess-1"]);
    assert!(wait_exit(&mut fg, Duration::from_secs(15)), "didn't stop with its last session");
    let mut err = String::new();
    fg.stderr.take().unwrap().read_to_string(&mut err).unwrap();
    assert!(!err.contains("before the session started"), "{err}");
    assert!(fg.wait().unwrap().success(), "{err}");
}

/// Everything the processes' host logs hold.
fn host_logs(env: &Env) -> String {
    let Ok(dir) = fs::read_dir(env.dir.join("home/hosts")) else { return String::new() };
    dir.map(|e| fs::read_to_string(e.unwrap().path()).unwrap()).collect()
}

/// What is left in the runtime dir.
fn runtime_files(env: &Env) -> Vec<String> {
    let names = |dir: &Path| -> Vec<String> {
        let entries = fs::read_dir(dir).into_iter().flatten();
        entries.map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect()
    };
    // The session locks' directory stays; what matters is what's in it.
    let run = env.dir.join("run");
    let mut files: Vec<String> = names(&run).into_iter().filter(|n| n != "sessions").collect();
    files.extend(names(&run.join("sessions")).into_iter().map(|n| format!("sessions/{n}")));
    files
}

/// `brnr start --foreground … | head -1`: the display stops, with a note in
/// the host log, and the session carries on. Stopped, it records `exited`
/// and leaves nothing behind in the runtime dir.
#[test]
fn adr_0009_foreground_outlives_its_stdout() {
    let env = Env::new("fg-head");
    let (reader, writer) = std::io::pipe().unwrap();
    let mut fg = env
        .brnr(&start_args(&["--foreground", "--prompt", "reply hi"]))
        .stdout(writer)
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut first = String::new();
    BufReader::new(reader).read_line(&mut first).unwrap();
    assert!(first.contains("user: reply hi"), "{first}");
    // Something more to show, on a stdout nobody reads any more.
    assert_eq!(env.ok(&["send", "sess-1", "--wait", "reply again"]), "again\n");
    let stopped = || host_logs(&env).contains(r#""event":"display-stopped""#);
    assert!(wait_for(Duration::from_secs(5), stopped), "no note of the display stopping");
    assert!(env.ok(&["status", "sess-1"]).contains("idle"));
    env.stop();
    assert!(wait_exit(&mut fg, Duration::from_secs(15)), "didn't stop");
    let mut err = String::new();
    fg.stderr.take().unwrap().read_to_string(&mut err).unwrap();
    assert!(fg.wait().unwrap().success(), "{err}");
    assert!(err.contains("brnr: agent exited"), "{err}");
    let exited = env.ok(&["log", "sess-1", "--json", "--events", "exited"]);
    assert!(exited.contains(r#""event":"exited""#), "{exited}");
    assert!(runtime_files(&env).is_empty(), "{:?}", runtime_files(&env));
}

/// A foreground reader that falls behind is skipped past and told how many
/// events it missed; the session isn't held up, and its transcript has them
/// all.
#[test]
fn adr_0009_slow_foreground_reader_is_told_what_it_missed() {
    let env = Env::new("fg-slow");
    let args = ["--foreground", "--stop-when-idle", "0", "--prompt", "many 80000"];
    let mut fg =
        env.brnr(&start_args(&args)).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
    // The session runs to its end while nothing is read.
    let ended = || host_logs(&env).contains(r#""event":"exited""#) && env.hosts().is_empty();
    assert!(wait_for(Duration::from_secs(30), ended), "the session was held up");
    let mut out = String::new();
    fg.stdout.take().unwrap().read_to_string(&mut out).unwrap();
    assert!(fg.wait().unwrap().success());
    let not_shown = |l: &str| l.strip_prefix("… ")?.strip_suffix(" events not shown")?.parse().ok();
    let skipped: Vec<u64> = out.lines().filter_map(not_shown).collect();
    assert!(!skipped.is_empty(), "no line about skipped events");
    let shown = out.lines().filter(|l| not_shown(l).is_none()).count() as u64;
    let logged = env.ok(&["log", "sess-1"]).lines().count() as u64;
    assert!(logged > 80000, "{logged}");
    assert_eq!(shown + skipped.iter().sum::<u64>(), logged);
}

/// In the foreground the agent's stderr goes to stderr as it comes,
/// unchanged; stdout is the events. It is in the host log too.
#[test]
fn adr_0010_foreground_passes_the_agents_stderr() {
    let env = Env::new("fg-stderr").agent("STDERR", "[session/create] phase=ready \x1b[1m");
    let args = ["--foreground", "--stop-when-idle", "0", "--prompt", "reply hi"];
    let out = env.run(&start_args(&args));
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stderr(&out).contains("[session/create] phase=ready \x1b[1m\n"), "{}", stderr(&out));
    assert!(!String::from_utf8_lossy(&out.stdout).contains("phase=ready"));
    let logged = r#""dir":"agent-stderr","raw":"[session/create] phase=ready"#;
    assert!(host_logs(&env).contains(logged));
}

/// A start that fails ends its error with the agent's last lines on stderr:
/// brnr start's, and the foreground's after the agent's stderr itself.
#[test]
fn adr_0010_failed_start_shows_the_agents_stderr() {
    let env = Env::new("errtail").agent("STDERR", "Error: claude CLI not found").agent("EXIT", "1");
    let err = env.fails(&start_args(&["--prompt", "hi"]));
    let want = "the agent exited before the session started. \
                The agent's last lines on stderr:\n  Error: claude CLI not found\n";
    assert!(err.ends_with(want), "{err}");
    let out = env.run(&start_args(&["--foreground", "--prompt", "hi"]));
    assert!(!out.status.success());
    assert_eq!(stderr(&out).matches("Error: claude CLI not found").count(), 2, "{}", stderr(&out));
    assert!(stderr(&out).contains(want), "{}", stderr(&out));
}

/// Answering a permission request while the agent is being stopped can't
/// reach it, so it fails rather than claiming success.
#[test]
fn adr_0027_approve_during_stop_fails() {
    let env = Env::new("stopperm").agent("PERMISSION", "1").agent("STUBBORN", "all");
    env.start(&["--prompt", "edit it"]);
    let waiting =
        || String::from_utf8_lossy(&env.run(&["pending", "sess-1"]).stdout).contains("p1");
    assert!(wait_for(Duration::from_secs(5), waiting), "no permission request");
    assert!(env.run(&["stop", &env.pid()]).status.success());
    let out = env.run(&["approve", "sess-1", "p1"]);
    assert!(!out.status.success(), "approve succeeded during stop");
    assert!(stderr(&out).contains("no longer"), "{}", stderr(&out));
}

/// Messages still held when the agent exits are dropped with an event each,
/// before `exited`, not silently.
#[test]
fn adr_0020_held_messages_are_reported_on_exit() {
    let env = Env::new("held");
    env.start(&["--prompt", "hang on"]);
    let mut watch = env
        .brnr(&["watch", "sess-1", "--json", "--events", "message_dropped,exited"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    sleep(Duration::from_millis(300));
    let out = env.run(&["send", "sess-1", "later"]);
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "held (message m2)");
    assert!(env.run(&["stop", &env.pid()]).status.success());

    assert!(wait_exit(&mut watch, Duration::from_secs(15)), "watch didn't end");
    let lines: Vec<Value> = BufReader::new(watch.stdout.take().unwrap())
        .lines()
        .map(|l| serde_json::from_str(&l.unwrap()).unwrap())
        .collect();
    assert_eq!(lines.len(), 2, "{lines:?}");
    let dropped = &lines[0];
    assert_eq!(dropped["event"], "message_dropped");
    assert_eq!(dropped["session"], "sess-1");
    assert_eq!(dropped["message"], "m2");
    assert_eq!(dropped["text"], "later");
    assert_eq!(dropped["by"], "exit");
    assert_eq!(lines[1]["event"], "exited");
    assert!(lines[1].get("undelivered").is_none(), "{}", lines[1]);
}

// ---- a process's death -------------------------------------------------

/// A panic ends the process, but not without a word (ADR 11), on the event
/// loop or on another thread: the panic is in the host log, with what Rust
/// said of it on stderr; `exited`, with the reason, is in the session's
/// transcript and reaches the peers; the agent goes; and the process's
/// files are removed. (`BRNR_TEST_PANIC` lets a request ask for the panic.)
#[test]
fn adr_0011_a_panic_is_recorded() {
    for on in ["loop", "thread"] {
        let env = Env::new(&format!("panic-{on}"));
        let args = start_args(&["--prompt", "hang on"]);
        let out = env.brnr(&args).env("BRNR_TEST_PANIC", "1").output().unwrap();
        assert!(out.status.success(), "{}", stderr(&out));
        // Held behind the hanging turn, so dropped when the process dies.
        assert!(env.ok(&["send", "sess-1", "later"]).contains("held"));
        let host = env.hosts().remove(0);
        let pid = |key: &str| host[key].as_i64().unwrap() as i32;
        let (host_pid, agent_pid) = (pid("host_pid"), pid("agent_pid"));
        // A peer, subscribed to `exited`, asks for the panic.
        let mut conn = UnixStream::connect(host["socket"].as_str().unwrap()).unwrap();
        conn.set_read_timeout(Some(Duration::from_secs(15))).unwrap();
        let mut from_host = BufReader::new(conn.try_clone().unwrap()).lines();
        writeln!(conn, r#"{{"cmd":"subscribe","events":["exited"]}}"#).unwrap();
        from_host.next().unwrap().unwrap();
        writeln!(conn, r#"{{"cmd":"panic","on":"{on}"}}"#).unwrap();
        let exited: Value = from_host
            .map(|l| serde_json::from_str(&l.unwrap()).unwrap())
            .find(|m: &Value| m["event"] == "exited")
            .expect("no exited");

        assert!(wait_for(Duration::from_secs(15), || !alive(host_pid)), "{on}: still running");
        assert!(wait_for(Duration::from_secs(5), || !alive(agent_pid)), "{on}: the agent lives on");
        let reason = exited["reason"].as_str().unwrap_or_default();
        assert!(reason.starts_with("brnr panicked at src/host/"), "{on}: {exited}");
        assert!(reason.ends_with(": a test asked for it"), "{on}: {exited}");
        let logged = env.ok(&["log", "sess-1", "--json", "--events", "exited"]);
        assert!(logged.contains(reason), "{on}: {logged}");
        let dropped = env.ok(&["log", "sess-1", "--json", "--events", "message_dropped"]);
        assert!(dropped.contains(r#""by":"exit""#), "{on}: {dropped}");
        let shown = env.ok(&["log", "sess-1", "--events", "exited"]);
        assert!(shown.contains(&format!("agent exited: null; {reason}")), "{on}: {shown}");
        let log = host_logs(&env);
        assert!(log.contains(r#""event":{"event":"panic""#), "{on}: {log}");
        let said = log.contains(r#""event":"host-stderr""#) && log.contains("panicked at");
        assert!(said, "{on}: {log}");
        assert!(runtime_files(&env).is_empty(), "{on}: {:?}", runtime_files(&env));
    }
}

/// A panic as the start is under way, once the agent, the log and a bridge
/// are there, is recorded as one on the event loop is (ADR 11): `panic` and
/// `exited` in the host log, so doctor finds no unrecorded death, `exited`
/// for the bridge, the agent killed and the runtime files removed. brnr
/// start fails saying why, and an editor's brnr acp too, with 101.
/// (`BRNR_TEST_PANIC=start` asks for the panic.)
#[test]
fn adr_0011_a_panic_while_starting_is_recorded() {
    let got = |env: &Env| fs::read_to_string(env.dir.join("bridge-events")).unwrap_or_default();
    for editor in [false, true] {
        let env = Env::new(if editor { "panic-start-ed" } else { "panic-start" });
        let script = format!("exec cat > '{}'", env.dir.join("bridge-events").display());
        env.write_config(&format!(
            "[[profiles.default.bridges]]\ncommand = [\"sh\", \"-c\", {script:?}]\n"
        ));
        let args = if editor { vec!["acp", "--", AGENT] } else { start_args(&["--prompt", "hi"]) };
        let out =
            env.brnr(&args).env("BRNR_TEST_PANIC", "start").stdin(Stdio::null()).output().unwrap();
        let err = stderr(&out);
        let said = if editor {
            assert_eq!(out.status.code(), Some(101), "{err}");
            "brnr acp: brnr panicked at src/host/"
        } else {
            assert!(!out.status.success(), "it started");
            "brnr: brnr panicked at src/host/"
        };
        let line = err.lines().find(|l| l.starts_with(said)).unwrap_or_default();
        assert!(line.ends_with(": a test asked for it"), "{editor}: {err}");
        // Then the link to report it (ADR 45), once.
        let command = if editor { "acp" } else { "start" };
        let link = format!("&what=%60brnr%20{command}%60%20panicked");
        assert_eq!(err.matches(&link).count(), 1, "{editor}: {err}");
        assert!(err.trim_end().lines().last().unwrap().starts_with(ISSUE_LINK), "{err}");

        let agent = started_record(&env)["info"]["agent_pid"].as_i64().unwrap() as i32;
        assert!(wait_for(Duration::from_secs(5), || !alive(agent)), "{editor}: the agent lives on");
        let log = host_logs(&env);
        assert!(log.contains(r#""event":{"event":"panic""#), "{editor}: {log}");
        let records = log.lines().map(|l| serde_json::from_str::<Value>(l).unwrap());
        let exited = records.into_iter().find(|r| r["event"]["event"] == "exited");
        let reason = exited.as_ref().and_then(|r| r["event"]["reason"].as_str());
        assert!(reason.is_some_and(|r| r.ends_with(": a test asked for it")), "{editor}: {log}");
        let told = || got(&env).contains(r#""event":"exited""#);
        assert!(wait_for(Duration::from_secs(5), told), "{editor}: the bridge: {}", got(&env));
        assert!(runtime_files(&env).is_empty(), "{editor}: {:?}", runtime_files(&env));
        assert!(env.prompts().is_empty(), "{editor}: the prompt went");
        let doctor = String::from_utf8_lossy(&env.run(&["doctor"]).stdout).into_owned();
        assert!(doctor.contains("no process died without recording it"), "{editor}: {doctor}");
    }
}

/// A panic prints a link to a pre-filled bug report where its user sees it
/// (ADR 45): the CLI's own, on its stderr, after Rust's message; a
/// foreground process's, on its stderr, once. `BRNR_TEST_PANIC=cli` makes
/// the CLI panic.
#[test]
fn adr_0045_a_panic_prints_a_link_to_report_it() {
    let env = Env::new("panic-link");
    let out = env.brnr(&["list"]).env("BRNR_TEST_PANIC", "cli").output().unwrap();
    let err = stderr(&out);
    assert_eq!(out.status.code(), Some(101), "{err}");
    let shown = err.find("a test asked for it").expect("Rust's message");
    let link = err.find(ISSUE_LINK).expect("the link");
    assert!(shown < link, "{err}");
    let url = err[link..].lines().next().unwrap();
    assert!(url.starts_with(&format!("{ISSUE_LINK}Panic%20at%20src%2Fbug.rs%3A")), "{url}");
    let version = format!("&version=brnr%20{}&setup=", env!("CARGO_PKG_VERSION"));
    assert!(url.contains(&version), "{url}");
    assert!(url.contains("&what=%60brnr%20list%60%20panicked%3A"), "{url}");
    assert!(url.contains("a%20test%20asked%20for%20it"), "{url}");
    assert!(!url.contains(' '), "{url}");

    let args = start_args(&["--foreground", "--prompt", "hi"]);
    let out =
        env.brnr(&args).env("BRNR_TEST_PANIC", "start").stdin(Stdio::null()).output().unwrap();
    let err = stderr(&out);
    assert_eq!(out.status.code(), Some(101), "{err}");
    assert_eq!(err.matches(ISSUE_LINK).count(), 1, "{err}");
    assert!(err.contains("&what=%60brnr%20start%20--foreground%60%20panicked"), "{err}");
}

// ---- sending -----------------------------------------------------------

/// Several interrupts sent before the turn stops are delivered in the order
/// they were sent.
#[test]
fn adr_0018_interrupts_keep_their_order() {
    let env = Env::new("interrupt").agent("CANCEL_DELAY", "1");
    env.start(&["--prompt", "hang on"]);
    assert!(wait_for(Duration::from_secs(5), || env.prompts().len() == 1));
    for text in ["first", "second"] {
        let out = env.run(&["send", "sess-1", "--interrupt", text]);
        assert!(out.status.success(), "send: {}", stderr(&out));
    }
    assert!(wait_for(Duration::from_secs(10), || env.prompts().len() == 3), "{:?}", env.prompts());
    assert_eq!(env.prompts(), ["hang on", "first", "second"]);
}

/// The messages of the turns that ended, in order, as `turn_ended` has them.
fn turns(env: &Env) -> Vec<Vec<String>> {
    let log = env.ok(&["log", "sess-1", "--json", "--events", "turn_ended"]);
    let messages = |l: &str| serde_json::from_str::<Value>(l).unwrap()["messages"].take();
    log.lines().map(|l| serde_json::from_value(messages(l)).unwrap()).collect()
}

/// A message sent while a turn runs is held, never a second prompt, and
/// goes as a turn of its own when that one ends, in the order sent.
#[test]
fn adr_0018_send_while_a_turn_runs_is_held() {
    let env = Env::new("held-order");
    env.start(&["--prompt", "hang on"]);
    assert!(wait_for(Duration::from_secs(5), || env.prompts().len() == 1));
    assert_eq!(env.ok(&["send", "sess-1", "reply first"]), "held (message m2)\n");
    assert_eq!(env.ok(&["send", "sess-1", "reply second"]), "held (message m3)\n");
    sleep(Duration::from_millis(300));
    assert_eq!(env.prompts(), ["hang on"], "a second prompt while one ran");
    env.ok(&["cancel", "sess-1", "--keep-held"]);
    let out = env.run(&["wait", "sess-1", "--timeout", "10"]);
    let said = format!("{}{}", String::from_utf8_lossy(&out.stdout), stderr(&out));
    assert_eq!(out.status.code(), Some(0), "{said}{}", env.ok(&["log", "sess-1"]));
    assert_eq!(env.prompts(), ["hang on", "reply first", "reply second"]);
    assert_eq!(turns(&env), [["m1"], ["m2"], ["m3"]]);
}

/// `--steer` goes into the running turn as `_session/steering`: no prompt of
/// its own, its id among that turn's messages, and `send --steer --wait`
/// ends with that turn.
#[test]
fn adr_0017_steer_goes_into_the_running_turn() {
    let env = Env::new("steer");
    env.start(&["--prompt", "hang on"]);
    assert!(wait_for(Duration::from_secs(5), || env.prompts().len() == 1));
    let out = env.run(&["send", "sess-1", "--steer", "--wait", "reply steered"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "steered\n");
    assert!(stderr(&out).contains("steered (message m2)"), "{}", stderr(&out));
    assert_eq!(env.prompts(), ["hang on"], "a prompt of its own");
    let steer = &env.calls_of("_session/steering")[0]["params"];
    assert_eq!(steer["prompt"][0]["text"], "reply steered");
    assert_eq!(steer["_meta"]["steering"]["idleBehavior"], "promptRequired");
    assert_eq!(turns(&env), [["m1", "m2"]]);
    let user = env.ok(&["log", "sess-1", "--events", "user_message"]);
    assert!(user.ends_with("user: reply steered\n"), "{user}");
}

/// `--steer` with no turn running is a prompt; so is one the agent answers
/// `promptRequired` (the turn ended before the steer reached it), which goes
/// ahead of what was held meanwhile, as it would have in the turn.
#[test]
fn adr_0018_steer_without_a_turn_is_a_prompt() {
    let env = Env::new("steer-idle");
    env.start(&[]);
    assert_eq!(env.ok(&["send", "sess-1", "--steer", "reply now"]), "delivered (message m1)\n");
    assert!(wait_for(Duration::from_secs(5), || env.prompts().len() == 1));
    assert!(env.calls_of("_session/steering").is_empty());

    // The agent reads nothing until this turn has ended.
    env.ok(&["send", "sess-1", "slow 2"]);
    assert!(wait_for(Duration::from_secs(5), || env.prompts().len() == 2));
    assert_eq!(env.ok(&["send", "sess-1", "reply held"]), "held (message m3)\n");
    let out = env.run(&["send", "sess-1", "--steer", "--wait", "reply late"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stderr(&out).contains("steered (message m4)"), "{}", stderr(&out));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "late\n");
    assert_eq!(env.calls_of("_session/steering").len(), 1);
    assert_eq!(env.run(&["wait", "sess-1", "--timeout", "10"]).status.code(), Some(0));
    assert_eq!(env.prompts(), ["reply now", "slow 2", "reply late", "reply held"]);
    assert_eq!(turns(&env)[2..], [["m4"], ["m3"]]);
}

/// The session's `turn_ended` and `message_dropped` events, in order, as
/// `(event, messages or message, by)`.
fn endings(env: &Env) -> Vec<(String, Value, Value)> {
    let log = env.ok(&["log", "sess-1", "--json"]);
    let events = log.lines().map(|l| serde_json::from_str::<Value>(l).unwrap());
    events
        .filter(|e| e["event"] == "turn_ended" || e["event"] == "message_dropped")
        .map(|e| {
            let which = if e["event"] == "turn_ended" { &e["messages"] } else { &e["message"] };
            (e["event"].as_str().unwrap().to_owned(), which.clone(), e["by"].clone())
        })
        .collect()
}

/// A steer the agent answers `injected` only after the turn it went into has
/// ended is in that turn all the same: the turn's `turn_ended` waits for the
/// answer and lists it, and `send --steer --wait` ends with it.
#[test]
fn adr_0056_steer_answered_after_its_turn_is_in_that_turn() {
    let env = Env::new("steer-late").agent("STEER_ANSWER", "late");
    env.start(&["--prompt", "hang on"]);
    assert!(wait_for(Duration::from_secs(5), || env.prompts().len() == 1));
    let out = env.run(&["send", "sess-1", "--steer", "--wait", "--timeout", "10", "reply steered"]);
    assert!(out.status.success(), "{}{}", stderr(&out), env.ok(&["log", "sess-1"]));
    // What the agent said came before it said it took the message, so isn't
    // shown as its reply; the transcript has it.
    assert_eq!(String::from_utf8_lossy(&out.stdout), "");
    assert_eq!(turns(&env), [["m1", "m2"]]);
    let said = env.ok(&["log", "sess-1", "--events", "user_message,agent_message"]);
    assert!(said.contains("agent: steered\n") && said.ends_with("user: reply steered\n"), "{said}");
}

/// A steer the agent answers with an error after its turn ended is dropped,
/// `by` `steer`, and that turn ends without it.
#[test]
fn adr_0056_steer_refused_after_its_turn_is_dropped() {
    let env = Env::new("steer-error").agent("STEER_ANSWER", "error");
    env.start(&["--prompt", "hang on"]);
    assert!(wait_for(Duration::from_secs(5), || env.prompts().len() == 1));
    let out = env.run(&["send", "sess-1", "--steer", "--wait", "--timeout", "10", "reply steered"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(stderr(&out).contains("m2 was dropped (steer)"), "{}", stderr(&out));
    let dropped = ("message_dropped".to_owned(), Value::from("m2"), Value::from("steer"));
    let ended = ("turn_ended".to_owned(), serde_json::json!(["m1"]), Value::from("control"));
    assert_eq!(endings(&env), [dropped, ended]);
}

/// A steer the agent never answers keeps its turn's `turn_ended` waiting,
/// since what the turn carried isn't known yet; closing the session drops
/// it, `by` `close`, and the turn ends without it.
#[test]
fn adr_0056_steer_never_answered_is_dropped_by_close() {
    let env = Env::new("steer-never").agent("STEER_ANSWER", "never");
    env.start(&["--prompt", "hang on"]);
    assert!(wait_for(Duration::from_secs(5), || env.prompts().len() == 1));
    let args = ["send", "sess-1", "--steer", "--wait", "--timeout", "20", "reply steered"];
    let waiter = env.brnr(&args).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    assert!(wait_for(Duration::from_secs(5), || env.calls_of("_session/steering").len() == 1));
    sleep(Duration::from_millis(300));
    assert_eq!(endings(&env), [], "the turn ended before its steer was answered");
    env.ok(&["close", "sess-1"]);
    let out = waiter.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(stderr(&out).contains("m2 was dropped (close)"), "{}", stderr(&out));
    let dropped = ("message_dropped".to_owned(), Value::from("m2"), Value::from("close"));
    let ended = ("turn_ended".to_owned(), serde_json::json!(["m1"]), Value::from("control"));
    assert_eq!(endings(&env), [dropped, ended]);
}

/// `--steer` into a running turn needs an agent that advertises steering
/// (P7). With no turn running it is a prompt, steering or not.
#[test]
fn adr_0018_steer_needs_the_agents_steering() {
    let env = Env::new("steer-none").agent("NO_STEERING", "1");
    env.start(&["--prompt", "hang on"]);
    assert!(wait_for(Duration::from_secs(5), || env.prompts().len() == 1));
    let err = env.fails(&["send", "sess-1", "--steer", "reply steered"]);
    let want = "the agent can't steer a running turn: it doesn't advertise _session/steering";
    assert!(err.contains(want), "{err}");
    assert!(env.calls_of("_session/steering").is_empty());
    env.ok(&["cancel", "sess-1"]);
    env.run(&["wait", "sess-1", "--timeout", "10"]);
    assert_eq!(env.ok(&["send", "sess-1", "--steer", "reply now"]), "delivered (message m2)\n");
}

// ---- strict mode -------------------------------------------------------

/// Strict mode (ADR 41), from `--strict` or the profile, is stable ACP
/// only: steering a running turn and forking are refused, saying why, and
/// the rest is as without.
#[test]
fn adr_0041_strict_mode_refuses_steering_and_fork() {
    let env = Env::new("strict");
    env.start(&["--strict", "--prompt", "hang on"]);
    assert_eq!(started_record(&env)["request"]["strict"], true);
    assert!(wait_for(Duration::from_secs(5), || env.prompts().len() == 1));
    let err = env.fails(&["send", "sess-1", "--steer", "reply steered"]);
    let want = "--steer uses _session/steering, an ACP extension, which strict mode doesn't";
    assert!(err.contains(want), "{err}");
    let err = env.fails(&["fork", "sess-1"]);
    let want = "fork uses session/fork, unstable in ACP v1, which strict mode doesn't";
    assert!(err.contains(want), "{err}");
    assert!(env.calls_of("_session/steering").is_empty());
    assert!(env.calls_of("session/fork").is_empty());
    assert_eq!(env.ok(&["send", "sess-1", "reply held"]), "held (message m2)\n");
    env.ok(&["cancel", "sess-1", "--keep-held"]);
    assert!(wait_for(Duration::from_secs(5), || env.prompts().len() == 2));
    env.ok(&["close", "sess-1"]);

    let env = Env::new("strict-profile");
    env.write_config("[profiles.default]\nstrict = true\n");
    env.start(&[]);
    assert!(env.fails(&["fork", "sess-1"]).contains("which strict mode doesn't"));
}

/// The editor's `fs` and `terminal` capabilities don't reach the agent, as
/// in ACP v2 (ADR 2), but in strict mode, as stable v1 has them.
#[test]
fn adr_0041_fs_and_terminal_pass_through_only_in_strict_mode() {
    let initialize = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{"fs":{"readTextFile":true,"writeTextFile":true},"terminal":true}}}"#;
    for (name, strict) in [("ed-caps", false), ("ed-strict", true)] {
        let env = Env::new(name);
        let mut args = vec!["acp", "--", AGENT];
        if strict {
            args.insert(1, "--strict");
        }
        let mut editor =
            env.brnr(&args).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
        let mut to_agent = editor.stdin.take().unwrap();
        let mut from_agent = BufReader::new(editor.stdout.take().unwrap());
        writeln!(to_agent, "{initialize}").unwrap();
        line_with(&mut from_agent, "protocolVersion");
        assert_eq!(started_record(&env)["request"]["strict"], strict);
        let caps = &env.calls_of("initialize")[0]["params"]["clientCapabilities"];
        assert_eq!(caps.get("fs").is_some(), strict, "{name}: {caps}");
        assert_eq!(caps.get("terminal").is_some(), strict, "{name}: {caps}");
        let _ = editor.kill();
        let _ = editor.wait();
    }
    let env = Env::new("ed-strictarg");
    assert!(env.fails(&["acp", "--strict=yes", "--", AGENT]).contains("--strict takes no value"));
}

// ---- watching ----------------------------------------------------------

/// A watcher that stops reading is disconnected instead of having every
/// event queued for it in the host.
#[test]
fn adr_0006_slow_watcher_is_disconnected() {
    // About 30 MB of events: more than a peer's queue holds.
    let env = Env::new("slowwatch").agent("FLOOD", "40000");
    env.start(&[]);
    let mut watch = env
        .brnr(&["watch", "sess-1", "--events", "all"])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    sleep(Duration::from_millis(300));
    kill(watch.id() as i32, libc::SIGSTOP);
    // Stopped until the turn has ended, so every event came while it was:
    // continued sooner, it would be reading again when the turn's 20 MB
    // message came, which a peer that keeps up takes (ADR 60).
    let out = env.run(&["send", "sess-1", "--wait", "go"]);
    assert!(out.status.success(), "send: {}", stderr(&out));
    kill(watch.id() as i32, libc::SIGCONT);

    assert!(wait_exit(&mut watch, Duration::from_secs(20)), "watch is still connected");
    assert!(!watch.wait().unwrap().success());
    let mut err = String::new();
    watch.stderr.take().unwrap().read_to_string(&mut err).unwrap();
    assert!(err.contains("closed the connection"), "{err}");
    // It may still be working through the burst; it must get there.
    let answers = || env.run(&["status", "sess-1"]).status.success();
    assert!(wait_for(Duration::from_secs(30), answers), "host stopped answering");
}

/// A watcher that keeps reading stays connected through a burst, and ends
/// cleanly when the host exits. Stopped once it has read the burst: an
/// exit waits only so long for a peer to be sent what is queued for it.
#[test]
fn adr_0006_reading_watcher_stays_connected() {
    let env = Env::new("fastwatch").agent("FLOOD", "20000");
    env.start(&[]);
    let mut watch = env
        .brnr(&["watch", "sess-1", "--events", "all"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = watch.stdout.take().unwrap();
    let read = Arc::new(AtomicUsize::new(0));
    let counted = read.clone();
    let lines = std::thread::spawn(move || {
        for _ in BufReader::new(stdout).lines() {
            counted.fetch_add(1, Relaxed);
        }
    });
    sleep(Duration::from_millis(300));
    assert!(env.run(&["send", "sess-1", "go"]).status.success());
    let burst = || read.load(Relaxed) > 20000 || watch.try_wait().unwrap().is_some();
    assert!(wait_for(Duration::from_secs(60), burst), "watch read {} lines", read.load(Relaxed));
    assert!(env.run(&["stop", &env.pid()]).status.success());

    assert!(wait_exit(&mut watch, Duration::from_secs(15)), "watch didn't end");
    let mut err = String::new();
    watch.stderr.take().unwrap().read_to_string(&mut err).unwrap();
    assert!(watch.wait().unwrap().success(), "watch failed: {err}");
    lines.join().unwrap();
    assert!(read.load(Relaxed) > 20000, "watch missed events");
}

/// One message bigger than a peer's whole queue still reaches a peer that
/// keeps up, and the status report (which quotes it) still gets out.
#[test]
fn adr_0006_huge_message_reaches_watchers() {
    let env = Env::new("huge");
    env.start(&[]);
    let mut watch = env
        .brnr(&["watch", "sess-1", "--json", "--events", "agent_message"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = watch.stdout.take().unwrap();
    let first = std::thread::spawn(move || BufReader::new(stdout).lines().next());
    sleep(Duration::from_millis(300));
    let size = 20_000_000;
    assert!(env.run(&["send", "sess-1", &format!("big {size}")]).status.success());
    let line = first.join().unwrap().expect("no message").unwrap();
    let event: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(event["text"].as_str().unwrap().len(), size);

    let out = env.run(&["status", "sess-1", "--json"]);
    assert!(out.status.success(), "status: {}", stderr(&out));
    let status: Value = serde_json::from_slice(&out.stdout).unwrap();
    let preview = status["last_message"].as_str().unwrap();
    assert!(preview.chars().count() <= 4001, "status quotes {} chars", preview.chars().count());
    let _ = watch.kill();
    let _ = watch.wait();
}

/// A reply bigger than a peer's whole queue reaches every kind of peer that
/// keeps up, and none is cut off for it (ADR 60): `start --wait`, `send
/// --wait`, a watcher and a bridge, the last two asking for `acp` too, so
/// that the reply comes to them twice at once, as its ACP message and as
/// its event.
#[test]
fn adr_0060_a_reply_bigger_than_the_queue_reaches_every_peer() {
    let env = Env::new("hugeall");
    let got = env.dir.join("bridge-events");
    env.write_config(&format!(
        "[[profiles.default.bridges]]\ncommand = [\"sh\", \"-c\", \"exec cat > '{}'\"]\n\
         events = [\"acp\", \"agent_message\", \"turn_ended\"]\n",
        got.display()
    ));
    let size = 20_000_000;
    let out = env.run(&start_args(&["--wait", "--prompt", &format!("big {size}")]));
    assert!(out.status.success(), "start --wait: {}", stderr(&out));
    assert_eq!(out.stdout.len(), size + 1, "start --wait: {}", stderr(&out));

    let mut watch = env
        .brnr(&["watch", "sess-1", "--json", "--events", "all"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = watch.stdout.take().unwrap();
    let lines = std::thread::spawn(move || {
        let lines = BufReader::new(stdout).lines().map_while(Result::ok);
        let mut sizes = Vec::new();
        for line in lines {
            let event: Value = serde_json::from_str(&line).unwrap();
            if event["event"] == "agent_message" {
                sizes.push(event["text"].as_str().unwrap().len());
            }
        }
        sizes
    });
    sleep(Duration::from_millis(300));
    let out = env.run(&["send", "sess-1", "--wait", &format!("big {size}")]);
    assert!(out.status.success(), "send --wait: {}", stderr(&out));
    assert_eq!(out.stdout.len(), size + 1, "send --wait: {}", stderr(&out));
    let out = env.run(&["send", "sess-1", "--wait", "--json", &format!("big {size}")]);
    assert!(out.status.success(), "send --wait --json: {}", stderr(&out));
    let turn: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(turn["reply"].as_str().unwrap().len(), size);

    assert!(watch.try_wait().unwrap().is_none(), "the watcher was cut off");
    let turns = || fs::read_to_string(&got).unwrap_or_default().matches(r#""turn_ended""#).count();
    assert!(wait_for(Duration::from_secs(30), || turns() == 3), "the bridge was cut off");
    env.stop();
    assert!(wait_exit(&mut watch, Duration::from_secs(30)), "watch didn't end");
    let mut err = String::new();
    watch.stderr.take().unwrap().read_to_string(&mut err).unwrap();
    assert!(watch.wait().unwrap().success(), "watch failed: {err}");
    assert_eq!(lines.join().unwrap(), [size, size], "the watcher's messages");
    let bridge = fs::read_to_string(&got).unwrap();
    let messages = bridge.lines().filter(|l| l.contains(r#""event":"agent_message""#));
    assert!(messages.map(str::len).all(|n| n > size), "the bridge's messages");
}

/// A watcher some way behind when a long message comes, one that takes it
/// past the 16 MiB a peer may have queued, isn't cut off for it: not by the
/// message, and not by the `turn_ended` that comes right after it (ADR 49).
/// Here it is 8 MB behind (stopped, for the test), then a 12 MB message
/// comes.
#[test]
fn adr_0049_a_long_message_doesnt_put_a_watcher_behind() {
    let env = Env::new("longmsg");
    env.start(&[]);
    let mut watch = env
        .brnr(&["watch", "sess-1", "--json", "--events", "agent_message,turn_ended"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = watch.stdout.take().unwrap();
    let lines = std::thread::spawn(move || BufReader::new(stdout).lines().count());
    sleep(Duration::from_millis(300));
    kill(watch.id() as i32, libc::SIGSTOP);
    for size in [8_000_000, 12_000_000] {
        let out = env.run(&["send", "sess-1", "--wait", &format!("big {size}")]);
        assert!(out.status.success(), "{}", stderr(&out));
    }
    // The watcher's line after the long message, which it is past the limit
    // with, and a status to make sure.
    assert!(env.run(&["status", "sess-1"]).status.success());
    kill(watch.id() as i32, libc::SIGCONT);
    env.stop();
    assert!(wait_exit(&mut watch, Duration::from_secs(30)), "watch didn't end");
    let mut err = String::new();
    watch.stderr.take().unwrap().read_to_string(&mut err).unwrap();
    assert!(watch.wait().unwrap().success(), "watch failed: {err}");
    assert_eq!(lines.join().unwrap(), 4, "two messages and two turn_endeds");
}

/// Installed the way Homebrew does it: `bin/brnr` and the adapters are
/// symlinks in the prefix's `bin`, which isn't on the editor's PATH. brnr is
/// started by that path and finds an adapter by its bare name next to it.
#[test]
fn adr_0038_adapters_next_to_a_symlinked_brnr() {
    let env = Env::new("linked");
    let bin = env.dir.join("prefix").join("bin");
    fs::create_dir_all(&bin).unwrap();
    symlink(env!("CARGO_BIN_EXE_brnr"), bin.join("brnr")).unwrap();
    symlink(AGENT, bin.join("brnr-claude-adapter")).unwrap();
    // Enough PATH for the fake agent's python3, not the prefix.
    let python = Command::new("sh").args(["-c", "command -v python3"]).output().unwrap();
    let python = String::from_utf8(python.stdout).unwrap();
    let path = format!("{}:/usr/bin:/bin", Path::new(python.trim()).parent().unwrap().display());

    let doctor = env.brnr_at(&bin.join("brnr"), &["doctor"]).env("PATH", &path).output().unwrap();
    let doctor = String::from_utf8_lossy(&doctor.stdout).into_owned();
    let want = format!("ok    brnr-claude-adapter: {}", bin.join("brnr-claude-adapter").display());
    assert!(doctor.contains(&want), "{doctor}");

    let args = ["start", "--wait", "--prompt", "reply linked", "--", "brnr-claude-adapter"];
    let out = env.brnr_at(&bin.join("brnr"), &args).env("PATH", &path).output().unwrap();
    assert!(out.status.success(), "start: {}", stderr(&out));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "linked\n");
}

/// A started bridge's bare command is found next to a symlinked brnr as an
/// adapter is, by whoever starts the process (ADR 8, ADR 38): the request
/// it hands the process has the path, and the bridge runs.
#[test]
fn adr_0038_bridges_next_to_a_symlinked_brnr() {
    let env = Env::new("linkedbridge");
    let bin = env.dir.join("prefix").join("bin");
    fs::create_dir_all(&bin).unwrap();
    symlink(env!("CARGO_BIN_EXE_brnr"), bin.join("brnr")).unwrap();
    let got = env.dir.join("bridge-events");
    let bridge = bin.join("brnr-test-bridge");
    script(&bridge, &format!("#!/bin/sh\nexec cat > '{}'\n", got.display()));
    env.write_config("[[profiles.default.bridges]]\ncommand = [\"brnr-test-bridge\"]\n");
    // Enough PATH for the fake agent's python3, not the prefix.
    let python = Command::new("sh").args(["-c", "command -v python3"]).output().unwrap();
    let python = String::from_utf8(python.stdout).unwrap();
    let path = format!("{}:/usr/bin:/bin", Path::new(python.trim()).parent().unwrap().display());

    let args = start_args(&["--wait", "--prompt", "reply linked"]);
    let out = env.brnr_at(&bin.join("brnr"), &args).env("PATH", &path).output().unwrap();
    assert!(out.status.success(), "start: {}", stderr(&out));
    let request = started_record(&env)["request"].clone();
    let command = request["bridges"][0]["command"][0].as_str().unwrap_or_default();
    assert_eq!(fs::canonicalize(command).ok(), fs::canonicalize(&bridge).ok(), "{request}");
    let ended = || fs::read_to_string(&got).unwrap_or_default().contains(r#""event":"turn_ended""#);
    assert!(wait_for(Duration::from_secs(5), ended), "the bridge got no events");
    env.stop();
}

/// A peer a few MB behind still takes one big message: it is cut off only
/// once its backlog is past the limit, not because the next line is big.
#[test]
fn adr_0006_big_message_to_a_lagging_watcher() {
    let env = Env::new("lagbig");
    env.start(&[]);
    let mut watch = env
        .brnr(&["watch", "sess-1", "--json", "--events", "agent_message"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = watch.stdout.take().unwrap();
    sleep(Duration::from_millis(300));
    kill(watch.id() as i32, libc::SIGSTOP);
    for size in [6_000_000, 12_000_000] {
        let out = env.run(&["send", "sess-1", "--wait", &format!("big {size}")]);
        assert!(out.status.success(), "send: {}", stderr(&out));
    }
    kill(watch.id() as i32, libc::SIGCONT);
    let lines = std::thread::spawn(move || {
        BufReader::new(stdout).lines().take(2).map(|l| l.unwrap().len()).collect::<Vec<_>>()
    });
    let sizes = lines.join().unwrap();
    assert_eq!(sizes.len(), 2, "the watcher was cut off");
    assert!(sizes[1] > 12_000_000, "{sizes:?}");
    assert!(watch.try_wait().unwrap().is_none(), "the watcher was disconnected");
    let _ = watch.kill();
    let _ = watch.wait();
}

/// Every watcher gets `exited` before the host is gone, not only those whose
/// writer happened to run before the process ended.
#[test]
fn adr_0006_every_watcher_sees_the_exit() {
    let env = Env::new("exitall");
    env.start(&[]);
    let watchers: Vec<_> = (0..8)
        .map(|_| {
            env.brnr(&["watch", "sess-1", "--json", "--events", "exited"])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    sleep(Duration::from_millis(500));
    assert!(env.run(&["stop", &env.pid()]).status.success());
    for watch in watchers {
        let out = watch.wait_with_output().unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(text.contains(r#""event":"exited""#), "no exited: {}", stderr(&out));
    }
}

/// A host that doesn't answer is reported as such, not as an errno.
#[test]
fn unresponsive_host_is_reported() {
    let env = Env::new("unresp");
    env.start(&[]);
    let host = env.host_pid();
    kill(host, libc::SIGSTOP);
    let out = env.run(&["send", "sess-1", "hello"]);
    kill(host, libc::SIGCONT);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("not answering"), "{}", stderr(&out));
}

/// A line that isn't a request (not even UTF-8) is answered with an error,
/// and the connection carries on.
#[test]
fn adr_0035_bad_request_line_is_answered() {
    let env = Env::new("badline");
    env.start(&[]);
    let socket = env.hosts()[0]["socket"].as_str().unwrap().to_owned();
    let mut conn = UnixStream::connect(socket).unwrap();
    conn.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    conn.write_all(b"\xff\xfe not a request\n{\"cmd\":\"status\",\"req_id\":1}\n").unwrap();
    let mut lines = BufReader::new(conn).lines();
    let mut next = || -> Value { serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap() };
    let bad = next();
    assert!(bad["error"].as_str().unwrap_or_default().starts_with("bad request"), "{bad}");
    let status = next();
    assert_eq!(
        (status["req_id"].as_i64(), status["ok"].as_bool()),
        (Some(1), Some(true)),
        "{status}"
    );
}

/// `brnr list | head -1`: a reader that goes away ends brnr quietly, as it
/// would a filter.
#[test]
fn closed_stdout_ends_quietly() {
    let env = Env::new("epipe");
    env.start(&[]);
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);
    let out = env.brnr(&["list"]).stdout(writer).stderr(Stdio::piped()).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stderr(&out), "");
}

// ---- the runtime dir ---------------------------------------------------

/// What brnr finds in the runtime dir is only trusted if it is private, as
/// the processes require: anyone who can write there could list a process of
/// their own and be sent what brnr sends.
#[test]
fn shared_runtime_dir_is_refused() {
    let env = Env::new("shared");
    assert!(env.ok(&["list"]).contains("no running sessions"), "a missing dir is no error");
    let run = env.dir.join("run");
    fs::create_dir(&run).unwrap();
    fs::set_permissions(&run, fs::Permissions::from_mode(0o777)).unwrap();
    let err = env.fails(&["list"]);
    assert!(err.contains("not a private directory owned by this user"), "{err}");
}

/// A metadata file counts only with its own socket, the one next to it.
#[test]
fn metadata_names_its_own_socket() {
    let env = Env::new("impostor");
    env.start(&[]);
    let mut meta = env.hosts()[0].clone();
    meta["id"] = "4242".into();
    fs::write(env.dir.join("run/4242.json"), meta.to_string()).unwrap();
    let list: Value = serde_json::from_str(&env.ok(&["list", "--json"])).unwrap();
    assert_eq!(list.as_array().unwrap().len(), 1, "{list}");
}

// ---- transcripts -------------------------------------------------------

#[test]
fn transcripts_are_private() {
    let env = Env::new("perms");
    env.start(&["--prompt", "hello"]);
    assert!(wait_for(Duration::from_secs(5), || !env.prompts().is_empty()));
    let home = env.dir.join("home");
    let mut checked = 0;
    let mut check = |path: &Path| {
        let mode = fs::metadata(path).unwrap().permissions().mode() & 0o777;
        env.ok(&["log", "sess-1"]); // Once the transcript is written.
        let want = if path.is_dir() { 0o700 } else { 0o600 };
        assert_eq!(mode, want, "{} is {mode:o}", path.display());
        checked += 1;
    };
    check(&home);
    for sub in ["hosts", "projects"] {
        check(&home.join(sub));
        for entry in fs::read_dir(home.join(sub)).unwrap().flatten() {
            check(&entry.path());
            if entry.path().is_dir() {
                for file in fs::read_dir(entry.path()).unwrap().flatten() {
                    check(&file.path());
                }
            }
        }
    }
    // home, hosts/ and its log, projects/, the folder and its two files
    assert!(checked >= 7, "only {checked} paths checked");
}

/// Each record of a JSONL file.
fn records(path: &Path) -> Vec<Value> {
    let text = fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    text.lines().map(|l| serde_json::from_str(l).unwrap()).collect()
}

/// The one project folder, and its files by name.
fn project(env: &Env) -> (PathBuf, Vec<String>) {
    let projects: Vec<_> = fs::read_dir(env.dir.join("home/projects")).unwrap().flatten().collect();
    assert_eq!(projects.len(), 1, "{projects:?}");
    let dir = projects[0].path();
    let mut names: Vec<String> = fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|f| f.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    (dir, names)
}

/// A session's transcript is two files: its events, which `log`, `list` and
/// `--resume` read, and its raw ACP beside them (ADR 22).
#[test]
fn adr_0022_transcripts_are_two_files() {
    let env = Env::new("twofiles");
    env.start(&["--wait", "--prompt", "reply hi"]);
    env.stop();
    assert!(wait_for(Duration::from_secs(15), || env.hosts().is_empty()));
    let (dir, names) = project(&env);
    assert_eq!(names, ["sess-1.acp.jsonl", "sess-1.jsonl"]);
    let (events, acp) =
        (records(&dir.join("sess-1.jsonl")), records(&dir.join("sess-1.acp.jsonl")));
    // Events only, from the session's opening, which names the raw file,
    // to the agent's exit.
    assert!(events.iter().all(|r| r["event"]["event"].is_string() && r.get("dir").is_none()));
    let opened = &events[0]["event"];
    assert_eq!(opened["event"], "session-opened");
    assert_eq!(opened["acp_log"], dir.join("sess-1.acp.jsonl").to_string_lossy().as_ref());
    assert_eq!(events.last().unwrap()["event"]["event"], "exited");
    // ACP only, the session's, with what joins it to the events.
    assert!(acp.iter().all(|r| r["dir"].is_string() && r.get("event").is_none()), "{acp:?}");
    let host_id = &events[0]["host_id"];
    assert!(acp.iter().all(|r| r["session_id"] == "sess-1" && r["host_id"] == *host_id));
    assert!(acp.iter().any(|r| r["msg"]["method"] == "session/new"));
    assert!(acp.iter().any(|r| r["msg"]["method"] == "session/prompt"));
    // What belongs to no session is in the host log.
    let host = fs::read_dir(env.dir.join("home/hosts")).unwrap().next().unwrap().unwrap().path();
    assert!(records(&host).iter().any(|r| r["msg"]["method"] == "initialize"));
    // One session, for list and --resume.
    let list: Value = serde_json::from_str(&env.ok(&["list", "--inactive", "--json"])).unwrap();
    assert_eq!(list.as_array().unwrap().len(), 1, "{list}");
    assert_eq!(list[0]["session"], "sess-1");
    let cwd = fs::canonicalize(&env.dir).unwrap();
    assert_eq!(list[0]["cwd"], cwd.to_string_lossy().as_ref());
    env.ok(&["start", "--resume", "sess-1", "--wait", "--prompt", "reply again"]);
    assert_eq!(project(&env).1, names, "resumed into other files");
    let acp = env.ok(&["log", "sess-1", "--events", "acp", "--json"]);
    assert_eq!(acp.matches(r#""method":"session/prompt""#).count(), 2, "{acp}");
}

/// Session ids the former `_` for anything unsafe in a file name, or a file
/// system that ignores case or Unicode normalization, put in one file.
const COLLIDING: &[&str] =
    &["a/b", "a_b", "x.acp", "x_acp", "Sess-1", "sess-1", "\u{e9}", "e\u{301}"];

/// `brnr start` of the fake agent, its session `id`.
fn start_as(env: &Env, id: &str) {
    let out =
        env.brnr(&start_args(&["--wait", "--prompt", "reply hi"])).env("SESSION_ID", id).output();
    let out = out.unwrap();
    assert!(out.status.success(), "start {id:?}: {}", stderr(&out));
}

/// Distinct session ids have distinct transcripts, one after the other in
/// one folder, and each is listed, by its own id (ADR 53).
#[test]
fn adr_0053_distinct_ids_never_share_a_transcript() {
    let env = Env::new("colliding");
    for id in COLLIDING {
        start_as(&env, id);
        env.stop();
        assert!(wait_for(Duration::from_secs(15), || env.hosts().is_empty()));
    }
    let (dir, names) = project(&env);
    let events: Vec<&String> = names.iter().filter(|n| !n.ends_with(".acp.jsonl")).collect();
    assert_eq!(events.len(), COLLIDING.len(), "{names:?}");
    for name in events {
        let ids: Vec<Value> =
            records(&dir.join(name)).iter().map(|r| r["session_id"].clone()).collect();
        assert!(ids.iter().all(|id| *id == ids[0]), "{name}: {ids:?}");
    }
    let list: Value = serde_json::from_str(&env.ok(&["list", "--inactive", "--json"])).unwrap();
    let mut listed: Vec<&str> =
        list.as_array().unwrap().iter().map(|r| r["session"].as_str().unwrap()).collect();
    listed.sort();
    let mut want = COLLIDING.to_vec();
    want.sort();
    assert_eq!(listed, want);
    for id in COLLIDING {
        let log = env.ok(&["log", id, "--json"]);
        assert_eq!(log.matches(r#""event":"turn_ended""#).count(), 1, "{id:?}: {log}");
    }
}

/// Distinct session ids are owned apart: each running process holds a lock
/// of its own, and each session is reached in its own (ADR 3, ADR 53).
#[test]
fn adr_0053_distinct_ids_never_share_a_lock() {
    let env = Env::new("colliding-locks");
    for id in COLLIDING {
        start_as(&env, id);
    }
    // Each lock names its own session and process.
    let mut locked: Vec<(String, Value)> = fs::read_dir(env.dir.join("run/sessions"))
        .unwrap()
        .map(|e| serde_json::from_slice(&fs::read(e.unwrap().path()).unwrap()).unwrap())
        .map(|l: Value| (l["session"].as_str().unwrap().to_owned(), l["pid"].clone()))
        .collect();
    locked.sort_by(|a, b| a.0.cmp(&b.0));
    let mut want = COLLIDING.to_vec();
    want.sort();
    assert_eq!(locked.iter().map(|(s, _)| s.as_str()).collect::<Vec<_>>(), want);
    let list: Value = serde_json::from_str(&env.ok(&["list", "--json"])).unwrap();
    let rows = list.as_array().unwrap();
    assert_eq!(rows.len(), COLLIDING.len(), "{list}");
    for (id, pid) in &locked {
        let row =
            rows.iter().find(|r| r["session"] == id.as_str()).unwrap_or_else(|| panic!("{id:?}"));
        assert_eq!((&row["state"], &row["pid"]), (&Value::from("idle"), pid), "{id:?}: {list}");
        env.ok(&["send", id, "reply again"]);
    }
}

/// `log = "events"` leaves out the raw ACP file, and `log --events acp`
/// says there is none; what belongs to no session is still in the host log.
#[test]
fn adr_0022_log_events_leaves_out_the_raw_acp() {
    let env = Env::new("logevents");
    env.write_config("[profiles.default]\nlog = \"events\"\n");
    env.start(&["--wait", "--prompt", "reply hi"]);
    for running in [true, false] {
        let out = env.run(&["log", "sess-1", "--events", "acp,agent_message"]);
        assert!(out.status.success(), "{}", stderr(&out));
        let err = stderr(&out);
        assert!(err.contains(r#"no raw ACP for sess-1: log = "events" leaves it out"#), "{err}");
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        let lines: Vec<&str> = text.lines().map(|l| &l[10..]).collect();
        assert_eq!(lines, ["agent: hi"], "running: {running}");
        if running {
            env.stop();
            assert!(wait_for(Duration::from_secs(15), || env.hosts().is_empty()));
        }
    }
    assert_eq!(project(&env).1, ["sess-1.jsonl"]);
    let host = fs::read_dir(env.dir.join("home/hosts")).unwrap().next().unwrap().unwrap().path();
    let host = records(&host);
    assert!(host.iter().any(|r| r["msg"]["method"] == "initialize"));
    assert!(!host.iter().any(|r| r["msg"]["method"] == "session/prompt"), "a session's ACP");
}

/// A process that exits is listed, and holds its sessions, until its
/// transcript has `exited`, so that what reads it once the process has gone
/// (`log`, `list --all`, `--resume`) reads it whole (ADR 48). A stalled
/// disk holds it up for 2 s at most.
#[test]
fn adr_0048_an_exit_is_written_before_the_process_goes() {
    let env = Env::new("exitlog");
    let stall = env.dir.join("stall");
    let env = env.agent("BRNR_TEST_LOG_STALL", &stall.to_string_lossy());
    env.start(&["--wait", "--prompt", "reply hi"]);
    env.ok(&["log", "sess-1"]);
    fs::write(&stall, "").unwrap();
    env.stop();
    sleep(Duration::from_millis(500));
    assert_eq!(env.hosts().len(), 1, "gone before its transcript was written");
    assert!(wait_for(Duration::from_secs(15), || env.hosts().is_empty()), "the disk held it");
    fs::remove_file(&stall).unwrap();
}

/// A disk too slow for brnr's own record neither slows the session nor
/// fills the host's memory: past 64 MiB queued for the logger, records are
/// skipped and counted, and once it catches up a `records-skipped` note says
/// how many, in the session's events file and the host log (ADR 6). What was
/// queued is all written. (`BRNR_TEST_LOG_STALL` holds the logger while the
/// file it names exists.)
#[test]
fn adr_0006_a_stalled_disk_skips_records_and_says_so() {
    let env = Env::new("logstall");
    let stall = env.dir.join("stall");
    let env = env.agent("BRNR_TEST_LOG_STALL", &stall.to_string_lossy());
    env.start(&[]);
    let host = env.host_pid();
    let projects = env.dir.join("home/projects");
    let opened = || fs::read_dir(&projects).is_ok_and(|mut d| d.next().is_some());
    assert!(wait_for(Duration::from_secs(5), opened), "no transcript");
    let events = project(&env).0.join("sess-1.jsonl");
    fs::write(&stall, "").unwrap();
    // Some 400 MB for the logger: 4000 messages of 50 kB, each an event and
    // raw ACP. The turn runs to its end while nothing is written. (`wait`,
    // as `send --wait` would take every message.)
    env.ok(&["send", "sess-1", "many 4000 50000"]);
    let out = env.run(&["wait", "sess-1", "--timeout", "60"]);
    assert!(out.status.success(), "the session waited for the logger: {}", stderr(&out));
    // The logger's 64 MiB, and the agent's 16 MiB on its way, with room.
    assert_holds_less(host, 160 << 20);

    fs::remove_file(&stall).unwrap();
    let noted = || fs::read_to_string(&events).is_ok_and(|t| t.contains("records-skipped"));
    assert!(wait_for(Duration::from_secs(30), noted), "no records-skipped note");
    assert_eq!(env.ok(&["send", "sess-1", "--wait", "reply after"]), "after\n");
    env.stop();
    assert!(wait_for(Duration::from_secs(15), || env.hosts().is_empty()));

    let events: Vec<Value> = records(&events).into_iter().map(|r| r["event"].clone()).collect();
    let at = |name: &str| events.iter().position(|e| e["event"] == name).unwrap();
    let note = &events[at("records-skipped")];
    let (count, acp) = (note["count"].as_u64().unwrap(), note["acp"].as_u64().unwrap());
    assert!(acp > 0 && count > acp, "{note}");
    assert!(note["since"].as_str().unwrap() <= note["until"].as_str().unwrap(), "{note}");
    // The turn's messages up to the gap, in order, none missing, then the
    // note; what came later was written again.
    let turn: Vec<&str> = events[..at("records-skipped")]
        .iter()
        .filter(|e| e["event"] == "agent_message")
        .map(|e| e["text"].as_str().unwrap())
        .collect();
    assert!(!turn.is_empty() && turn.len() < 4000, "{} messages written", turn.len());
    for (i, text) in turn.iter().enumerate() {
        assert!(text.starts_with(&format!("message {i}m")), "message {i}: {}", &text[..20]);
    }
    assert!(turn.len() as u64 + (count - acp) >= 4000, "{} written, {note}", turn.len());
    let after = events.iter().rposition(|e| e["event"] == "agent_message").unwrap();
    assert!(after > at("records-skipped") && events[after]["text"] == "after");
    assert_eq!(events.iter().filter(|e| e["event"] == "records-skipped").count(), 1);
    // The host log counts every record the gap skipped.
    let host = fs::read_dir(env.dir.join("home/hosts")).unwrap().next().unwrap().unwrap().path();
    let host = records(&host);
    let host_note = host.iter().find(|r| r["event"]["event"] == "records-skipped").unwrap();
    assert!(host_note["event"]["count"].as_u64().unwrap() >= count, "{host_note}");
    // log tells its reader of the gap, whatever --events chose.
    let log = env.ok(&["log", "sess-1", "--events", "user_message"]);
    assert!(log.contains(&format!("{count} records not written ({acp} of them raw ACP)")), "{log}");
    let json = env.ok(&["log", "sess-1", "--json", "--events", "user_message"]);
    assert!(json.contains(r#""event":"records-skipped""#), "{json}");
}

// ---- secrets -----------------------------------------------------------

/// Everything brnr has recorded: every file under its state directory.
fn everything_recorded(dir: &Path) -> String {
    let mut text = String::new();
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            text.push_str(&everything_recorded(&path));
        } else {
            text.push_str(&fs::read_to_string(&path).unwrap());
        }
    }
    text
}

/// The first event `watch --json` prints that is the ACP request `method`.
fn watched_request(watch: &mut Child, method: &str) -> Value {
    let mut lines = BufReader::new(watch.stdout.as_mut().unwrap()).lines();
    loop {
        let line = lines.next().expect("watch ended").unwrap();
        let event: Value = serde_json::from_str(&line).unwrap();
        if event["msg"]["method"] == method {
            return event;
        }
    }
}

/// The values of a profile's MCP servers' `env` and `headers` reach the
/// agent, and are `<redacted>` in what brnr records (the host log, the raw
/// ACP, the started request) and in `acp` events (ADR 25).
#[test]
fn adr_0025_a_profiles_mcp_secrets_are_redacted() {
    let env = Env::new("secrets");
    env.write_config(
        r#"[[profiles.default.headless.mcp_servers]]
name = "github"
command = "true"
env = { GITHUB_TOKEN = "env-secret" }

[[profiles.default.headless.mcp_servers]]
name = "web"
url = "https://example.invalid/mcp"
headers = { Authorization = "Bearer header-secret" }
"#,
    );
    env.start(&["--wait", "--prompt", "reply hi"]);
    let args = ["watch", "sess-1", "--events", "acp", "--json"];
    let mut watch = env.brnr(&args).stdout(Stdio::piped()).spawn().unwrap();
    sleep(Duration::from_millis(300));
    env.ok(&["fork", "sess-1"]);
    let redacted = |servers: &Value| {
        let env = serde_json::json!([{ "name": "GITHUB_TOKEN", "value": "<redacted>" }]);
        assert_eq!(servers[0]["env"], env);
        assert_eq!(servers[1]["headers"][0]["name"], "Authorization");
        assert_eq!(servers[1]["headers"][0]["value"], "<redacted>");
        assert_eq!(servers[1]["url"], "https://example.invalid/mcp");
    };
    redacted(&watched_request(&mut watch, "session/fork")["msg"]["params"]["mcpServers"]);
    let _ = watch.kill();
    for method in ["session/new", "session/fork"] {
        let servers = &env.calls_of(method)[0]["params"]["mcpServers"];
        assert_eq!(servers[0]["env"][0]["value"], "env-secret", "{method}");
        assert_eq!(servers[1]["headers"][0]["value"], "Bearer header-secret", "{method}");
    }
    redacted(&started_record(&env)["request"]["role"]["headless"]["mcp_servers"]);
    let acp = env.ok(&["log", "sess-1", "--events", "acp", "--json"]);
    let new = acp.lines().map(|l| serde_json::from_str::<Value>(l).unwrap());
    let new = new.into_iter().find(|e| e["msg"]["method"] == "session/new").expect(&acp);
    redacted(&new["msg"]["params"]["mcpServers"]);
    env.stop();
    assert!(wait_for(Duration::from_secs(15), || env.hosts().is_empty()));
    let all = everything_recorded(&env.dir.join("home"));
    assert!(!all.contains("env-secret") && !all.contains("header-secret"), "a secret recorded");
    assert!(all.contains("<redacted>"));
}

/// An editor's own `session/new` reaches the agent as the editor sent it;
/// what brnr records of it, and sends to `acp` subscribers, has its MCP
/// servers' secrets redacted (ADR 25).
#[test]
fn adr_0025_an_editors_mcp_secrets_are_redacted() {
    let env = Env::new("ed-secrets");
    let mut editor = env
        .brnr(&["acp", "--", AGENT])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut to_agent = editor.stdin.take().unwrap();
    let mut from_agent = BufReader::new(editor.stdout.take().unwrap());
    let mut answer = |id: u64| -> Value {
        let mut line = String::new();
        loop {
            line.clear();
            assert!(from_agent.read_line(&mut line).unwrap() > 0, "no answer to {id}");
            let msg: Value = serde_json::from_str(&line).unwrap();
            if msg["id"] == id {
                return msg;
            }
        }
    };
    writeln!(to_agent, "{INITIALIZE}").unwrap();
    answer(1);
    let args = ["watch", "--pid", &env.pid(), "--events", "acp", "--json"];
    let mut watch = env.brnr(&args).stdout(Stdio::piped()).spawn().unwrap();
    sleep(Duration::from_millis(300));
    let header = serde_json::json!({ "name": "Authorization", "value": "Bearer editor-secret" });
    let url = "https://example.invalid";
    let server =
        serde_json::json!({ "type": "http", "name": "api", "url": url, "headers": [header] });
    let params = serde_json::json!({ "cwd": env.dir, "mcpServers": [server] });
    let new =
        serde_json::json!({ "jsonrpc": "2.0", "id": 2, "method": "session/new", "params": params });
    writeln!(to_agent, "{new}").unwrap();
    assert_eq!(answer(2)["result"]["sessionId"], "sess-1");
    assert_eq!(env.calls_of("session/new")[0], new);
    let seen = watched_request(&mut watch, "session/new");
    let server = &seen["msg"]["params"]["mcpServers"][0];
    let header = serde_json::json!({ "name": "Authorization", "value": "<redacted>" });
    assert_eq!(server["headers"], serde_json::json!([header]));
    assert_eq!((&server["name"], &server["url"]), (&"api".into(), &url.into()));
    let host = env.host_pid();
    drop(to_agent);
    assert!(wait_exit(&mut editor, Duration::from_secs(15)), "acp didn't exit");
    assert!(wait_for(Duration::from_secs(15), || !alive(host)));
    let _ = watch.kill();
    let all = everything_recorded(&env.dir.join("home"));
    assert!(!all.contains("editor-secret"), "a secret recorded");
    assert_eq!(all.matches(r#""value":"<redacted>""#).count(), 2, "host log and raw file");
}

/// The runtime directory check doesn't follow a symlink to some other
/// private directory.
#[test]
fn runtime_dir_symlink_is_refused() {
    let env = Env::new("symlink");
    let target = env.dir.join("elsewhere");
    fs::create_dir(&target).unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o700)).unwrap();
    symlink(&target, env.dir.join("run")).unwrap();
    let out = env.run(&start_args(&[]));
    assert!(!out.status.success(), "started in a symlinked runtime dir");
    assert!(fs::read_dir(&target).unwrap().next().is_none(), "wrote into the symlink target");
}

/// `brnr acp` is what an editor runs as its agent: ACP over stdio, with the
/// session reachable meanwhile. When the editor goes, the agent goes too.
#[test]
fn adr_0002_acp_is_what_an_editor_runs() {
    use std::io::Write;
    let env = Env::new("ed-acp");
    env.write_config("[profiles.default.editor]\nexperimental = [\"send\"]\n");
    let mut editor = env
        .brnr(&["acp", "--", AGENT])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut to_agent = editor.stdin.take().unwrap();
    let mut from_agent = BufReader::new(editor.stdout.take().unwrap());
    let mut answer = |id: u64| -> Value {
        let mut line = String::new();
        loop {
            line.clear();
            assert!(from_agent.read_line(&mut line).unwrap() > 0, "no answer to {id}");
            let msg: Value = serde_json::from_str(&line).unwrap();
            if msg["id"] == id {
                return msg;
            }
        }
    };
    let initialize = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{}}}"#;
    writeln!(to_agent, "{initialize}").unwrap();
    assert_eq!(answer(1)["result"]["protocolVersion"], 1);
    let new = format!(
        r#"{{"jsonrpc":"2.0","id":2,"method":"session/new","params":{{"cwd":{:?},"mcpServers":[]}}}}"#,
        env.dir.display().to_string()
    );
    writeln!(to_agent, "{new}").unwrap();
    assert_eq!(answer(2)["result"]["sessionId"], "sess-1");
    assert!(env.ok(&["ps"]).contains("editor"));
    assert!(env.ok(&["list"]).contains("sess-1"));
    // A message sent from outside is shown to the editor as a completed
    // tool call; its response (to brnr's own prompt id) is kept from it.
    env.ok(&["send", "sess-1", "reply hi"]);
    let echo = loop {
        let mut line = String::new();
        assert!(from_agent.read_line(&mut line).unwrap() > 0, "no echo");
        let msg: Value = serde_json::from_str(&line).unwrap();
        let update = &msg["params"]["update"];
        if update["sessionUpdate"] == "tool_call" {
            break update.clone();
        }
        assert!(msg.get("id").is_none(), "the editor saw brnr's prompt: {line}");
    };
    assert!(echo["toolCallId"].as_str().unwrap().starts_with("brnr-echo-"), "{echo}");
    assert_eq!(echo["title"], "Message via brnr");
    assert_eq!(echo["status"], "completed");
    assert_eq!(echo["content"][0]["content"]["text"], "reply hi");
    let host = env.host_pid();
    drop(to_agent); // The editor goes away, and the agent with it.
    assert!(wait_exit(&mut editor, Duration::from_secs(15)), "acp didn't exit");
    assert!(wait_for(Duration::from_secs(15), || !alive(host)), "the agent outlived the editor");
    assert!(env.ok(&["log", "sess-1"]).contains("title: Fake session"));
    let err = env.fails(&["acp", "--on-disconnect", "headless", "--", AGENT]);
    assert!(err.contains("unknown option: --on-disconnect"), "{err}");
}

const INITIALIZE: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{}}}"#;

/// `brnr acp` resolves the profile itself: a config error is the editor's
/// to see, on its stderr, and no process starts.
#[test]
fn adr_0008_acp_reports_a_config_error() {
    let env = Env::new("ed-config");
    env.write_config("[profiles.default]\npermission_timeout = 600\n");
    let out = env.brnr(&["acp", "--", AGENT]).stdin(Stdio::null()).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let err = stderr(&out);
    assert!(err.starts_with("brnr acp: "), "{err}");
    assert!(err.contains("profiles.default: permission_timeout is for brnr start only; it goes under [profiles.default.headless]"), "{err}");
    assert!(env.hosts().is_empty());
}

/// An editor's process gets the profile's shared and editor parts, and the
/// editor's process id and signal mask, in its request.
#[test]
fn adr_0008_acp_hands_over_one_request() {
    let env = Env::new("ed-request");
    env.write_config(
        "[profiles.default]\nstrict = true\nlog = \"events\"\n\n[profiles.default.headless]\nmode = \"plan\"\n\n\
         [profiles.default.editor]\nexperimental = [\"send\", \"approve\"]\nfeatures = [\"shared_sessions\"]\n",
    );
    let (mut editor, _to_agent, _from_agent) = open_editor(&env);
    let request = started_record(&env)["request"].clone();
    assert_eq!(request["strict"], true, "{request}");
    assert_eq!(request["log"], "events");
    let part = &request["role"]["editor"];
    assert_eq!(part["proxy_pid"], editor.id(), "{request}");
    assert_eq!(part["experimental"], serde_json::json!(["send", "approve"]));
    assert_eq!(part["features"], serde_json::json!(["shared_sessions"]));
    assert!(part["sigmask"].is_array());
    assert!(request["role"].get("headless").is_none(), "{request}");
    // No headless setting reaches an editor's session.
    assert!(env.calls_of("session/set_mode").is_empty());
    let ps = Command::new("ps").args(["-o", "args=", "-p", &env.host_pid().to_string()]).output();
    let args = String::from_utf8_lossy(&ps.unwrap().stdout).into_owned();
    assert!(!args.contains("fake_agent"), "agent visible in ps: {args}");
    let _ = editor.kill();
}

/// `brnr acp`, initialized, as an editor starts it.
fn editor(env: &Env) -> (Child, ChildStdin, BufReader<ChildStdout>) {
    editor_with(env, &[])
}

/// `brnr acp <args>`, initialized.
fn editor_with(env: &Env, args: &[&str]) -> (Child, ChildStdin, BufReader<ChildStdout>) {
    let args: Vec<&str> = ["acp"].iter().chain(args).chain(&["--", AGENT]).copied().collect();
    let mut editor = env.brnr(&args).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let mut to_agent = editor.stdin.take().unwrap();
    let mut from_agent = BufReader::new(editor.stdout.take().unwrap());
    writeln!(to_agent, "{INITIALIZE}").unwrap();
    response(&mut from_agent, 1);
    (editor, to_agent, from_agent)
}

/// The editor's answer to its request `id`, past what comes before it.
fn response(from_agent: &mut BufReader<ChildStdout>, id: u64) -> Value {
    let mut line = String::new();
    loop {
        line.clear();
        assert!(from_agent.read_line(&mut line).unwrap() > 0, "no answer to {id}");
        let msg: Value = serde_json::from_str(&line).unwrap();
        if msg["id"] == id {
            return msg;
        }
    }
}

/// `brnr acp` with a session open (sess-1), as an editor has it.
fn open_editor(env: &Env) -> (Child, ChildStdin, BufReader<ChildStdout>) {
    open_editor_with(env, &[])
}

/// `brnr acp <args>` with sess-1 open.
fn open_editor_with(env: &Env, args: &[&str]) -> (Child, ChildStdin, BufReader<ChildStdout>) {
    let (editor, mut to_agent, mut from_agent) = editor_with(env, args);
    let new = format!(
        r#"{{"jsonrpc":"2.0","id":2,"method":"session/new","params":{{"cwd":{:?},"mcpServers":[]}}}}"#,
        env.dir.display().to_string()
    );
    writeln!(to_agent, "{new}").unwrap();
    assert_eq!(response(&mut from_agent, 2)["result"]["sessionId"], "sess-1");
    (editor, to_agent, from_agent)
}

/// The editor closing its session (`session/close`) is a `session_closed`,
/// by the editor, in the session's transcript; the process stays the
/// editor's.
#[test]
fn adr_0020_editor_close_is_an_event() {
    let env = Env::new("ed-close");
    let (_editor, mut to_agent, mut from_agent) = open_editor(&env);
    let close =
        r#"{"jsonrpc":"2.0","id":3,"method":"session/close","params":{"sessionId":"sess-1"}}"#;
    writeln!(to_agent, "{close}").unwrap();
    let mut line = String::new();
    while !serde_json::from_str::<Value>(&line).is_ok_and(|m| m["id"] == 3) {
        line.clear();
        assert!(from_agent.read_line(&mut line).unwrap() > 0, "no answer to the close");
    }
    assert!(!env.ok(&["list"]).contains("sess-1"), "the session is still listed");
    let log = env.ok(&["log", "sess-1", "--json", "--events", "session_closed"]);
    let closed: Value =
        serde_json::from_str(log.lines().next().expect("no session_closed")).unwrap();
    assert_eq!((&closed["session"], &closed["by"]), (&"sess-1".into(), &"editor".into()));
    assert!(env.ok(&["ps"]).contains("editor"));
}

/// When the editor goes, what the agent started goes too: its process
/// group, as when it is stopped.
#[test]
fn adr_0002_editor_gone_takes_the_agents_children() {
    let env = Env::new("ed-kids").agent("STUBBORN", "child");
    let (mut editor, _to_agent, _from_agent) = open_editor(&env);
    let (host, child) = (env.host_pid(), env.child_pid());
    editor.kill().unwrap();
    editor.wait().unwrap();
    assert!(wait_for(Duration::from_secs(15), || !alive(host)), "the agent outlived the editor");
    assert!(wait_for(Duration::from_secs(2), || !alive(child)), "its child outlived the editor");
}

/// That the host, `pid`, holds less than `limit` bytes of memory, unless it
/// is built under a sanitizer (`--cfg sanitized`): its memory is then mostly
/// the sanitizer's, such as the freed memory AddressSanitizer holds back to
/// catch a use after free (256 MB on Linux), and its shadow.
fn assert_holds_less(pid: i32, limit: u64) {
    let ps = Command::new("ps").args(["-o", "rss=", "-p", &pid.to_string()]).output().unwrap();
    let held = String::from_utf8_lossy(&ps.stdout).trim().parse::<u64>().unwrap_or(0) << 10;
    assert!(cfg!(sanitized) || held < limit, "the host holds {} MB", held >> 20);
}

/// An editor that stops reading holds its agent back, as a pipe would: the
/// host keeps no more of the agent's output than its cap (some 80 MB comes),
/// the agent waits as long as the editor does (30 s here, once the time
/// after which the host gave up on the editor and ended the agent), and goes
/// on once the editor reads again.
#[test]
fn adr_0006_editor_that_stops_reading_holds_the_agent_back() {
    let env = Env::new("ed-stall").agent("NOISE", "150000");
    let (_editor, mut to_agent, mut from_agent) = open_editor(&env);
    let host = env.host_pid();
    writeln!(to_agent, "{}", editor_prompt(3, "reply done")).unwrap();
    sleep(Duration::from_secs(32));
    assert!(alive(host), "a stalled editor ended its agent");
    assert_holds_less(host, 64 << 20);
    assert!(env.ok(&["ps"]).contains("editor"));
    // The agent went on: its answer comes after everything it wrote.
    line_with(&mut from_agent, "end_turn");
    assert!(alive(host) && env.ok(&["ps"]).contains("editor"));
}

/// The agent's stdin likewise: an agent that stops reading holds back what
/// the editor writes, and the host keeps no more of it than its cap. The
/// editor going away is still noticed, and takes the agent with it.
#[test]
fn adr_0006_a_stalled_agent_holds_the_editor_back() {
    let env = Env::new("ed-full").agent("STALL", "1");
    let (mut editor, to_agent, _from_agent) = open_editor(&env);
    let host = env.host_pid();
    let writer = flood(to_agent);
    sleep(Duration::from_secs(5));
    assert!(!writer.is_finished(), "the editor's writes didn't wait");
    assert_holds_less(host, 64 << 20);
    editor.kill().unwrap();
    editor.wait().unwrap();
    assert!(wait_for(Duration::from_secs(15), || !alive(host)), "the agent outlived the editor");
    assert!(!writer.join().unwrap(), "the editor's writes all went through");
}

/// 200 MB of notifications from the editor, on their own thread: whether
/// they all went through.
fn flood(mut to_agent: ChildStdin) -> std::thread::JoinHandle<bool> {
    std::thread::spawn(move || {
        let params = serde_json::json!({ "pad": "x".repeat(100_000) });
        let line = serde_json::json!({ "jsonrpc": "2.0", "method": "_noise", "params": params });
        let line = format!("{line}\n");
        (0..2000).all(|_| to_agent.write_all(line.as_bytes()).is_ok())
    })
}

/// The `line_too_long` events in sess-1's transcript (ADR 51).
fn too_long(env: &Env) -> Vec<Value> {
    let log = env.ok(&["log", "sess-1", "--json", "--events", "line_too_long"]);
    log.lines().map(|l| serde_json::from_str(l).unwrap()).collect()
}

/// Headless, a line from the agent past the limit isn't read, whether it is
/// one message of 40 MB or 40 MiB of no JSON at all: it is dropped, an event
/// says so, and the session goes on (ADR 51).
#[test]
fn adr_0051_headless_an_agent_line_past_the_limit_is_dropped_and_said() {
    let env = Env::new("toolong");
    env.start(&[]);
    for prompt in ["big 40000000", "long 40"] {
        let out = env.run(&["send", "sess-1", "--wait", prompt]);
        assert!(out.status.success(), "send {prompt}: {}", stderr(&out));
        assert!(out.stdout.is_empty(), "send {prompt} printed {} bytes", out.stdout.len());
    }
    let events = too_long(&env);
    assert_eq!(events.len(), 2, "{events:?}");
    let dropped = |e: &Value| e["from"] == "agent" && e["relayed"] == false;
    assert!(events.iter().all(dropped), "{events:?}");
    assert_eq!(env.ok(&["send", "sess-1", "--wait", "reply after"]), "after\n");
    env.stop();
}

/// A line from the agent with no newline in sight is held back as any other
/// output is when the editor stops reading: the host keeps no more of it
/// than its cap and the most of a line it reads, 48 MiB in all (128 MiB
/// comes, its newline last), and the editor gets it unchanged once it reads
/// again (ADR 51).
#[test]
fn adr_0051_a_long_line_to_an_editor_that_stops_reading_is_held_back() {
    let env = Env::new("ed-long");
    let (_editor, mut to_agent, mut from_agent) = open_editor(&env);
    let host = env.host_pid();
    writeln!(to_agent, "{}", editor_prompt(3, "long 128")).unwrap();
    let said = wait_for(Duration::from_secs(15), || !too_long(&env).is_empty());
    assert!(said, "no line_too_long");
    sleep(Duration::from_secs(1));
    assert_holds_less(host, 96 << 20);
    let mut line = Vec::new();
    while !line.starts_with(b"x") {
        line.clear();
        assert!(from_agent.read_until(b'\n', &mut line).unwrap() > 0, "no long line");
    }
    assert_eq!(line.len(), (128 << 20) + 1);
    assert!(line[..128 << 20].iter().all(|&b| b == b'x') && line.ends_with(b"\n"));
    line_with(&mut from_agent, "end_turn");
    let events = too_long(&env);
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!((&events[0]["from"], &events[0]["relayed"]), (&"agent".into(), &true.into()));
}

/// A line from the editor past the limit goes to the agent as it came,
/// unread by the host, and an event says so (ADR 51).
#[test]
fn adr_0051_an_editor_line_past_the_limit_goes_to_the_agent_unread() {
    let env = Env::new("ed-longin");
    let (_editor, mut to_agent, mut from_agent) = open_editor(&env);
    let params = serde_json::json!({ "pad": "e".repeat(40 << 20) });
    let line = serde_json::json!({ "jsonrpc": "2.0", "method": "_noise", "params": params });
    writeln!(to_agent, "{line}").unwrap();
    writeln!(to_agent, "{}", editor_prompt(3, "reply after")).unwrap();
    line_with(&mut from_agent, "end_turn");
    let noise = env.calls_of("_noise");
    assert_eq!(noise.len(), 1);
    assert_eq!(noise[0]["params"]["pad"].as_str().unwrap().len(), 40 << 20);
    let events = too_long(&env);
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!((&events[0]["from"], &events[0]["relayed"]), (&"editor".into(), &true.into()));
}

/// The agent's stderr is passed on in pieces, not whole lines: 96 MiB of it
/// with no newline leaves the host holding none of it (ADR 51). (No
/// transcripts: the host log's queue isn't what is measured.)
#[test]
fn adr_0051_the_agents_stderr_without_a_newline_is_bounded() {
    let env = Env::new("errlong");
    env.write_config("[profiles.default]\nlog = false\n");
    env.start(&[]);
    let host = env.host_pid();
    let out = env.run(&["send", "sess-1", "--wait", "long 96 stderr"]);
    assert!(out.status.success(), "send: {}", stderr(&out));
    assert_holds_less(host, 64 << 20);
    env.stop();
}

/// A signal to `brnr acp` reaches the agent at once, as it would the agent
/// run directly, while the agent's stdin holds back what the editor writes:
/// it doesn't wait behind it. The agent dies of it, and so `brnr acp` does.
#[test]
fn adr_0002_a_signal_doesnt_wait_behind_a_stalled_agents_stdin() {
    let env = Env::new("ed-signal").agent("STALL", "1");
    let (mut editor, to_agent, _from_agent) = open_editor(&env);
    let host = env.host_pid();
    let writer = flood(to_agent);
    sleep(Duration::from_secs(3));
    assert!(!writer.is_finished(), "the editor's writes didn't wait");
    kill(editor.id() as i32, libc::SIGTERM);
    assert!(wait_exit(&mut editor, Duration::from_secs(5)), "the agent didn't get the signal");
    let status = editor.wait().unwrap();
    assert_eq!(status.signal(), Some(libc::SIGTERM), "acp: {status}");
    assert!(wait_for(Duration::from_secs(15), || !alive(host)), "the host lives on");
    assert!(!writer.join().unwrap(), "the editor's writes all went through");
}

/// An editor's process doesn't start without the proxy's signal link on its
/// fd 4, as it doesn't without the link: it tells the proxy so on the link,
/// and the agent never started.
#[test]
fn adr_0002_an_editors_process_needs_its_signal_link() {
    let env = Env::new("ed-nosig");
    let (mut ours, theirs) = UnixStream::pair().unwrap();
    let fd = theirs.as_raw_fd();
    let mut cmd = env.brnr(&["host"]);
    cmd.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped());
    // SAFETY: between fork and exec the closure only calls dup2(2), which is
    // async-signal-safe, and allocates nothing.
    unsafe {
        cmd.pre_exec(move || {
            (libc::dup2(fd, 3) >= 0).then_some(()).ok_or_else(std::io::Error::last_os_error)
        })
    };
    let mut child = cmd.spawn().unwrap();
    drop(theirs);
    let editor = serde_json::json!({
        "proxy_pid": std::process::id(), "sigmask": [], "experimental": [], "features": [],
    });
    let request = serde_json::json!({
        "profile": null, "agent": [AGENT], "cwd": env.dir, "strict": false, "log": "all",
        "bridges": [], "role": { "editor": editor },
    });
    child.stdin.take().unwrap().write_all(request.to_string().as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(stderr(&out).contains("no signal link on fd 4"), "{}", stderr(&out));
    // A FAILED frame: kind, length (u32 BE), payload.
    let mut head = [0; 5];
    ours.read_exact(&mut head).unwrap();
    assert_eq!(head[0], b'F', "{head:?}");
    let mut payload = vec![0; u32::from_be_bytes(head[1..].try_into().unwrap()) as usize];
    ours.read_exact(&mut payload).unwrap();
    let failure: Value = serde_json::from_slice(&payload).unwrap();
    assert_eq!(failure["code"], 2, "{failure}");
    assert!(env.hosts().is_empty() && env.calls().is_empty());
}

/// An editor may hand over a non-blocking stdin: nothing to read yet is not
/// the end of it.
#[test]
fn adr_0002_non_blocking_stdin_is_waited_on() {
    let env = Env::new("ed-nonblock");
    let (reader, mut writer) = std::io::pipe().unwrap();
    let fd = reader.as_raw_fd();
    // SAFETY: fcntl(2) on reader's descriptor, open while reader is; no memory
    // is touched.
    unsafe { libc::fcntl(fd, libc::F_SETFL, libc::fcntl(fd, libc::F_GETFL) | libc::O_NONBLOCK) };
    let mut editor =
        env.brnr(&["acp", "--", AGENT]).stdin(reader).stdout(Stdio::piped()).spawn().unwrap();
    sleep(Duration::from_millis(500));
    writeln!(writer, "{INITIALIZE}").unwrap();
    let mut line = String::new();
    BufReader::new(editor.stdout.take().unwrap()).read_line(&mut line).unwrap();
    let answer: Value = serde_json::from_str(&line).unwrap_or_default();
    assert_eq!(answer["id"], 1, "no answer: {line:?}");
    drop(writer);
    assert!(wait_exit(&mut editor, Duration::from_secs(15)), "acp didn't exit");
}

// ---- ownership ----------------------------------------------------------

/// The editor's `session/load` of `session`, with id `id`.
fn editor_load(env: &Env, id: u64, session: &str) -> String {
    let params = serde_json::json!({
        "sessionId": session,
        "cwd": env.dir.display().to_string(),
        "mcpServers": [],
    });
    serde_json::json!({ "jsonrpc": "2.0", "id": id, "method": "session/load", "params": params })
        .to_string()
}

/// The editor's process, as `brnr ps` has it.
fn editor_process(env: &Env) -> Value {
    env.hosts().into_iter().find(|h| h["proxy_pid"].is_number()).expect("no editor's process")
}

/// An editor's load of a session a headless process holds is answered by
/// brnr, with an error saying how to release it, and the agent never hears
/// of it; once released, it loads (ADR 3).
#[test]
fn adr_0003_an_editors_load_of_a_held_session_is_refused() {
    let env = Env::new("ed-held");
    env.start(&[]);
    let headless = env.pid();
    let (mut editor, mut to_agent, mut from_agent) = editor(&env);
    writeln!(to_agent, "{}", editor_load(&env, 2, "sess-1")).unwrap();
    let error = response(&mut from_agent, 2)["error"].clone();
    let message = error["message"].as_str().unwrap_or_default();
    assert!(message.contains(&format!("sess-1 is running in brnr process {headless}")), "{error}");
    assert!(message.contains("`brnr close sess-1`"), "{error}");
    assert!(env.calls_of("session/load").is_empty());
    env.ok(&["close", "sess-1"]);
    writeln!(to_agent, "{}", editor_load(&env, 3, "sess-1")).unwrap();
    assert!(response(&mut from_agent, 3)["result"].is_object());
    let status: Value = serde_json::from_str(&env.ok(&["status", "sess-1", "--json"])).unwrap();
    assert_eq!(status["held_by"], editor_process(&env)["host_pid"]);
    let _ = editor.kill();
}

/// An editor's session whose lock can't be taken (a directory where its lock
/// file goes) is passed through, as the proxy passes on what it can't be
/// sure of: the editor opens and loads sessions as it would without brnr,
/// its process keeps the transcript, and `status` says the session isn't
/// locked. A headless resume of it is still refused (ADR 50).
#[test]
fn adr_0050_an_editors_session_that_cant_be_locked_is_passed_through() {
    use std::os::unix::fs::DirBuilderExt;
    let env = Env::new("ed-nolock");
    let unlockable = |session: &str| {
        let lock = env.dir.join(format!("run/sessions/{session}.lock"));
        fs::DirBuilder::new().recursive(true).mode(0o700).create(&lock).unwrap();
        lock
    };
    let lock = unlockable("sess-1");
    let (mut editor, mut to_agent, mut from_agent) = open_editor(&env);
    writeln!(to_agent, "{}", editor_prompt(3, "reply from the editor")).unwrap();
    assert!(response(&mut from_agent, 3)["result"].is_object());
    let status: Value = serde_json::from_str(&env.ok(&["status", "sess-1", "--json"])).unwrap();
    let why = status["lock_error"].as_str().unwrap_or_default().to_owned();
    assert!(why.starts_with(&format!("{}: ", lock.display())), "{status}");
    assert_eq!(status["held_by"], Value::Null);
    let text = env.ok(&["status", "sess-1"]);
    assert!(text.contains(&format!("not locked: {why}; nothing stops")), "{text}");
    assert!(env.ok(&["log", "sess-1"]).contains("from the editor"));
    let pid = editor_process(&env)["host_pid"].to_string();
    let err = env.fails(&start_args(&["--resume", "sess-1"]));
    assert!(err.contains(&format!("sess-1 is running in process {pid}")), "{err}");
    // A load reaches the agent, as it would with the lock taken.
    unlockable("old-1");
    writeln!(to_agent, "{}", editor_load(&env, 4, "old-1")).unwrap();
    assert!(response(&mut from_agent, 4)["result"].is_object());
    assert_eq!(env.calls_of("session/load").len(), 1);
    let _ = editor.kill();
}

/// With `shared_sessions`, the editor's load goes through: its process
/// serves the session without the lock, records it in its host log only,
/// and `status` says the session is shared. The headless process stays its
/// owner (ADR 3, ADR 42).
#[test]
fn adr_0042_shared_sessions_let_an_editor_load_a_held_session() {
    let env = Env::new("ed-shared");
    env.write_config("[profiles.default.editor]\nfeatures = [\"shared_sessions\"]\n");
    env.start(&[]);
    let headless = env.pid();
    let (mut editor, mut to_agent, mut from_agent) = editor(&env);
    writeln!(to_agent, "{}", editor_load(&env, 2, "sess-1")).unwrap();
    assert!(response(&mut from_agent, 2)["result"].is_object());
    writeln!(to_agent, "{}", editor_prompt(3, "reply from the editor")).unwrap();
    response(&mut from_agent, 3);
    let shared = editor_process(&env);
    let status: Value = serde_json::from_str(&env.ok(&["status", "sess-1", "--json"])).unwrap();
    assert_eq!(
        (status["pid"].to_string(), status["held_by"].to_string()),
        (headless.clone(), headless)
    );
    assert_eq!(status["shared_by"], serde_json::json!([shared["host_pid"]]));
    let text = env.ok(&["status", "sess-1"]);
    assert!(text.contains(&format!("shared by process {}", shared["host_pid"])), "{text}");
    // Its turn isn't in the session's transcript, but in its host log.
    assert!(!env.ok(&["log", "sess-1"]).contains("from the editor"));
    let host_log = fs::read_to_string(shared["host_log"].as_str().unwrap()).unwrap();
    assert!(host_log.contains("from the editor") && host_log.contains("session-shared"));
    let _ = editor.kill();
}

/// `--take-over` doesn't take a session from an editor whose profile doesn't
/// enable the experimental `close` (ADR 4), and the process started for it
/// goes, having done nothing.
#[test]
fn adr_0004_take_over_from_an_editor_is_refused() {
    let env = Env::new("ed-takeover");
    let (mut editor, _to_agent, _from_agent) = open_editor(&env);
    let pid = env.pid();
    let err = env.fails(&start_args(&["--resume", "sess-1", "--take-over"]));
    let refused = format!(
        "sess-1 is running in process {pid}, an editor's: `close` on an editor's session is \
         experimental"
    );
    assert!(err.contains(&refused), "{err}");
    assert!(env.calls_of("session/close").is_empty());
    assert_eq!(env.hosts().len(), 1, "a second process started");
    let _ = editor.kill();
}

// ---- lines brnr reads --------------------------------------------------

/// A prompt from the editor, with id `id`.
fn editor_prompt(id: u64, text: &str) -> String {
    let prompt = [serde_json::json!({ "type": "text", "text": text })];
    let params = serde_json::json!({ "sessionId": "sess-1", "prompt": prompt });
    serde_json::json!({ "jsonrpc": "2.0", "id": id, "method": "session/prompt", "params": params })
        .to_string()
}

/// The next line the editor gets that has `text` in it.
fn line_with(from_agent: &mut BufReader<ChildStdout>, text: &str) -> String {
    let mut line = String::new();
    loop {
        line.clear();
        assert!(from_agent.read_line(&mut line).unwrap() > 0, "no line with {text}");
        if line.contains(text) {
            return line;
        }
    }
}

/// The editor allowing the fake agent's first permission request.
const ALLOW: &str = r#"{"jsonrpc":"2.0","id":"perm-1","result":{"outcome":{"outcome":"selected","optionId":"allow"}}}"#;

/// A title cut in the middle of an emoji (a lone surrogate) is JSON: the
/// request reaches the editor as the agent wrote it, the host knows the
/// session waits on it (titled with U+FFFD), and the editor's answer
/// reaches the agent.
#[test]
fn adr_0026_a_lone_surrogate_is_read() {
    let env = Env::new("ed-lone");
    let (_editor, mut to_agent, mut from_agent) = open_editor(&env);
    writeln!(to_agent, "{}", editor_prompt(3, "odd surrogate")).unwrap();
    let request = line_with(&mut from_agent, "session/request_permission");
    assert!(request.contains(r#""title": "Edit \ud83d""#), "{request}");
    let pending: Value = serde_json::from_str(&env.ok(&["pending", "sess-1", "--json"])).unwrap();
    assert_eq!(pending[0]["title"], "Edit \u{fffd}", "{pending}");
    writeln!(to_agent, "{ALLOW}").unwrap();
    line_with(&mut from_agent, "end_turn");
}

/// So is nesting deeper than serde_json's 128: the host reads a request
/// 20000 deep, and carries on once it is answered.
#[test]
fn adr_0026_deep_nesting_is_read() {
    let env = Env::new("ed-deep");
    let (_editor, mut to_agent, mut from_agent) = open_editor(&env);
    writeln!(to_agent, "{}", editor_prompt(3, "odd deep 20000")).unwrap();
    let request = line_with(&mut from_agent, "session/request_permission");
    assert!(request.contains(&"[".repeat(20000)), "the request was cut");
    let state = || {
        let status: Value = serde_json::from_str(&env.ok(&["status", "sess-1", "--json"])).unwrap();
        status["state"].clone()
    };
    assert_eq!(state(), "waiting");
    writeln!(to_agent, "{ALLOW}").unwrap();
    line_with(&mut from_agent, "end_turn");
    assert_eq!(state(), "idle");
}

/// A request that isn't JSON at all can't be tracked, but its answer isn't
/// held back: an answer to an id the host doesn't know goes to the agent.
#[test]
fn adr_0026_an_answer_the_host_cant_place_reaches_the_agent() {
    let env = Env::new("ed-garbled");
    let (_editor, mut to_agent, mut from_agent) = open_editor(&env);
    writeln!(to_agent, "{}", editor_prompt(3, "odd garbled")).unwrap();
    let request = line_with(&mut from_agent, "session/request_permission");
    assert!(serde_json::from_str::<Value>(&request).is_err(), "{request}");
    assert_eq!(env.ok(&["pending", "sess-1", "--json"]).trim(), "[]");
    writeln!(to_agent, "{ALLOW}").unwrap();
    line_with(&mut from_agent, "end_turn");
}

/// When a turn is cancelled from outside, the host answers the agent's
/// pending request itself; the editor's late answer to it is dropped, so the
/// agent isn't answered twice, and the editor is told so in the session
/// (ADR 26).
#[test]
fn adr_0026_a_late_answer_to_a_cancelled_request_is_dropped() {
    let env = Env::new("ed-late");
    let (_editor, mut to_agent, mut from_agent) = experimental_editor(&env, &["cancel"]);
    writeln!(to_agent, "{}", editor_prompt(3, "perm edit")).unwrap();
    line_with(&mut from_agent, "session/request_permission");
    env.ok(&["cancel", "sess-1"]);
    line_with(&mut from_agent, "end_turn");
    writeln!(to_agent, "{ALLOW}").unwrap();
    let note = update(&mut from_agent, "tool_call");
    assert_eq!(note["title"], "Already cancelled via brnr", "{note}");
    let text = note["content"][0]["content"]["text"].as_str().unwrap_or_default();
    assert!(text.contains("\"Edit src/lib.rs\" was already cancelled through brnr"), "{text}");
    // Once a later prompt is answered, the host has seen the late answer.
    writeln!(to_agent, "{}", editor_prompt(4, "reply done")).unwrap();
    line_with(&mut from_agent, "end_turn");
    let answers: Vec<Value> = env.calls().into_iter().filter(|c| c["id"] == "perm-1").collect();
    assert_eq!(answers.len(), 1, "{answers:?}");
    assert_eq!(answers[0]["result"]["outcome"]["outcome"], "cancelled");
}

// ---- experimental actions on an editor's session (ADR 4) ---------------

/// `brnr acp` with sess-1 open, its profile's editor part enabling `actions`.
fn experimental_editor(env: &Env, actions: &[&str]) -> (Child, ChildStdin, BufReader<ChildStdout>) {
    let names: Vec<String> = actions.iter().map(|a| format!("{a:?}")).collect();
    env.write_config(&format!(
        "[profiles.default.editor]\nexperimental = [{}]\n",
        names.join(", ")
    ));
    open_editor(env)
}

/// The next message the editor gets that `wanted` picks.
fn message(from_agent: &mut BufReader<ChildStdout>, wanted: impl Fn(&Value) -> bool) -> Value {
    let mut line = String::new();
    loop {
        line.clear();
        assert!(from_agent.read_line(&mut line).unwrap() > 0, "no such message");
        if let Ok(msg) = serde_json::from_str::<Value>(&line)
            && wanted(&msg)
        {
            return msg;
        }
    }
}

/// The next `session/update` of kind `kind` the editor gets: the update.
fn update(from_agent: &mut BufReader<ChildStdout>, kind: &str) -> Value {
    let msg = message(from_agent, |m| m["params"]["update"]["sessionUpdate"] == kind);
    msg["params"]["update"].clone()
}

/// Waits for sess-1 to have no turn running.
fn wait_idle(env: &Env) {
    let idle = || {
        let status: Value = serde_json::from_str(&env.ok(&["status", "sess-1", "--json"])).unwrap();
        status["state"] == "idle"
    };
    assert!(wait_for(Duration::from_secs(5), idle), "sess-1 is still busy");
}

/// Each action on an editor's session is refused unless its profile enables
/// it, saying what to add; observing it is always allowed, and the agent
/// hears of none of it.
#[test]
fn adr_0004_experimental_actions_are_refused_without_opt_in() {
    let env = Env::new("ex-refused");
    let (_editor, mut to_agent, mut from_agent) = open_editor(&env);
    writeln!(to_agent, "{}", editor_prompt(3, "perm edit")).unwrap();
    line_with(&mut from_agent, "session/request_permission");
    let actions: [(&[&str], &str); 10] = [
        (&["send", "sess-1", "hi"], "send"),
        (&["send", "sess-1", "--context", "hi"], "context"),
        (&["queue", "sess-1", "--clear-context"], "context"),
        (&["cancel", "sess-1"], "cancel"),
        (&["approve", "sess-1", "p1"], "approve"),
        (&["deny", "sess-1", "p1"], "approve"),
        (&["mode", "sess-1", "plan"], "settings"),
        (&["model", "sess-1", "large"], "settings"),
        (&["config", "sess-1", "model=large"], "settings"),
        (&["close", "sess-1"], "close"),
    ];
    for (args, action) in actions {
        let err = env.fails(args);
        let says = format!(
            "`{action}` on an editor's session is experimental; enable it with `experimental = \
             [\"{action}\"]` under `[profiles.default.editor]`"
        );
        assert!(err.contains(&says), "{args:?}: {err}");
    }
    assert!(env.fails(&["fork", "sess-1"]).contains("the editor owns this process"));
    let observing: [&[&str]; 9] = [
        &["status", "sess-1"],
        &["pending", "sess-1"],
        &["show", "sess-1", "p1"],
        &["queue", "sess-1"],
        &["mode", "sess-1"],
        &["model", "sess-1"],
        &["config", "sess-1"],
        &["commands", "sess-1"],
        &["log", "sess-1"],
    ];
    for args in observing {
        env.ok(args);
    }
    // show says where it can be answered, and why not here.
    let show = env.ok(&["show", "sess-1", "p1"]);
    assert!(show.contains("p1, session sess-1, waiting in the editor"), "{show}");
    let why = "answer it in the editor (`approve` on an editor's session is experimental";
    assert!(show.contains(why), "{show}");
    for method in ["session/cancel", "session/set_mode", "session/set_config_option"] {
        assert!(env.calls_of(method).is_empty(), "the agent got {method}");
    }
    writeln!(to_agent, "{ALLOW}").unwrap();
    line_with(&mut from_agent, "end_turn");
    let answers: Vec<Value> = env.calls().into_iter().filter(|c| c["id"] == "perm-1").collect();
    assert_eq!(answers.len(), 1, "{answers:?}");
    assert_eq!(env.prompts(), ["perm edit"]);
}

/// In strict mode an editor's session has no experimental actions, whatever
/// its profile enables (ADR 41), and fork stays refused.
#[test]
fn adr_0041_strict_mode_has_no_experimental_actions() {
    let env = Env::new("ex-strict");
    env.write_config(
        "[profiles.default]\nstrict = true\n\n[profiles.default.editor]\nexperimental = \
         [\"send\", \"context\", \"cancel\", \"approve\", \"settings\", \"close\"]\n",
    );
    let (_editor, mut to_agent, mut from_agent) = open_editor(&env);
    writeln!(to_agent, "{}", editor_prompt(3, "perm edit")).unwrap();
    line_with(&mut from_agent, "session/request_permission");
    let actions: [(&[&str], &str); 7] = [
        (&["send", "sess-1", "hi"], "send"),
        (&["send", "sess-1", "--context", "hi"], "context"),
        (&["cancel", "sess-1"], "cancel"),
        (&["approve", "sess-1", "p1"], "approve"),
        (&["deny", "sess-1", "p1"], "approve"),
        (&["mode", "sess-1", "plan"], "settings"),
        (&["close", "sess-1"], "close"),
    ];
    for (args, action) in actions {
        let err = env.fails(args);
        let says =
            format!("{action} on an editor's session is an experimental action, and strict mode");
        assert!(err.contains(&says), "{args:?}: {err}");
    }
    assert!(env.fails(&["fork", "sess-1"]).contains("the editor owns this process"));
    writeln!(to_agent, "{ALLOW}").unwrap();
    line_with(&mut from_agent, "end_turn");
}

/// `send` to an editor's session goes as a prompt only while no turn runs,
/// the editor's or brnr's, and is shown to the editor (ADR 5); it is never
/// held, steered or interrupting. Enabling `send` enables nothing else, and
/// the refusal names the profile.
#[test]
fn adr_0005_send_to_an_editors_session_waits_for_no_turn() {
    let env = Env::new("ex-send");
    env.write_config("[profiles.work.editor]\nexperimental = [\"send\"]\n");
    let (_editor, mut to_agent, mut from_agent) = open_editor_with(&env, &["--profile", "work"]);
    assert!(env.ok(&["send", "sess-1", "reply hi"]).starts_with("delivered"));
    let echo = update(&mut from_agent, "tool_call");
    assert_eq!(
        (&echo["title"], &echo["status"]),
        (&"Message via brnr".into(), &"completed".into())
    );
    wait_idle(&env);
    // The editor's turn.
    writeln!(to_agent, "{}", editor_prompt(3, "hang")).unwrap();
    assert!(wait_for(Duration::from_secs(5), || env.prompts().len() == 2));
    let err = env.fails(&["send", "sess-1", "more"]);
    assert!(err.contains("the editor controls this session's turns; one is running"), "{err}");
    for flag in ["--steer", "--interrupt"] {
        let err = env.fails(&["send", "sess-1", flag, "more"]);
        let says = format!("the editor controls this session's turns; {flag} is its call");
        assert!(err.contains(&says), "{err}");
    }
    let err = env.fails(&["cancel", "sess-1"]);
    assert!(err.contains("`cancel` on an editor's session is experimental"), "{err}");
    assert!(err.contains("under `[profiles.work.editor]`"), "{err}");
    let cancel = r#"{"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":"sess-1"}}"#;
    writeln!(to_agent, "{cancel}").unwrap();
    assert_eq!(response(&mut from_agent, 3)["result"]["stopReason"], "cancelled");
    // brnr's own turn.
    env.ok(&["send", "sess-1", "hang on"]);
    assert!(wait_for(Duration::from_secs(5), || env.prompts().len() == 3));
    let err = env.fails(&["send", "sess-1", "more"]);
    assert!(err.contains("the editor controls this session's turns; one is running"), "{err}");
    writeln!(to_agent, "{cancel}").unwrap();
    wait_idle(&env);
    sleep(Duration::from_millis(300));
    assert_eq!(env.prompts(), ["reply hi", "hang", "hang on"]);
    assert!(env.calls_of("_session/steering").is_empty());
}

/// `context` joins the editor's own next prompt, and is shown to it.
#[test]
fn adr_0005_context_joins_the_editors_next_prompt() {
    let env = Env::new("ex-context");
    let (_editor, mut to_agent, mut from_agent) = experimental_editor(&env, &["context"]);
    let err = env.fails(&["send", "sess-1", "hi"]);
    assert!(err.contains("`send` on an editor's session is experimental"), "{err}");
    env.ok(&["send", "sess-1", "--context", "dropped"]);
    env.ok(&["queue", "sess-1", "--clear-context"]);
    env.ok(&["send", "sess-1", "--context", "kept"]);
    writeln!(to_agent, "{}", editor_prompt(3, "reply ok")).unwrap();
    let echo = update(&mut from_agent, "tool_call");
    assert_eq!(echo["title"], "Context via brnr");
    assert_eq!(echo["content"][0]["content"]["text"], "kept");
    response(&mut from_agent, 3);
    assert_eq!(env.prompts(), ["reply ok\nkept"]);
}

/// `cancel` answers the agent's pending request `cancelled` and withdraws it
/// from the editor (`$/cancel_request`). The editor acknowledging that is
/// dropped, with nothing to tell it.
#[test]
fn adr_0004_cancel_withdraws_the_editors_requests() {
    let env = Env::new("ex-cancel");
    let (_editor, mut to_agent, mut from_agent) = experimental_editor(&env, &["cancel"]);
    writeln!(to_agent, "{}", editor_prompt(3, "perm edit")).unwrap();
    line_with(&mut from_agent, "session/request_permission");
    assert!(env.ok(&["cancel", "sess-1"]).starts_with("cancelling"));
    let withdrawn = message(&mut from_agent, |m| m["method"] == "$/cancel_request");
    assert_eq!(withdrawn["params"], serde_json::json!({ "requestId": "perm-1" }));
    response(&mut from_agent, 3);
    let ack =
        r#"{"jsonrpc":"2.0","id":"perm-1","error":{"code":-32800,"message":"Request cancelled"}}"#;
    writeln!(to_agent, "{ack}").unwrap();
    writeln!(to_agent, "{}", editor_prompt(4, "reply done")).unwrap();
    let mut line = String::new();
    while !serde_json::from_str::<Value>(&line).is_ok_and(|m| m["id"] == 4) {
        line.clear();
        assert!(from_agent.read_line(&mut line).unwrap() > 0, "no answer to 4");
        assert!(!line.contains("Already cancelled"), "told of its acknowledgement: {line}");
    }
    let answers: Vec<Value> = env.calls().into_iter().filter(|c| c["id"] == "perm-1").collect();
    assert_eq!(answers.len(), 1, "{answers:?}");
    assert_eq!(answers[0]["result"]["outcome"]["outcome"], "cancelled");
}

/// `approve` and `deny` answer the agent in the editor's place: the request
/// is withdrawn from the editor, its tool call updated, and the editor told
/// who answered. The editor's own answer after that is dropped, and it is
/// told who answered first.
#[test]
fn adr_0004_approve_answers_in_the_editors_place() {
    let env = Env::new("ex-approve");
    let (_editor, mut to_agent, mut from_agent) = experimental_editor(&env, &["approve"]);
    writeln!(to_agent, "{}", editor_prompt(3, "perm edit")).unwrap();
    line_with(&mut from_agent, "session/request_permission");
    let show = env.ok(&["show", "sess-1", "p1"]);
    let here = "answer: in the editor, or brnr approve sess-1 p1, brnr deny sess-1 p1";
    assert!(show.contains(here), "{show}");
    let json: Value = serde_json::from_str(&env.ok(&["show", "sess-1", "p1", "--json"])).unwrap();
    assert_eq!(
        (&json["answerable"], &json["why_not"]),
        (&Value::Bool(true), &Value::Null),
        "{json}"
    );
    assert_eq!(env.ok(&["approve", "sess-1", "p1"]), "p1 allow\n");
    let withdrawn = message(&mut from_agent, |m| m["method"] == "$/cancel_request");
    assert_eq!(withdrawn["params"]["requestId"], "perm-1");
    let tool = update(&mut from_agent, "tool_call_update");
    assert_eq!((&tool["toolCallId"], &tool["status"]), (&"perm-1".into(), &"in_progress".into()));
    let told = update(&mut from_agent, "tool_call");
    assert_eq!(told["title"], "Approved via brnr", "{told}");
    let text = told["content"][0]["content"]["text"].as_str().unwrap_or_default();
    let says = "\"Edit src/lib.rs\" was approved through brnr's control socket";
    assert!(text.contains(says), "{text}");
    assert_eq!(response(&mut from_agent, 3)["result"]["stopReason"], "end_turn");
    writeln!(to_agent, "{ALLOW}").unwrap();
    let note = update(&mut from_agent, "tool_call");
    assert_eq!(note["title"], "Already approved via brnr", "{note}");
    let text = note["content"][0]["content"]["text"].as_str().unwrap_or_default();
    let says = "\"Edit src/lib.rs\" was already approved through brnr's control socket";
    assert!(text.contains(says), "{text}");
    // A denied tool call has failed.
    writeln!(to_agent, "{}", editor_prompt(4, "perm edit")).unwrap();
    line_with(&mut from_agent, "session/request_permission");
    assert_eq!(env.ok(&["deny", "sess-1", "p2"]), "p2 reject\n");
    let tool = update(&mut from_agent, "tool_call_update");
    assert_eq!((&tool["toolCallId"], &tool["status"]), (&"perm-1".into(), &"failed".into()));
    assert_eq!(update(&mut from_agent, "tool_call")["title"], "Denied via brnr");
    response(&mut from_agent, 4);
    let answers: Vec<Value> = (env.calls().into_iter())
        .filter(|c| c["id"] == "perm-1" && c.get("method").is_none())
        .map(|c| c["result"]["outcome"]["optionId"].clone())
        .collect();
    assert_eq!(answers, ["allow", "reject"]);
}

/// The agent answers a change of mode or config option only to the host,
/// which asked; the editor is sent the update itself (ADR 28).
#[test]
fn adr_0004_settings_are_told_to_the_editor() {
    let env = Env::new("ex-settings").agent("QUIET_MODE", "1");
    let (_editor, _to_agent, mut from_agent) = experimental_editor(&env, &["settings"]);
    assert_eq!(env.ok(&["mode", "sess-1", "plan"]), "mode plan\n");
    assert_eq!(update(&mut from_agent, "current_mode_update")["currentModeId"], "plan");
    env.ok(&["model", "sess-1", "large"]);
    let config = update(&mut from_agent, "config_option_update");
    assert_eq!(config["configOptions"][0]["currentValue"], "large", "{config}");
    env.ok(&["config", "sess-1", "model=small"]);
    let config = update(&mut from_agent, "config_option_update");
    assert_eq!(config["configOptions"][0]["currentValue"], "small", "{config}");
}

/// An editor that negotiates boolean config options gets them from the
/// agent, and `config` sets one as ACP has it: `type: "boolean"` and a JSON
/// boolean. A value that isn't `true` or `false` fails before it reaches the
/// agent; a select option is still set by its value id.
#[test]
fn adr_0028_a_boolean_option_is_set_as_a_boolean() {
    let env = Env::new("ex-boolean");
    env.write_config("[profiles.default.editor]\nexperimental = [\"settings\"]\n");
    let args = ["acp", "--", AGENT];
    let mut editor = env.brnr(&args).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let mut to_agent = editor.stdin.take().unwrap();
    let mut from_agent = BufReader::new(editor.stdout.take().unwrap());
    let initialize = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{"session":{"configOptions":{"boolean":{}}}}}}"#;
    writeln!(to_agent, "{initialize}").unwrap();
    response(&mut from_agent, 1);
    let new = format!(
        r#"{{"jsonrpc":"2.0","id":2,"method":"session/new","params":{{"cwd":{:?},"mcpServers":[]}}}}"#,
        env.dir.display().to_string()
    );
    writeln!(to_agent, "{new}").unwrap();
    let options = response(&mut from_agent, 2)["result"]["configOptions"].clone();
    assert_eq!(options[1]["type"], "boolean", "{options}");
    assert!(env.ok(&["config", "sess-1"]).contains("fast    false"));

    assert_eq!(env.ok(&["config", "sess-1", "fast=true"]), "fast=true\n");
    let set = env.calls_of("session/set_config_option")[0]["params"].clone();
    let typed = serde_json::json!({ "sessionId": "sess-1", "configId": "fast", "type": "boolean", "value": true });
    assert_eq!(set, typed);
    let config = update(&mut from_agent, "config_option_update");
    assert_eq!(config["configOptions"][1]["currentValue"], true, "{config}");
    assert!(env.ok(&["config", "sess-1"]).contains("fast    true"));

    let err = env.fails(&["config", "sess-1", "fast=yes"]);
    assert!(err.contains("fast is a boolean option: true or false, not yes"), "{err}");
    assert_eq!(env.calls_of("session/set_config_option").len(), 1, "yes reached the agent");

    env.ok(&["config", "sess-1", "model=large"]);
    let set = env.calls_of("session/set_config_option")[1]["params"].clone();
    let id = serde_json::json!({ "sessionId": "sess-1", "configId": "model", "value": "large" });
    assert_eq!(set, id);
    let _ = editor.kill();
    let _ = editor.wait();
}

/// `close` cancels the editor's turn, tells it in the session, and closes the
/// session; the editor's requests for it are answered by brnr after that,
/// until it loads it again.
#[test]
fn adr_0004_close_tells_the_editor() {
    let env = Env::new("ex-close");
    let (_editor, mut to_agent, mut from_agent) = experimental_editor(&env, &["close"]);
    writeln!(to_agent, "{}", editor_prompt(3, "hang")).unwrap();
    assert!(wait_for(Duration::from_secs(5), || env.prompts().len() == 1));
    env.ok(&["close", "sess-1"]);
    assert_eq!(update(&mut from_agent, "tool_call")["title"], "Session closed via brnr");
    assert_eq!(response(&mut from_agent, 3)["result"]["stopReason"], "cancelled");
    assert_eq!(env.calls_of("session/close").len(), 1);
    let log = env.ok(&["log", "sess-1", "--json", "--events", "session_closed"]);
    let closed: Value =
        serde_json::from_str(log.lines().next().expect("no session_closed")).unwrap();
    assert_eq!(closed["by"], "close");
    writeln!(to_agent, "{}", editor_prompt(4, "reply again")).unwrap();
    let error = response(&mut from_agent, 4)["error"].clone();
    let says = "session sess-1 was closed via brnr; load it again to continue it here";
    assert!(error["message"].as_str().unwrap_or_default().contains(says), "{error}");
    assert_eq!(env.prompts(), ["hang"]);
    writeln!(to_agent, "{}", editor_load(&env, 5, "sess-1")).unwrap();
    assert!(response(&mut from_agent, 5)["result"].is_object());
    writeln!(to_agent, "{}", editor_prompt(6, "reply back")).unwrap();
    assert_eq!(response(&mut from_agent, 6)["result"]["stopReason"], "end_turn");
}

/// With `close` enabled, `--take-over` takes the session from an editor: the
/// editor is told in the session which process has it, and its requests for
/// it say where it continues.
#[test]
fn adr_0004_take_over_from_an_editor() {
    let env = Env::new("ex-takeover");
    let (_editor, mut to_agent, mut from_agent) = experimental_editor(&env, &["close"]);
    let editors = env.pid();
    writeln!(to_agent, "{}", editor_prompt(3, "hang")).unwrap();
    assert!(wait_for(Duration::from_secs(5), || env.prompts().len() == 1));
    let out = env.run(&start_args(&["--resume", "sess-1", "--take-over"]));
    assert!(out.status.success(), "{}", stderr(&out));
    let said = format!("closed sess-1 in process {editors}");
    assert!(stderr(&out).contains(&said), "{}", stderr(&out));
    let status: Value = serde_json::from_str(&env.ok(&["status", "sess-1", "--json"])).unwrap();
    let pid = status["pid"].to_string();
    assert_ne!(pid, editors);
    let note = update(&mut from_agent, "tool_call");
    assert_eq!(note["title"], format!("Session taken over by brnr (process {pid})"));
    assert_eq!(response(&mut from_agent, 3)["result"]["stopReason"], "cancelled");
    writeln!(to_agent, "{}", editor_prompt(4, "reply here")).unwrap();
    let error = response(&mut from_agent, 4)["error"].clone();
    let says = format!(
        "session sess-1 was taken over by brnr (process {pid}); it continues in brnr process {pid}"
    );
    assert!(error["message"].as_str().unwrap_or_default().contains(&says), "{error}");
    assert_eq!(env.prompts(), ["hang"]);
}

/// The editor's own steer goes to the agent untouched, and once the agent has
/// taken it into the turn it is a `user_message` of that turn, by the editor.
#[test]
fn adr_0004_the_editors_own_steer_is_recorded() {
    let env = Env::new("ex-steer");
    let (_editor, mut to_agent, mut from_agent) = open_editor(&env);
    writeln!(to_agent, "{}", editor_prompt(3, "hang")).unwrap();
    assert!(wait_for(Duration::from_secs(5), || env.prompts().len() == 1));
    let prompt = [serde_json::json!({ "type": "text", "text": "reply steered" })];
    let params = serde_json::json!({ "sessionId": "sess-1", "prompt": prompt });
    let method = "_session/steering";
    let steer =
        serde_json::json!({ "jsonrpc": "2.0", "id": 4, "method": method, "params": params });
    writeln!(to_agent, "{steer}").unwrap();
    assert_eq!(response(&mut from_agent, 4)["result"]["outcome"], "injected");
    assert_eq!(response(&mut from_agent, 3)["result"]["stopReason"], "end_turn");
    assert_eq!(env.calls_of("_session/steering"), [steer]);
    let log = env.ok(&["log", "sess-1", "--json", "--events", "user_message"]);
    let said: Vec<Value> = log.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(said.len(), 2, "{said:?}");
    assert_eq!((&said[1]["by"], &said[1]["text"]), (&"editor".into(), &"reply steered".into()));
    assert_eq!(said[1]["prompt"], said[0]["prompt"]);
}
