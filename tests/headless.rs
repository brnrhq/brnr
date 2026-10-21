//! Headless sessions (`brnr session new`) and editors' (`brnr acp`) against a
//! fake ACP agent (fake_agent.py). Each test gets its own runtime, state and
//! config directories, and kills whatever it leaves running.

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
use std::time::{Duration, Instant};

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
    assert_eq!(record["event"]["event"], "process_started", "{record}");
    record["event"].clone()
}

/// A `brnr session new` that goes away before the start commits stops the
/// process at once, whatever it is doing (here, waiting 30 s for the
/// session), and the agent never gets the prompt.
#[test]
fn adr_0003_abandoned_start_sends_no_prompt() {
    let env = Env::new("abandon").agent("NEW_DELAY", "30");
    let mut start = env
        .brnr(&new_args(&["--prompt", "run the migration"]))
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    assert!(wait_for(Duration::from_secs(5), || !env.hosts().is_empty()), "no host");
    start.kill().unwrap();
    start.wait().unwrap();

    assert!(wait_for(Duration::from_secs(15), || env.hosts().is_empty()), "host kept running");
    assert!(env.prompts().is_empty(), "agent got {:?}", env.prompts());
    let log = fs::read_dir(env.dir.join("home/hosts")).unwrap().next().unwrap().unwrap().path();
    assert!(fs::read_to_string(log).unwrap().contains("start_abandoned"));
}

/// A `brnr session new` that goes once the process has decided to report
/// ready, but before the report is written, hasn't been told the session: the
/// write fails, the start is abandoned as before the commit, and the agent
/// never gets the prompt (ADR 3.4). (`BRNR_TEST_READY=hold` holds the commit
/// there until brnr session new has gone.)
#[test]
fn adr_0003_start_gone_as_ready_is_written_sends_no_prompt() {
    let env = Env::new("abandon-ready");
    let mut start = env
        .brnr(&new_args(&["--prompt", "run the migration"]))
        .env("BRNR_TEST_READY", "hold")
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let held = || host_logs(&env).contains("test_ready_held");
    assert!(wait_for(Duration::from_secs(10), held), "never held: {}", host_logs(&env));
    start.kill().unwrap();
    start.wait().unwrap();

    assert!(wait_for(Duration::from_secs(15), || env.hosts().is_empty()), "host kept running");
    assert!(env.prompts().is_empty(), "agent got {:?}", env.prompts());
    assert!(host_logs(&env).contains("start_abandoned"), "{}", host_logs(&env));
}

/// A `brnr session new --wait` that goes once it has the ready report leaves
/// the session running, its prompt sent: the start committed (ADR 3.4, P14).
#[test]
fn adr_0003_start_gone_after_ready_leaves_the_session_running() {
    let env = Env::new("after-ready");
    let mut start = env
        .brnr(&new_args(&["--wait", "--prompt", "hang on"]))
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
    assert!(!host_logs(&env).contains("start_abandoned"), "{}", host_logs(&env));
    env.stop();
}

/// Everything the process does is in the one request it was started with,
/// which its `started` record holds; its argv is only `brnr host`.
#[test]
fn adr_0003_start_hands_over_one_request() {
    let env = Env::new("request");
    env.write_config(
        "[profiles.default]\nstrict = true\n\n[profiles.default.headless]\nmode = \"plan\"\n",
    );
    env.start(&["--option", "model=large", "--stop-when-idle", "60", "--prompt", "hello"]);
    let started = started_record(&env);
    let request = &started["start_request"];
    assert_eq!(request["agent"][0], AGENT, "{request}");
    assert_eq!(request["strict"], true);
    assert_eq!(request["log"], "all");
    let headless = &request["role"]["headless"];
    // The flags' settings and the profile's, apart (ADR 6.1).
    assert_eq!(headless["defaults"]["mode"], "plan", "{request}");
    assert_eq!(headless["settings"]["options"]["model"], "large");
    assert!(headless["settings"]["mode"].is_null() && headless["defaults"]["options"] == json!({}));
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
fn adr_0003_host_is_not_run_by_hand() {
    let env = Env::new("byhand");
    let err = env.fails(&["host", "--prompt", "hi", "--", AGENT]);
    assert!(err.contains("not by hand"), "{err}");
    let err = env.fails(&["host"]);
    assert!(err.contains("brnr session new --foreground"), "{err}");
    assert!(env.hosts().is_empty());
}

/// The process reads its request to EOF before doing anything: one cut short
/// (its starter died writing it) or of the wrong shape is refused, and the
/// agent never started.
#[test]
fn adr_0003_a_bad_request_is_refused() {
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

/// When the session doesn't open in time, `brnr session new` says so and the
/// host stops instead of carrying on unseen.
#[test]
fn adr_0003_start_timeout_stops_the_host() {
    let env = Env::new("timeout").agent("NEW_DELAY", "4");
    let out = env
        .brnr(&new_args(&["--prompt", "run the migration"]))
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
fn adr_0003_empty_prompt_is_refused() {
    let env = Env::new("empty");
    let out = env.run(&new_args(&["--prompt", " "]));
    assert!(!out.status.success());
    assert!(stderr(&out).contains("empty"), "{}", stderr(&out));
    assert!(env.hosts().is_empty());
}

/// The prompt reaches the agent without ever being on a command line, where
/// `ps` shows it and Linux caps a single argument at 128 KiB. Nor is the
/// agent's command: `pkill -f <adapter>` would take the process with it.
#[test]
fn adr_0003_prompt_is_not_on_the_command_line() {
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
fn adr_0003_large_prompt_from_stdin() {
    let env = Env::new("bigprompt");
    let prompt = "x".repeat(300_000);
    let out = env.run_with_stdin(&new_args(&["--prompt", "-"]), prompt.as_bytes());
    assert!(out.status.success(), "session new failed: {}", stderr(&out));
    assert!(wait_for(Duration::from_secs(5), || !env.prompts().is_empty()));
    assert_eq!(env.prompts()[0].len(), prompt.len());
    let started = started_record(&env);
    assert_eq!(
        started["start_request"]["role"]["headless"]["prompt"]["text"].as_str().unwrap().len(),
        300_000
    );
}

/// An Env whose agent writes how it found SIGCHLD (see `sigchld_found`).
fn sigchld_env(name: &str) -> Env {
    let env = Env::new(name);
    let log = env.dir.join("sigchld");
    env.agent("SIGCHLD_LOG", log.to_str().unwrap())
}

/// How the agent found SIGCHLD: `ignored` or `default`.
fn sigchld_found(env: &Env) -> String {
    let log = env.dir.join("sigchld");
    assert!(wait_for(Duration::from_secs(10), || log.exists()), "the agent didn't start");
    assert!(wait_for(Duration::from_secs(5), || fs::metadata(&log).unwrap().len() > 0));
    fs::read_to_string(log).unwrap()
}

/// `brnr session new` started with SIGCHLD ignored, as some supervisors leave
/// it, starts its process, which waits for its agent all the same: the
/// session stops when asked. The agent finds SIGCHLD as brnr did.
#[test]
fn adr_0003_a_start_with_sigchld_ignored() {
    let env = sigchld_env("chld-new");
    let out =
        with_sigchld_ignored(&mut env.brnr(&new_args(&["--prompt", "reply hi"]))).output().unwrap();
    assert!(out.status.success(), "session new failed: {}", stderr(&out));
    assert_eq!(sigchld_found(&env), "ignored");
    env.stop();
    assert!(wait_for(Duration::from_secs(10), || env.hosts().is_empty()), "didn't stop");
}

/// `brnr session new --foreground` with SIGCHLD ignored sees its process
/// through: the process sees its agent exit, and exits as it did.
#[test]
fn adr_0003_foreground_with_sigchld_ignored() {
    let env = sigchld_env("chld-fg");
    let args =
        new_args(&["--foreground", "--quiet", "--stop-when-idle", "0", "--prompt", "reply hi"]);
    // stderr to a file: a process left running would hold a pipe open.
    let err = env.dir.join("stderr");
    let mut fg = with_sigchld_ignored(&mut env.brnr(&args))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(fs::File::create(&err).unwrap())
        .spawn()
        .unwrap();
    let exited = wait_exit(&mut fg, Duration::from_secs(20));
    if !exited {
        let _ = fg.kill();
    }
    let status = fg.wait().unwrap();
    let err = fs::read_to_string(err).unwrap();
    assert!(exited, "didn't exit: {err}");
    assert!(status.success(), "{status}: {err}");
    assert!(env.hosts().is_empty(), "its process is still running: {err}");
    assert_eq!(sigchld_found(&env), "ignored");
}

/// `brnr acp` run by an editor that ignores SIGCHLD starts its process, and
/// the agent finds SIGCHLD ignored, as it would run by the editor directly
/// (P1). When the editor's stdin ends, the agent's exit is seen and passed
/// on.
#[test]
fn adr_0003_acp_with_sigchld_ignored() {
    let env = sigchld_env("chld-acp");
    let err = env.dir.join("stderr");
    let mut editor = with_sigchld_ignored(&mut env.brnr(&["acp", "--", AGENT]))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(fs::File::create(&err).unwrap())
        .spawn()
        .unwrap();
    let mut to_agent = editor.stdin.take().unwrap();
    let mut from_agent = BufReader::new(editor.stdout.take().unwrap());
    writeln!(to_agent, "{INITIALIZE}").unwrap();
    let mut line = String::new();
    let answered = from_agent.read_line(&mut line).unwrap() > 0;
    assert!(answered, "no answer to initialize: {}", fs::read_to_string(&err).unwrap());
    assert_eq!(sigchld_found(&env), "ignored");
    drop(to_agent);
    let exited = wait_exit(&mut editor, Duration::from_secs(10));
    if !exited {
        let _ = editor.kill();
    }
    let status = editor.wait().unwrap();
    let err = fs::read_to_string(err).unwrap();
    assert!(exited, "didn't exit with the agent: {err}");
    assert!(status.success(), "{status}: {err}");
}

// ---- stopping ----------------------------------------------------------

/// An agent that stops reading its stdin must not wedge the host: it still
/// answers, and `brnr process stop` still ends it.
#[test]
fn stalled_agent_can_still_be_stopped() {
    let env = Env::new("stall").agent("STALL", "1");
    env.start(&[]);
    let host = env.host_pid();
    let big = vec![b'x'; 1 << 20];
    let out = env.run_with_stdin(&["prompt", "send", "sess-1", "-"], &big);
    assert!(out.status.success(), "send: {}", stderr(&out));

    let out = env.run(&["session", "status", "sess-1"]);
    assert!(out.status.success(), "status: {}", stderr(&out));
    let out = env.run(&["process", "stop", &host.to_string()]);
    assert!(out.status.success(), "stop: {}", stderr(&out));
    assert!(wait_for(Duration::from_secs(15), || !alive(host)), "host still running");
}

/// `brnr process stop` escalates to the agent's whole process group.
#[test]
fn stop_kills_the_agents_children() {
    let env = Env::new("stubborn").agent("STUBBORN", "all");
    env.start(&[]);
    let (host, child) = (env.host_pid(), env.child_pid());
    assert!(env.run(&["process", "stop", &env.pid()]).status.success());
    assert!(wait_for(Duration::from_secs(15), || !alive(host)), "host still running");
    assert!(wait_for(Duration::from_secs(2), || !alive(child)), "agent's child survived");
}

/// A child that outlives an agent which stopped cleanly is killed too.
#[test]
fn stop_kills_children_left_behind() {
    let env = Env::new("orphan").agent("STUBBORN", "child");
    env.start(&[]);
    let (host, child) = (env.host_pid(), env.child_pid());
    assert!(env.run(&["process", "stop", &env.pid()]).status.success());
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
    assert!(env.run(&["process", "stop", &env.pid()]).status.success());
    assert!(wait_for(Duration::from_secs(20), || !alive(host)), "host still running");
}

/// A start that fails after the session opened (setting its model, which
/// the agent refuses) fails in the foreground too: it says why, and doesn't
/// exit 0.
#[test]
fn adr_0003_foreground_start_failure_is_reported() {
    let env = Env::new("fg-mode");
    let out = env.run(&new_args(&["--foreground", "--model", "huge"]));
    assert!(!out.status.success(), "exited 0: {}", stderr(&out));
    let says = "setting model huge failed: bad option model=huge";
    assert!(stderr(&out).contains(says), "{}", stderr(&out));
}

/// Closing the last session ends a foreground process as it should: no
/// claim that the session never started.
#[test]
fn adr_0003_foreground_close_of_the_last_session() {
    let env = Env::new("fg-close");
    let mut fg = env
        .brnr(&new_args(&["--foreground", "--quiet"]))
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    assert!(
        wait_for(Duration::from_secs(10), || env
            .ok(&["session", "list", "--include", "active"])
            .contains("sess-1")),
        "no session"
    );
    env.ok(&["session", "close", "sess-1"]);
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

/// What is left in the runtime dir, and the session locks left in the state
/// dir (ADR 3.9); the locks' directory itself stays.
fn runtime_files(env: &Env) -> Vec<String> {
    let names = |dir: &Path| -> Vec<String> {
        let entries = fs::read_dir(dir).into_iter().flatten();
        entries.map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect()
    };
    let mut files = names(&env.dir.join("run"));
    let locks = names(&env.dir.join("home/sessions"));
    files.extend(locks.into_iter().map(|n| format!("sessions/{n}")));
    files
}

/// `brnr session new --foreground … | head -1`: the display stops, with a note
/// in the host log, and the session carries on. Stopped, it records `exited`
/// and leaves nothing behind in the runtime dir.
#[test]
fn adr_0003_foreground_outlives_its_stdout() {
    let env = Env::new("fg-head");
    let (reader, writer) = std::io::pipe().unwrap();
    let mut fg = env
        .brnr(&new_args(&["--foreground", "--prompt", "reply hi"]))
        .stdout(writer)
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut first = String::new();
    BufReader::new(reader).read_line(&mut first).unwrap();
    assert!(first.contains("user: reply hi"), "{first}");
    // Something more to show, on a stdout nobody reads any more.
    assert_eq!(env.ok(&["prompt", "send", "sess-1", "--wait", "reply again"]), "again\n");
    let stopped = || host_logs(&env).contains(r#""event":"display_stopped""#);
    assert!(wait_for(Duration::from_secs(5), stopped), "no note of the display stopping");
    assert!(env.ok(&["session", "status", "sess-1"]).contains("idle"));
    env.stop();
    assert!(wait_exit(&mut fg, Duration::from_secs(15)), "didn't stop");
    let mut err = String::new();
    fg.stderr.take().unwrap().read_to_string(&mut err).unwrap();
    assert!(fg.wait().unwrap().success(), "{err}");
    assert!(err.contains("brnr: agent exited"), "{err}");
    let exited = env.ok(&["event", "log", "sess-1", "--json", "--events", "process_exited"]);
    assert!(exited.contains(r#""event":"process_exited""#), "{exited}");
    assert!(runtime_files(&env).is_empty(), "{:?}", runtime_files(&env));
}

/// A foreground reader that falls behind is skipped past and told how many
/// events it missed; the session isn't held up, and its transcript has them
/// all.
#[test]
fn adr_0003_slow_foreground_reader_is_told_what_it_missed() {
    let env = Env::new("fg-slow");
    let args = ["--foreground", "--stop-when-idle", "0", "--prompt", "many 80000"];
    let mut fg =
        env.brnr(&new_args(&args)).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
    // The session runs to its end while nothing is read.
    let ended =
        || host_logs(&env).contains(r#""event":"process_exited""#) && env.hosts().is_empty();
    assert!(wait_for(Duration::from_secs(30), ended), "the session was held up");
    let mut out = String::new();
    fg.stdout.take().unwrap().read_to_string(&mut out).unwrap();
    assert!(fg.wait().unwrap().success());
    let not_shown = |l: &str| l.strip_prefix("… ")?.strip_suffix(" events not shown")?.parse().ok();
    let skipped: Vec<u64> = out.lines().filter_map(not_shown).collect();
    assert!(!skipped.is_empty(), "no line about skipped events");
    let shown = out.lines().filter(|l| not_shown(l).is_none()).count() as u64;
    let logged = env.ok(&["event", "log", "sess-1"]).lines().count() as u64;
    assert!(logged > 80000, "{logged}");
    assert_eq!(shown + skipped.iter().sum::<u64>(), logged);
}

/// In the foreground the agent's stderr goes to stderr as it comes,
/// unchanged; stdout is the events. It is in the host log too.
#[test]
fn adr_0003_foreground_passes_the_agents_stderr() {
    let env = Env::new("fg-stderr").agent("STDERR", "[session/create] phase=ready \x1b[1m");
    let args = ["--foreground", "--stop-when-idle", "0", "--prompt", "reply hi"];
    let out = env.run(&new_args(&args));
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stderr(&out).contains("[session/create] phase=ready \x1b[1m\n"), "{}", stderr(&out));
    assert!(!String::from_utf8_lossy(&out.stdout).contains("phase=ready"));
    let logged = r#""dir":"agent_stderr","raw":"[session/create] phase=ready"#;
    assert!(host_logs(&env).contains(logged));
}

/// A start that fails ends its error with the agent's last lines on stderr:
/// brnr session new's, and the foreground's after the agent's stderr itself.
#[test]
fn adr_0003_failed_start_shows_the_agents_stderr() {
    let env = Env::new("errtail").agent("STDERR", "Error: claude CLI not found").agent("EXIT", "1");
    let err = env.fails(&new_args(&["--prompt", "hi"]));
    let want = "the agent exited before the session started. \
                The agent's last lines on stderr:\n  Error: claude CLI not found\n";
    assert!(err.ends_with(want), "{err}");
    let out = env.run(&new_args(&["--foreground", "--prompt", "hi"]));
    assert!(!out.status.success());
    assert_eq!(stderr(&out).matches("Error: claude CLI not found").count(), 2, "{}", stderr(&out));
    assert!(stderr(&out).contains(want), "{}", stderr(&out));
}

/// A start that times out while the agent waits at a prompt of its own on
/// stderr, no newline after it, ends its error with that prompt.
/// (`BRNR_TEST_START_TIMER=stderr` holds the timeout until the prompt has
/// come, so the agent can write it after the timeout, as a slow one would.)
#[test]
fn adr_0002_a_timed_out_start_shows_the_line_stderr_is_in() {
    let env = Env::new("errpart");
    let agent = env.dir.join("agent.sh");
    script(&agent, "#!/bin/sh\nsleep 2\nprintf 'login required: ' >&2\nexec cat >/dev/null\n");
    let out = (env.brnr(&["session", "new", "--prompt", "hi", "--", agent.to_str().unwrap()]))
        .env("BRNR_START_TIMEOUT", "1")
        .env("BRNR_TEST_START_TIMER", "stderr")
        .output()
        .unwrap();
    assert!(!out.status.success());
    let want = "timed out waiting for the session. \
                The agent's last lines on stderr:\n  login required: \n";
    assert!(stderr(&out).ends_with(want), "{}", stderr(&out));
}

/// Answering a permission request while the agent is being stopped can't
/// reach it, so it fails rather than claiming success.
#[test]
fn adr_0004_allow_during_stop_fails() {
    let env = Env::new("stopperm").agent("PERMISSION", "1").agent("STUBBORN", "all");
    env.start(&["--prompt", "edit it"]);
    let waiting = || {
        String::from_utf8_lossy(&env.run(&["permission", "requests", "sess-1"]).stdout)
            .contains("p1")
    };
    assert!(wait_for(Duration::from_secs(5), waiting), "no permission request");
    assert!(env.run(&["process", "stop", &env.pid()]).status.success());
    let out = env.run(&["permission", "allow", "sess-1", "p1"]);
    assert!(!out.status.success(), "allow succeeded during stop");
    assert!(stderr(&out).contains("no longer"), "{}", stderr(&out));
}

/// Messages still held when the agent exits are dropped with an event each,
/// before `exited`, not silently.
#[test]
fn adr_0005_held_messages_are_reported_on_exit() {
    let env = Env::new("held");
    env.start(&["--prompt", "hang on"]);
    let mut watch = env
        .brnr(&["event", "watch", "sess-1", "--json", "--events", "message_dropped,process_exited"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    sleep(Duration::from_millis(300));
    let out = env.run(&["prompt", "send", "sess-1", "later"]);
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "held (message m2)");
    assert!(env.run(&["process", "stop", &env.pid()]).status.success());

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
    assert_eq!(dropped["reason"], "exit");
    assert_eq!(lines[1]["event"], "process_exited");
    assert!(lines[1].get("undelivered").is_none(), "{}", lines[1]);
}

// ---- a process's death -------------------------------------------------

/// A panic ends the process, but not without a word (ADR 3.7), on the event
/// loop or on another thread: the panic is in the host log, with what Rust
/// said of it on stderr; `exited`, with the reason, is in the session's
/// transcript and reaches the peers; the agent goes; and the process's
/// files are removed. (`BRNR_TEST_PANIC` lets a request ask for the panic.)
#[test]
fn adr_0003_a_panic_is_recorded() {
    for on in ["loop", "thread"] {
        let env = Env::new(&format!("panic-{on}"));
        let args = new_args(&["--prompt", "hang on"]);
        let out = env.brnr(&args).env("BRNR_TEST_PANIC", "1").output().unwrap();
        assert!(out.status.success(), "{}", stderr(&out));
        // Held behind the hanging turn, so dropped when the process dies.
        assert!(env.ok(&["prompt", "send", "sess-1", "later"]).contains("held"));
        let host = env.hosts().remove(0);
        let pid = |key: &str| host[key].as_i64().unwrap() as i32;
        let (host_pid, agent_pid) = (pid("host_pid"), pid("agent_pid"));
        // A peer, subscribed to `exited`, asks for the panic.
        let mut conn = UnixStream::connect(host["socket"].as_str().unwrap()).unwrap();
        conn.set_read_timeout(Some(Duration::from_secs(15))).unwrap();
        let mut from_host = BufReader::new(conn.try_clone().unwrap()).lines();
        writeln!(conn, r#"{{"cmd":"subscribe","events":["process_exited"]}}"#).unwrap();
        from_host.next().unwrap().unwrap();
        writeln!(conn, r#"{{"cmd":"panic","on":"{on}"}}"#).unwrap();
        let exited: Value = from_host
            .map(|l| serde_json::from_str(&l.unwrap()).unwrap())
            .find(|m: &Value| m["event"] == "process_exited")
            .expect("no exited");

        assert!(wait_for(Duration::from_secs(15), || !alive(host_pid)), "{on}: still running");
        assert!(wait_for(Duration::from_secs(5), || !alive(agent_pid)), "{on}: the agent lives on");
        let reason = exited["error"].as_str().unwrap_or_default();
        assert!(reason.starts_with("brnr panicked at src/host/"), "{on}: {exited}");
        assert!(reason.ends_with(": a test asked for it"), "{on}: {exited}");
        let logged = env.ok(&["event", "log", "sess-1", "--json", "--events", "process_exited"]);
        assert!(logged.contains(reason), "{on}: {logged}");
        let dropped = env.ok(&["event", "log", "sess-1", "--json", "--events", "message_dropped"]);
        assert!(dropped.contains(r#""reason":"exit""#), "{on}: {dropped}");
        let shown = env.ok(&["event", "log", "sess-1", "--events", "process_exited"]);
        assert!(
            shown.contains(&format!("agent exited (status unknown); {reason}")),
            "{on}: {shown}"
        );
        let log = host_logs(&env);
        assert!(log.contains(r#""event":{"event":"process_panicked""#), "{on}: {log}");
        let said = log.contains(r#""event":"process_stderr""#) && log.contains("panicked at");
        assert!(said, "{on}: {log}");
        assert!(runtime_files(&env).is_empty(), "{on}: {:?}", runtime_files(&env));
    }
}

/// A panic as the start is under way, once the agent, the log and a bridge
/// are there, is recorded as one on the event loop is (ADR 3.7): `panic` and
/// `exited` in the host log, so doctor finds no unrecorded death, `exited`
/// for the bridge, the agent killed and the runtime files removed. brnr
/// start fails saying why, and an editor's brnr acp too, with 101.
/// (`BRNR_TEST_PANIC=start` asks for the panic.)
#[test]
fn adr_0003_a_panic_while_starting_is_recorded() {
    let got = |env: &Env| fs::read_to_string(env.dir.join("bridge-events")).unwrap_or_default();
    for editor in [false, true] {
        let env = Env::new(if editor { "panic-start-ed" } else { "panic-start" });
        let script = format!("exec cat > '{}'", env.dir.join("bridge-events").display());
        env.write_config(&format!(
            "[[profiles.default.bridges]]\ncommand = [\"sh\", \"-c\", {script:?}]\n"
        ));
        let args = if editor { vec!["acp", "--", AGENT] } else { new_args(&["--prompt", "hi"]) };
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
        // Then the link to report it (ADR 11.5), once.
        let command = if editor { "acp" } else { "session%20new" };
        let link = format!("&what=%60brnr%20{command}%60%20panicked");
        assert_eq!(err.matches(&link).count(), 1, "{editor}: {err}");
        assert!(err.trim_end().lines().last().unwrap().starts_with(ISSUE_LINK), "{err}");

        let agent = started_record(&env)["info"]["agent_pid"].as_i64().unwrap() as i32;
        assert!(wait_for(Duration::from_secs(5), || !alive(agent)), "{editor}: the agent lives on");
        let log = host_logs(&env);
        assert!(log.contains(r#""event":{"event":"process_panicked""#), "{editor}: {log}");
        let records = log.lines().map(|l| serde_json::from_str::<Value>(l).unwrap());
        let exited = records.into_iter().find(|r| r["event"]["event"] == "process_exited");
        let reason = exited.as_ref().and_then(|r| r["event"]["error"].as_str());
        assert!(reason.is_some_and(|r| r.ends_with(": a test asked for it")), "{editor}: {log}");
        let told = || got(&env).contains(r#""event":"process_exited""#);
        assert!(wait_for(Duration::from_secs(5), told), "{editor}: the bridge: {}", got(&env));
        assert!(runtime_files(&env).is_empty(), "{editor}: {:?}", runtime_files(&env));
        assert!(env.prompts().is_empty(), "{editor}: the prompt went");
        let doctor = String::from_utf8_lossy(&env.run(&["doctor"]).stdout).into_owned();
        assert!(doctor.contains("no process died without recording it"), "{editor}: {doctor}");
    }
}

/// A panic prints a link to a pre-filled bug report where its user sees it
/// (ADR 11.5): the CLI's own, on its stderr, after Rust's message; a
/// foreground process's, on its stderr, once. `BRNR_TEST_PANIC=cli` makes
/// the CLI panic.
#[test]
fn adr_0011_a_panic_prints_a_link_to_report_it() {
    let env = Env::new("panic-link");
    let out = env.brnr(&["session", "list"]).env("BRNR_TEST_PANIC", "cli").output().unwrap();
    let err = stderr(&out);
    assert_eq!(out.status.code(), Some(101), "{err}");
    let shown = err.find("a test asked for it").expect("Rust's message");
    let link = err.find(ISSUE_LINK).expect("the link");
    assert!(shown < link, "{err}");
    let url = err[link..].lines().next().unwrap();
    assert!(url.starts_with(&format!("{ISSUE_LINK}Panic%20at%20src%2Fbug.rs%3A")), "{url}");
    let version = format!("&version=brnr%20{}&setup=", env!("CARGO_PKG_VERSION"));
    assert!(url.contains(&version), "{url}");
    assert!(url.contains("&what=%60brnr%20session%20list%60%20panicked%3A"), "{url}");
    assert!(url.contains("a%20test%20asked%20for%20it"), "{url}");
    assert!(!url.contains(' '), "{url}");
    // A group's command is named with its verb, not its arguments.
    let args = ["prompt", "send", "sess-1", "x"];
    let out = env.brnr(&args).env("BRNR_TEST_PANIC", "cli").output().unwrap();
    let err = stderr(&out);
    assert_eq!(out.status.code(), Some(101), "{err}");
    assert!(err.contains("&what=%60brnr%20prompt%20send%60%20panicked%3A"), "{err}");

    let args = new_args(&["--foreground", "--prompt", "hi"]);
    let out =
        env.brnr(&args).env("BRNR_TEST_PANIC", "start").stdin(Stdio::null()).output().unwrap();
    let err = stderr(&out);
    assert_eq!(out.status.code(), Some(101), "{err}");
    assert_eq!(err.matches(ISSUE_LINK).count(), 1, "{err}");
    assert!(err.contains("&what=%60brnr%20session%20new%20--foreground%60%20panicked"), "{err}");
}

// ---- sending -----------------------------------------------------------

/// Several interrupts sent before the turn stops are delivered in the order
/// they were sent.
#[test]
fn adr_0004_interrupts_keep_their_order() {
    let env = Env::new("interrupt").agent("CANCEL_DELAY", "1");
    env.start(&["--prompt", "hang on"]);
    assert!(wait_for(Duration::from_secs(5), || env.prompts().len() == 1));
    for text in ["first", "second"] {
        let out = env.run(&["prompt", "send", "sess-1", "--interrupt", text]);
        assert!(out.status.success(), "send: {}", stderr(&out));
    }
    assert!(wait_for(Duration::from_secs(10), || env.prompts().len() == 3), "{:?}", env.prompts());
    assert_eq!(env.prompts(), ["hang on", "first", "second"]);
}

/// The messages of the turns that ended, in order, as `turn_ended` has them.
fn turns(env: &Env) -> Vec<Vec<String>> {
    let log = env.ok(&["event", "log", "sess-1", "--json", "--events", "turn_ended"]);
    let messages = |l: &str| serde_json::from_str::<Value>(l).unwrap()["messages"].take();
    log.lines().map(|l| serde_json::from_value(messages(l)).unwrap()).collect()
}

/// A message sent while a turn runs is held, never a second prompt, and
/// goes as a turn of its own when that one ends, in the order sent.
#[test]
fn adr_0004_send_while_a_turn_runs_is_held() {
    let env = Env::new("held-order");
    env.start(&["--prompt", "hang on"]);
    assert!(wait_for(Duration::from_secs(5), || env.prompts().len() == 1));
    assert_eq!(env.ok(&["prompt", "send", "sess-1", "reply first"]), "held (message m2)\n");
    assert_eq!(env.ok(&["prompt", "send", "sess-1", "reply second"]), "held (message m3)\n");
    sleep(Duration::from_millis(300));
    assert_eq!(env.prompts(), ["hang on"], "a second prompt while one ran");
    env.ok(&["prompt", "cancel", "sess-1", "--keep-held"]);
    let out = env.run(&["event", "wait", "sess-1", "--timeout", "10"]);
    let said = format!("{}{}", String::from_utf8_lossy(&out.stdout), stderr(&out));
    assert_eq!(out.status.code(), Some(0), "{said}{}", env.ok(&["event", "log", "sess-1"]));
    assert_eq!(env.prompts(), ["hang on", "reply first", "reply second"]);
    assert_eq!(turns(&env), [["m1"], ["m2"], ["m3"]]);
}

/// `--steer` goes into the running turn as `_session/steering`: no prompt of
/// its own, its id among that turn's messages, and `send --steer --wait`
/// ends with that turn.
#[test]
fn adr_0004_steer_goes_into_the_running_turn() {
    let env = Env::new("steer");
    env.start(&["--prompt", "hang on"]);
    assert!(wait_for(Duration::from_secs(5), || env.prompts().len() == 1));
    let out = env.run(&["prompt", "send", "sess-1", "--steer", "--wait", "reply steered"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "steered\n");
    assert!(stderr(&out).contains("steered (message m2)"), "{}", stderr(&out));
    assert_eq!(env.prompts(), ["hang on"], "a prompt of its own");
    let steer = &env.calls_of("_session/steering")[0]["params"];
    assert_eq!(steer["prompt"][0]["text"], "reply steered");
    assert_eq!(steer["_meta"]["steering"]["idleBehavior"], "promptRequired");
    assert_eq!(turns(&env), [["m1", "m2"]]);
    let user = env.ok(&["event", "log", "sess-1", "--events", "user_message"]);
    assert!(user.ends_with("user: reply steered\n"), "{user}");
}

/// `--steer` with no turn running is a prompt; so is one the agent answers
/// `promptRequired` (the turn ended before the steer reached it), which goes
/// ahead of what was held meanwhile, as it would have in the turn.
#[test]
fn adr_0004_steer_without_a_turn_is_a_prompt() {
    let env = Env::new("steer-idle");
    env.start(&[]);
    assert_eq!(
        env.ok(&["prompt", "send", "sess-1", "--steer", "reply now"]),
        "delivered (message m1)\n"
    );
    assert!(wait_for(Duration::from_secs(5), || env.prompts().len() == 1));
    assert!(env.calls_of("_session/steering").is_empty());

    // The agent reads nothing until this turn has ended.
    env.ok(&["prompt", "send", "sess-1", "slow 2"]);
    assert!(wait_for(Duration::from_secs(5), || env.prompts().len() == 2));
    assert_eq!(env.ok(&["prompt", "send", "sess-1", "reply held"]), "held (message m3)\n");
    let out = env.run(&["prompt", "send", "sess-1", "--steer", "--wait", "reply late"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stderr(&out).contains("steered (message m4)"), "{}", stderr(&out));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "late\n");
    assert_eq!(env.calls_of("_session/steering").len(), 1);
    assert_eq!(env.run(&["event", "wait", "sess-1", "--timeout", "10"]).status.code(), Some(0));
    assert_eq!(env.prompts(), ["reply now", "slow 2", "reply late", "reply held"]);
    assert_eq!(turns(&env)[2..], [["m4"], ["m3"]]);
}

/// The session's `turn_ended` and `message_dropped` events, in order, as
/// `(event, messages or message, by or reason)`.
fn endings(env: &Env) -> Vec<(String, Value, Value)> {
    let log = env.ok(&["event", "log", "sess-1", "--json"]);
    let events = log.lines().map(|l| serde_json::from_str::<Value>(l).unwrap());
    events
        .filter(|e| e["event"] == "turn_ended" || e["event"] == "message_dropped")
        .map(|e| {
            let (which, why) = match e["event"] == "turn_ended" {
                true => (&e["messages"], &e["by"]),
                false => (&e["message"], &e["reason"]),
            };
            (e["event"].as_str().unwrap().to_owned(), which.clone(), why.clone())
        })
        .collect()
}

/// A steer the agent answers `injected` only after the turn it went into has
/// ended is in that turn all the same: the turn's `turn_ended` waits for the
/// answer and lists it, and `send --steer --wait` ends with it.
#[test]
fn adr_0004_steer_answered_after_its_turn_is_in_that_turn() {
    let env = Env::new("steer-late").agent("STEER_ANSWER", "late");
    env.start(&["--prompt", "hang on"]);
    assert!(wait_for(Duration::from_secs(5), || env.prompts().len() == 1));
    let out = env.run(&[
        "prompt",
        "send",
        "sess-1",
        "--steer",
        "--wait",
        "--timeout",
        "10",
        "reply steered",
    ]);
    assert!(out.status.success(), "{}{}", stderr(&out), env.ok(&["event", "log", "sess-1"]));
    // What the agent said came before it said it took the message, so isn't
    // shown as its reply; the transcript has it.
    assert_eq!(String::from_utf8_lossy(&out.stdout), "");
    assert_eq!(turns(&env), [["m1", "m2"]]);
    let said = env.ok(&["event", "log", "sess-1", "--events", "user_message,agent_message"]);
    assert!(said.contains("agent: steered\n") && said.ends_with("user: reply steered\n"), "{said}");
}

/// A steer the agent answers with an error after its turn ended is dropped,
/// `by` `steer`, and that turn ends without it.
#[test]
fn adr_0004_steer_refused_after_its_turn_is_dropped() {
    let env = Env::new("steer-error").agent("STEER_ANSWER", "error");
    env.start(&["--prompt", "hang on"]);
    assert!(wait_for(Duration::from_secs(5), || env.prompts().len() == 1));
    let out = env.run(&[
        "prompt",
        "send",
        "sess-1",
        "--steer",
        "--wait",
        "--timeout",
        "10",
        "reply steered",
    ]);
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
fn adr_0004_steer_never_answered_is_dropped_by_close() {
    let env = Env::new("steer-never").agent("STEER_ANSWER", "never");
    env.start(&["--prompt", "hang on"]);
    assert!(wait_for(Duration::from_secs(5), || env.prompts().len() == 1));
    let args =
        ["prompt", "send", "sess-1", "--steer", "--wait", "--timeout", "20", "reply steered"];
    let waiter = env.brnr(&args).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    assert!(wait_for(Duration::from_secs(5), || env.calls_of("_session/steering").len() == 1));
    sleep(Duration::from_millis(300));
    assert_eq!(endings(&env), [], "the turn ended before its steer was answered");
    env.ok(&["session", "close", "sess-1"]);
    let out = waiter.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(stderr(&out).contains("m2 was dropped (close)"), "{}", stderr(&out));
    let dropped = ("message_dropped".to_owned(), Value::from("m2"), Value::from("close"));
    let ended = ("turn_ended".to_owned(), serde_json::json!(["m1"]), Value::from("control"));
    assert_eq!(endings(&env), [dropped, ended]);
}

/// `--interrupt` after a turn's own answer, while a steer into it is
/// unanswered, is held, with no turn to cancel: it waits for that answer, as
/// what is held does, then goes ahead of what is held. Each turn ends once,
/// with its own messages, and each message's `--wait` ends with its turn.
#[test]
fn adr_0004_interrupt_waits_for_the_steers_into_the_turn_before() {
    let env = Env::new("steer-interrupt");
    let gate = env.dir.join("answer-steer");
    let env = env.agent("STEER_ANSWER", "gate").agent("STEER_GATE", gate.to_str().unwrap());
    env.start(&[]);
    let wait = |args: &[&str]| {
        let mut all = vec!["prompt", "send", "sess-1", "--wait", "--timeout", "15"];
        all.extend(args);
        env.brnr(&all).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap()
    };
    let first = wait(&["hang on"]);
    assert!(wait_for(Duration::from_secs(5), || env.prompts().len() == 1));
    let steered = wait(&["--steer", "reply steered"]);
    assert!(wait_for(Duration::from_secs(5), || env.calls_of("_session/steering").len() == 1));
    sleep(Duration::from_millis(300));
    assert_eq!(endings(&env), [], "the turn ended before its steer was answered");
    let interrupt = wait(&["--interrupt", "reply third"]);
    let held = || env.ok(&["queue", "list", "sess-1"]).contains("m3");
    assert!(wait_for(Duration::from_secs(5), held), "{}", env.ok(&["queue", "list", "sess-1"]));
    assert_eq!(env.prompts(), ["hang on"], "a turn began before the last one ended");
    fs::write(&gate, "").unwrap();
    for (waiter, id) in [(first, "m1"), (steered, "m2"), (interrupt, "m3")] {
        let out = waiter.wait_with_output().unwrap();
        let log = env.ok(&["event", "log", "sess-1"]);
        assert_eq!(out.status.code(), Some(0), "{id}: {}{log}", stderr(&out));
        if id == "m3" {
            // Held: no turn runs for it to cancel.
            assert!(stderr(&out).contains("held (message m3)"), "{}", stderr(&out));
        }
    }
    assert!(env.calls_of("session/cancel").is_empty());
    assert_eq!(env.prompts(), ["hang on", "reply third"]);
    assert_eq!(turns(&env), [vec!["m1", "m2"], vec!["m3"]]);
}

/// `--steer` into a running turn needs an agent that advertises steering
/// (P7). With no turn running it is a prompt, steering or not.
#[test]
fn adr_0004_steer_needs_the_agents_steering() {
    let env = Env::new("steer-none").agent("NO_STEERING", "1");
    env.start(&["--prompt", "hang on"]);
    assert!(wait_for(Duration::from_secs(5), || env.prompts().len() == 1));
    let err = env.fails(&["prompt", "send", "sess-1", "--steer", "reply steered"]);
    let want = "the agent can't steer a running turn: it doesn't advertise _session/steering";
    assert!(err.contains(want), "{err}");
    assert!(env.calls_of("_session/steering").is_empty());
    env.ok(&["prompt", "cancel", "sess-1"]);
    env.run(&["event", "wait", "sess-1", "--timeout", "10"]);
    assert_eq!(
        env.ok(&["prompt", "send", "sess-1", "--steer", "reply now"]),
        "delivered (message m2)\n"
    );
}

// ---- strict mode -------------------------------------------------------

/// Strict mode (ADR 2.1), from `--strict` or the profile, is stable ACP
/// only: steering a running turn and forking are refused, saying why, and
/// the rest is as without.
#[test]
fn adr_0002_strict_mode_refuses_steering_and_fork() {
    let env = Env::new("strict");
    env.start(&["--strict", "--prompt", "hang on"]);
    assert_eq!(started_record(&env)["start_request"]["strict"], true);
    assert!(wait_for(Duration::from_secs(5), || env.prompts().len() == 1));
    let err = env.fails(&["prompt", "send", "sess-1", "--steer", "reply steered"]);
    let want = "--steer uses _session/steering, an ACP extension, which strict mode doesn't";
    assert!(err.contains(want), "{err}");
    let err = env.fails(&["session", "fork", "sess-1"]);
    let want = "session fork uses session/fork, unstable in ACP v1, which strict mode doesn't";
    assert!(err.contains(want), "{err}");
    assert!(env.calls_of("_session/steering").is_empty());
    assert!(env.calls_of("session/fork").is_empty());
    assert_eq!(env.ok(&["prompt", "send", "sess-1", "reply held"]), "held (message m2)\n");
    env.ok(&["prompt", "cancel", "sess-1", "--keep-held"]);
    assert!(wait_for(Duration::from_secs(5), || env.prompts().len() == 2));
    env.ok(&["session", "close", "sess-1"]);

    let env = Env::new("strict-profile");
    env.write_config("[profiles.default]\nstrict = true\n");
    env.start(&[]);
    assert!(env.fails(&["session", "fork", "sess-1"]).contains("which strict mode doesn't"));
    let env = Env::new("ed-strictarg");
    assert!(env.fails(&["acp", "--strict=yes", "--", AGENT]).contains("--strict takes no value"));
}

// ---- the editor's client capabilities (ADR 2.7) -------------------------

/// What an editor that offers its file system and terminal sends as
/// `initialize`.
const CAPABLE_INITIALIZE: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{"fs":{"readTextFile":true,"writeTextFile":true},"terminal":true}}}"#;

/// `brnr acp <args>`, initialized by an editor offering its file system and
/// terminal, with sess-1 open.
fn capable_editor(env: &Env, args: &[&str]) -> (Child, ChildStdin, BufReader<ChildStdout>) {
    let args: Vec<&str> = ["acp"].iter().chain(args).chain(&["--", AGENT]).copied().collect();
    let mut editor = env.brnr(&args).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let mut to_agent = editor.stdin.take().unwrap();
    let mut from_agent = BufReader::new(editor.stdout.take().unwrap());
    writeln!(to_agent, "{CAPABLE_INITIALIZE}").unwrap();
    response(&mut from_agent, 1);
    let new = format!(
        r#"{{"jsonrpc":"2.0","id":2,"method":"session/new","params":{{"cwd":{:?},"mcpServers":[]}}}}"#,
        env.dir.display().to_string()
    );
    writeln!(to_agent, "{new}").unwrap();
    assert_eq!(response(&mut from_agent, 2)["result"]["sessionId"], "sess-1");
    (editor, to_agent, from_agent)
}

/// The editor's `fs` and `terminal` capabilities don't reach the agent by
/// default, as ACP v2 has neither, and the host log says which were
/// dropped. In strict mode both pass, as stable ACP v1 has them, and so does
/// each one the profile's `client_capabilities` names, as the editor sent
/// it.
#[test]
fn adr_0002_fs_and_terminal_reach_the_agent_in_strict_mode_or_when_named() {
    let sent: Value = serde_json::from_str(CAPABLE_INITIALIZE).unwrap();
    let sent = &sent["params"]["clientCapabilities"];
    let named = |names: &str| format!("[profiles.default.editor]\nclient_capabilities = {names}\n");
    let cases = [
        ("ed-caps", vec![], None, vec![]),
        ("ed-strict", vec!["--strict"], None, vec!["fs", "terminal"]),
        ("ed-fs", vec![], Some(named(r#"["fs"]"#)), vec!["fs"]),
        ("ed-both", vec![], Some(named(r#"["fs", "terminal"]"#)), vec!["fs", "terminal"]),
    ];
    for (name, args, config, passed) in cases {
        let env = Env::new(name);
        if let Some(config) = config {
            env.write_config(&config);
        }
        let (mut editor, _to_agent, _from_agent) = capable_editor(&env, &args);
        assert_eq!(started_record(&env)["start_request"]["strict"], !args.is_empty());
        let caps = &env.calls_of("initialize")[0]["params"]["clientCapabilities"];
        for cap in ["fs", "terminal"] {
            let expected = if passed.contains(&cap) { &sent[cap] } else { &Value::Null };
            assert_eq!(&caps[cap], expected, "{name}: {cap} in {caps}");
        }
        // The note comes before the session/new the host log records next.
        let logged = || host_logs(&env).contains("session/new");
        assert!(wait_for(Duration::from_secs(5), logged), "{name}: no session/new");
        let note = (host_logs(&env).lines())
            .map(|l| serde_json::from_str::<Value>(l).unwrap()["event"].clone())
            .find(|e| e["event"] == "capabilities_dropped");
        let dropped: Vec<&str> =
            ["fs", "terminal"].into_iter().filter(|c| !passed.contains(c)).collect();
        match note {
            Some(note) => {
                let names: Vec<&str> =
                    note["capabilities"].as_object().unwrap().keys().map(String::as_str).collect();
                assert_eq!(names, dropped, "{name}: {note}");
                for cap in dropped {
                    assert_eq!(note["capabilities"][cap], sent[cap], "{name}: {note}");
                }
            }
            None => assert!(dropped.is_empty(), "{name}: no capabilities_dropped note"),
        }
        let _ = editor.kill();
        let _ = editor.wait();
    }
}

/// With both named in the profile, the agent's `fs/*` and `terminal/*`
/// requests reach the editor as the agent wrote them, and the editor's
/// answers, results and errors, reach the agent.
#[test]
fn adr_0002_the_agents_file_and_terminal_requests_reach_the_editor() {
    let env = Env::new("ed-client");
    env.write_config("[profiles.default.editor]\nclient_capabilities = [\"fs\", \"terminal\"]\n");
    let (_editor, mut to_agent, mut from_agent) = capable_editor(&env, &[]);
    writeln!(to_agent, "{}", editor_prompt(3, "client")).unwrap();
    // Without the capabilities the agent asks nothing and answers the prompt.
    let asked = |from_agent: &mut BufReader<ChildStdout>, method: &str| {
        let msg = message(from_agent, |m| m["method"] == method || m["id"] == 3);
        assert_eq!(msg["method"], method, "{msg}");
        msg
    };
    let read = asked(&mut from_agent, "fs/read_text_file");
    assert_eq!(read["id"], "fs-1");
    assert_eq!(read["params"], json!({ "sessionId": "sess-1", "path": "/work/notes.txt" }));
    writeln!(to_agent, r#"{{"jsonrpc":"2.0","id":"fs-1","result":{{"content":"unsaved text"}}}}"#)
        .unwrap();
    let said = update(&mut from_agent, "agent_message_chunk");
    assert_eq!(said["content"]["text"], "read: unsaved text");
    let create = asked(&mut from_agent, "terminal/create");
    assert_eq!(create["id"], "terminal-2");
    let params = json!({ "sessionId": "sess-1", "command": "echo", "args": ["hi"] });
    assert_eq!(create["params"], params);
    let refused = r#"{"jsonrpc":"2.0","id":"terminal-2","error":{"code":-32603,"message":"no terminal here"}}"#;
    writeln!(to_agent, "{refused}").unwrap();
    let said = update(&mut from_agent, "agent_message_chunk");
    assert_eq!(said["content"]["text"], "\nterminal failed: no terminal here");
    assert_eq!(response(&mut from_agent, 3)["result"]["stopReason"], "end_turn");
    let answers: Vec<Value> =
        env.calls().into_iter().filter(|c| c.get("method").is_none()).collect();
    assert_eq!(answers[0]["result"]["content"], "unsaved text", "{answers:?}");
    assert_eq!(answers[1]["error"]["message"], "no terminal here", "{answers:?}");
}

/// A session served headless offers the agent neither the file system nor a
/// terminal, as no editor is there to serve them: one started headless, one
/// forked from it, and one taken over from an editor that offered both. An
/// agent that asks anyway is told the host doesn't serve them.
#[test]
fn adr_0002_headless_sessions_offer_neither() {
    let neither = "no fs\nno terminal\n";
    let env = Env::new("hl-client");
    env.start(&[]);
    assert_eq!(env.ok(&["prompt", "send", "sess-1", "--wait", "client"]), neither);
    let refused = "fs failed: fs/read_text_file is not supported without an editor\n\
                   terminal failed: terminal/create is not supported without an editor\n";
    assert_eq!(env.ok(&["prompt", "send", "sess-1", "--wait", "client", "anyway"]), refused);
    assert_eq!(env.ok(&["session", "fork", "sess-1"]), "forked sess-1 into sess-2\n");
    assert_eq!(env.ok(&["prompt", "send", "sess-2", "--wait", "client"]), neither);
    // Elicitation is offered headless (ADR 4.5); fs and terminal are not.
    let init = env.calls_of("initialize");
    let caps = &init[0]["params"]["clientCapabilities"];
    assert!(caps["fs"].is_null() && caps["terminal"].is_null(), "{caps}");

    let env = Env::new("hl-takeover");
    env.write_config("[profiles.default.editor]\nexperimental = [\"close\"]\n");
    let (_editor, _to_agent, _from_agent) = capable_editor(&env, &[]);
    let editors = env.pid();
    let out = env.run(&resume_args("sess-1", &["--take-over"]));
    assert!(out.status.success(), "{}", stderr(&out));
    let status: Value =
        serde_json::from_str(&env.ok(&["session", "status", "sess-1", "--json"])).unwrap();
    assert_ne!(status["pid"].to_string(), editors);
    assert_eq!(env.ok(&["prompt", "send", "sess-1", "--wait", "client"]), neither);
    let offered: Vec<Value> = (env.calls_of("initialize").iter())
        .map(|c| c["params"]["clientCapabilities"].clone())
        .collect();
    assert_eq!(offered.len(), 2, "{offered:?}");
    assert!(offered[1]["fs"].is_null() && offered[1]["terminal"].is_null(), "{offered:?}");
}

// ---- watching ----------------------------------------------------------

/// A watcher that stops reading is disconnected instead of having every
/// event queued for it in the host. It then catches up from the transcript
/// (ADR 10.2), so it stays until the process stops.
#[test]
fn adr_0010_slow_watcher_is_disconnected() {
    // About 30 MB of events: more than a peer's queue holds.
    let env = Env::new("slowwatch").agent("FLOOD", "40000");
    env.start(&[]);
    let mut watch = env
        .brnr(&["event", "watch", "sess-1", "--events", "all"])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    sleep(Duration::from_millis(300));
    kill(watch.id() as i32, libc::SIGSTOP);
    // Stopped until the turn has ended, so every event came while it was:
    // continued sooner, it would be reading again when the turn's 20 MB
    // message came, which a peer that keeps up takes (ADR 10.1).
    let out = env.run(&["prompt", "send", "sess-1", "--wait", "go"]);
    assert!(out.status.success(), "send: {}", stderr(&out));
    kill(watch.id() as i32, libc::SIGCONT);

    let dropped = || host_logs(&env).contains(r#""event":"peer_dropped""#);
    assert!(wait_for(Duration::from_secs(20), dropped), "the watcher wasn't cut off");
    // It may still be working through the burst; it must get there.
    let answers = || env.run(&["session", "status", "sess-1"]).status.success();
    assert!(wait_for(Duration::from_secs(30), answers), "host stopped answering");
    env.stop();
    assert!(wait_exit(&mut watch, Duration::from_secs(30)), "watch didn't end");
    let mut err = String::new();
    watch.stderr.take().unwrap().read_to_string(&mut err).unwrap();
    assert!(watch.wait().unwrap().success(), "watch failed: {err}");
}

// ---- catching up -------------------------------------------------------

/// `brnr event watch …` for `env`, its stdout read to its end on a thread,
/// as JSON lines.
fn watcher(env: &Env, args: &[&str]) -> (Child, std::thread::JoinHandle<Vec<Value>>) {
    let mut watch = env.brnr(args).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let stdout = watch.stdout.take().unwrap();
    let lines = std::thread::spawn(move || {
        let lines = BufReader::new(stdout).lines().map_while(Result::ok);
        lines.map(|l| serde_json::from_str(&l).unwrap()).collect()
    });
    (watch, lines)
}

/// The positions of `events` (ADR 10.2).
fn seqs(events: &[Value]) -> Vec<u64> {
    events.iter().map(|e| e["seq"].as_u64().unwrap_or_else(|| panic!("no seq: {e}"))).collect()
}

/// A watcher cut off for falling behind (stopped, here, while two 6 MB
/// replies came, each as its ACP message and its event) catches up from the
/// transcript and goes on live: it gets every event once, in order, as the
/// transcript has them. Another watcher is told it was cut off.
#[test]
fn adr_0010_a_watcher_cut_off_catches_up() {
    let env = Env::new("catchup");
    env.start(&[]);
    let (mut watch, lines) =
        watcher(&env, &["event", "watch", "sess-1", "--json", "--events", "all"]);
    let args = ["event", "watch", "sess-1", "--json", "--events", "peer_cut_off"];
    let (mut told, told_lines) = watcher(&env, &args);
    sleep(Duration::from_millis(300));
    kill(watch.id() as i32, libc::SIGSTOP);
    for _ in 0..2 {
        env.ok(&["prompt", "send", "sess-1", "--wait", "big 6000000"]);
    }
    let dropped = || host_logs(&env).contains(r#""event":"peer_dropped""#);
    let cut = wait_for(Duration::from_secs(10), dropped);
    kill(watch.id() as i32, libc::SIGCONT);
    assert!(cut, "the watcher wasn't cut off");
    assert_eq!(env.ok(&["prompt", "send", "sess-1", "--wait", "reply after"]), "after\n");
    env.stop();
    for w in [&mut watch, &mut told] {
        assert!(wait_exit(w, Duration::from_secs(30)), "watch didn't end");
    }
    let mut err = String::new();
    watch.stderr.take().unwrap().read_to_string(&mut err).unwrap();
    assert!(watch.wait().unwrap().success(), "watch failed: {err}");

    let got = lines.join().unwrap();
    let texts: Vec<usize> = (got.iter())
        .filter(|e| e["event"] == "agent_message")
        .map(|e| e["text"].as_str().unwrap().len())
        .collect();
    assert_eq!(texts, [6_000_000, 6_000_000, 5], "the agent's messages");
    assert_eq!(got.last().unwrap()["event"], "process_exited");
    // Its own cut-off, which it asked for too, came from the host log.
    let own = got.iter().filter(|e| e["event"] == "peer_cut_off").count();
    assert_eq!(own, 1, "its own cut-off");
    // Once each, in order, and none of what the transcript has left out.
    let session: Vec<Value> = got.into_iter().filter(|e| e["event"] != "peer_cut_off").collect();
    let got = seqs(&session);
    assert!(got.windows(2).all(|w| w[0] < w[1]), "out of order or twice: {got:?}");
    let log = env.ok(&["event", "log", "sess-1", "--events", "all", "--json"]);
    let logged: Vec<Value> = log.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    // The session's `session/new` has its position in the host log only.
    let logged = logged.iter().filter_map(|e| e["seq"].as_u64());
    let logged: Vec<u64> = logged.filter(|n| *n >= got[0]).collect();
    assert_eq!(got, logged, "the watcher's events and the transcript's");

    let told = told_lines.join().unwrap();
    let cut = told.iter().find(|e| e["event"] == "peer_cut_off").expect("not told");
    let host = host_logs(&env);
    assert!(host.contains(&format!(r#""peer":"{}""#, cut["peer"].as_str().unwrap())), "{cut}");
    assert!(cut["last_queued_seq"].as_u64() < Some(*got.last().unwrap()), "{cut}");
}

/// The burst of #117: a thought and a message each over 16 MiB, to a
/// watcher that asked for `acp`, come to it as four lines past 64 MiB, which
/// cut it off even when it keeps up (here it reads nothing meanwhile, so
/// that it surely is). It catches up from the transcript: both reach it.
#[test]
fn adr_0010_a_burst_past_64_mib_reaches_a_watcher() {
    let env = Env::new("burst117");
    env.start(&[]);
    let (mut watch, lines) =
        watcher(&env, &["event", "watch", "sess-1", "--json", "--events", "all"]);
    sleep(Duration::from_millis(300));
    kill(watch.id() as i32, libc::SIGSTOP);
    let size = (16 << 20) + 1;
    env.ok(&["prompt", "send", "sess-1", "--wait", &format!("big {size} think")]);
    let dropped = || host_logs(&env).contains(r#""event":"peer_dropped""#);
    let cut = wait_for(Duration::from_secs(10), dropped);
    kill(watch.id() as i32, libc::SIGCONT);
    assert!(cut, "the watcher wasn't cut off");
    env.stop();
    assert!(wait_exit(&mut watch, Duration::from_secs(60)), "watch didn't end");
    let mut err = String::new();
    watch.stderr.take().unwrap().read_to_string(&mut err).unwrap();
    assert!(watch.wait().unwrap().success(), "watch failed: {err}");
    let got = lines.join().unwrap();
    let long = |name: &str| {
        let text = |e: &&Value| e["event"] == name && e["text"].as_str().unwrap().len() == size;
        got.iter().filter(text).count()
    };
    assert_eq!((long("agent_thought"), long("agent_message")), (1, 1));
    let got = seqs(&got);
    assert!(got.windows(2).all(|w| w[0] < w[1]), "out of order or twice");
}

/// A watcher cut off where the transcript can't have what it missed (here
/// none is kept, `log = false`) says what it missed, after which event, and
/// why, as its last line, and exits 3 (ADR 10.2).
#[test]
fn adr_0010_a_watcher_that_cant_catch_up_says_what_it_missed() {
    let env = Env::new("nocatchup");
    env.write_config("[profiles.default]\nlog = false\n");
    env.start(&[]);
    let (mut watch, lines) =
        watcher(&env, &["event", "watch", "sess-1", "--json", "--events", "all"]);
    sleep(Duration::from_millis(300));
    kill(watch.id() as i32, libc::SIGSTOP);
    for _ in 0..2 {
        env.ok(&["prompt", "send", "sess-1", "--wait", "big 6000000"]);
    }
    kill(watch.id() as i32, libc::SIGCONT);
    assert!(wait_exit(&mut watch, Duration::from_secs(30)), "watch didn't end");
    assert_eq!(watch.wait().unwrap().code(), Some(3));
    let got = lines.join().unwrap();
    let note = got.last().unwrap();
    assert_eq!(note["event"], "events_missed", "{note}");
    assert_eq!(note["error"], "the process keeps no transcript (log = false)");
    let before = &got[got.len() - 2];
    assert_eq!(note["after_seq"], before["seq"], "{note}");
    assert!(note["until_seq"].as_u64() > note["after_seq"].as_u64(), "{note}");
    env.stop();
}

/// `prompt send --wait`, cut off while its message is held behind another
/// turn (stopped, here, while that turn's four 6 MB messages came), catches
/// up: it prints its turn's reply and exits as the turn ended (ADR 10.2).
#[test]
fn adr_0010_a_wait_cut_off_catches_up() {
    let env = Env::new("waitcatchup");
    env.start(&[]);
    env.ok(&["prompt", "send", "sess-1", "hang"]);
    let wait = env
        .brnr(&["prompt", "send", "sess-1", "--wait", "reply done"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let held = || env.ok(&["queue", "list", "sess-1"]).contains("m2 (after turn)");
    assert!(wait_for(Duration::from_secs(10), held), "not held");
    kill(wait.id() as i32, libc::SIGSTOP);
    env.ok(&["prompt", "send", "sess-1", "--interrupt", "many 4 6000000"]);
    env.ok(&["event", "wait", "sess-1"]);
    let dropped = || host_logs(&env).contains(r#""event":"peer_dropped""#);
    let cut = wait_for(Duration::from_secs(10), dropped);
    kill(wait.id() as i32, libc::SIGCONT);
    assert!(cut, "the wait wasn't cut off");
    let out = wait.wait_with_output().unwrap();
    assert!(out.status.success(), "send --wait: {}", stderr(&out));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "done\n");
    env.stop();
}

/// `event wait --for turn`, cut off while four 6 MB approvals came (stopped,
/// here), catches up on the turn's end (ADR 10.2).
#[test]
fn adr_0010_event_wait_cut_off_catches_up() {
    let env = Env::new("evwaitcatchup").agent("PERM_PAD", "6000000");
    env.start(&[]);
    let wait = env
        .brnr(&["event", "wait", "sess-1", "--for", "turn", "--json"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    sleep(Duration::from_millis(300));
    kill(wait.id() as i32, libc::SIGSTOP);
    env.ok(&["prompt", "send", "sess-1", "perm edit 4"]);
    let pending = || env.ok(&["permission", "requests", "sess-1"]).contains("p4 ");
    assert!(wait_for(Duration::from_secs(10), pending), "no approvals");
    for n in 1..=4 {
        env.ok(&["permission", "allow", "sess-1", &format!("p{n}")]);
    }
    env.ok(&["event", "wait", "sess-1"]);
    let dropped = || host_logs(&env).contains(r#""event":"peer_dropped""#);
    let cut = wait_for(Duration::from_secs(10), dropped);
    kill(wait.id() as i32, libc::SIGCONT);
    assert!(cut, "the wait wasn't cut off");
    let out = wait.wait_with_output().unwrap();
    assert!(out.status.success(), "event wait: {}", stderr(&out));
    let ended: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        (ended["event"].as_str(), ended["stop_reason"].as_str()),
        (Some("turn_ended"), Some("end_turn"))
    );
    env.stop();
}

/// A watcher that keeps reading stays connected through a burst, and ends
/// cleanly when the host exits. Stopped once it has read the burst: an
/// exit waits only so long for a peer to be sent what is queued for it.
#[test]
fn adr_0010_reading_watcher_stays_connected() {
    let env = Env::new("fastwatch").agent("FLOOD", "20000");
    env.start(&[]);
    let mut watch = env
        .brnr(&["event", "watch", "sess-1", "--events", "all"])
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
    assert!(env.run(&["prompt", "send", "sess-1", "go"]).status.success());
    let burst = || read.load(Relaxed) > 20000 || watch.try_wait().unwrap().is_some();
    assert!(wait_for(Duration::from_secs(60), burst), "watch read {} lines", read.load(Relaxed));
    assert!(env.run(&["process", "stop", &env.pid()]).status.success());

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
fn adr_0010_huge_message_reaches_watchers() {
    let env = Env::new("huge");
    env.start(&[]);
    let mut watch = env
        .brnr(&["event", "watch", "sess-1", "--json", "--events", "agent_message"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = watch.stdout.take().unwrap();
    let first = std::thread::spawn(move || BufReader::new(stdout).lines().next());
    sleep(Duration::from_millis(300));
    let size = 20_000_000;
    assert!(env.run(&["prompt", "send", "sess-1", &format!("big {size}")]).status.success());
    let line = first.join().unwrap().expect("no message").unwrap();
    let event: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(event["text"].as_str().unwrap().len(), size);

    let out = env.run(&["session", "status", "sess-1", "--json"]);
    assert!(out.status.success(), "status: {}", stderr(&out));
    let status: Value = serde_json::from_slice(&out.stdout).unwrap();
    let preview = status["last_message"].as_str().unwrap();
    assert!(preview.chars().count() <= 4001, "status quotes {} chars", preview.chars().count());
    let _ = watch.kill();
    let _ = watch.wait();
}

/// A reply bigger than a peer's whole queue reaches every kind of peer that
/// keeps up, and none is cut off for it (ADR 10.1): `session new --wait`,
/// `prompt send --wait`, a watcher and a bridge, the last two asking for `acp` too, so
/// that the reply comes to them twice at once, as its ACP message and as
/// its event.
#[test]
fn adr_0010_a_reply_bigger_than_the_queue_reaches_every_peer() {
    let env = Env::new("hugeall");
    let got = env.dir.join("bridge-events");
    env.write_config(&format!(
        "[[profiles.default.bridges]]\ncommand = [\"sh\", \"-c\", \"exec cat > '{}'\"]\n\
         events = [\"acp\", \"agent_message\", \"turn_ended\"]\n",
        got.display()
    ));
    let size = 20_000_000;
    let out = env.run(&new_args(&["--wait", "--prompt", &format!("big {size}")]));
    assert!(out.status.success(), "session new --wait: {}", stderr(&out));
    assert_eq!(out.stdout.len(), size + 1, "session new --wait: {}", stderr(&out));

    let mut watch = env
        .brnr(&["event", "watch", "sess-1", "--json", "--events", "all"])
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
    let out = env.run(&["prompt", "send", "sess-1", "--wait", &format!("big {size}")]);
    assert!(out.status.success(), "send --wait: {}", stderr(&out));
    assert_eq!(out.stdout.len(), size + 1, "send --wait: {}", stderr(&out));
    let out = env.run(&["prompt", "send", "sess-1", "--wait", "--json", &format!("big {size}")]);
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
/// message, and not by the `turn_ended` that comes right after it (ADR 10.1).
/// Here it is 8 MB behind (stopped, for the test), then a 12 MB message
/// comes.
#[test]
fn adr_0010_a_long_message_doesnt_put_a_watcher_behind() {
    let env = Env::new("longmsg");
    env.start(&[]);
    let mut watch = env
        .brnr(&["event", "watch", "sess-1", "--json", "--events", "agent_message,turn_ended"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = watch.stdout.take().unwrap();
    let lines = std::thread::spawn(move || BufReader::new(stdout).lines().count());
    sleep(Duration::from_millis(300));
    kill(watch.id() as i32, libc::SIGSTOP);
    for size in [8_000_000, 12_000_000] {
        let out = env.run(&["prompt", "send", "sess-1", "--wait", &format!("big {size}")]);
        assert!(out.status.success(), "{}", stderr(&out));
    }
    // The watcher's line after the long message, which it is past the limit
    // with, and a status to make sure.
    assert!(env.run(&["session", "status", "sess-1"]).status.success());
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
fn adr_0011_adapters_next_to_a_symlinked_brnr() {
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

    let args =
        ["session", "new", "--wait", "--prompt", "reply linked", "--", "brnr-claude-adapter"];
    let out = env.brnr_at(&bin.join("brnr"), &args).env("PATH", &path).output().unwrap();
    assert!(out.status.success(), "start: {}", stderr(&out));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "linked\n");
}

/// A started bridge's bare command is found next to a symlinked brnr as an
/// adapter is, by whoever starts the process (ADR 3.3, ADR 11.2): the request
/// it hands the process has the path, and the bridge runs.
#[test]
fn adr_0011_bridges_next_to_a_symlinked_brnr() {
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

    let args = new_args(&["--wait", "--prompt", "reply linked"]);
    let out = env.brnr_at(&bin.join("brnr"), &args).env("PATH", &path).output().unwrap();
    assert!(out.status.success(), "start: {}", stderr(&out));
    let request = started_record(&env)["start_request"].clone();
    let command = request["bridges"][0]["command"][0].as_str().unwrap_or_default();
    assert_eq!(fs::canonicalize(command).ok(), fs::canonicalize(&bridge).ok(), "{request}");
    let ended = || fs::read_to_string(&got).unwrap_or_default().contains(r#""event":"turn_ended""#);
    assert!(wait_for(Duration::from_secs(5), ended), "the bridge got no events");
    env.stop();
}

/// A peer a few MB behind still takes one big message: it is cut off only
/// once its backlog is past the limit, not because the next line is big.
#[test]
fn adr_0010_big_message_to_a_lagging_watcher() {
    let env = Env::new("lagbig");
    env.start(&[]);
    let mut watch = env
        .brnr(&["event", "watch", "sess-1", "--json", "--events", "agent_message"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = watch.stdout.take().unwrap();
    sleep(Duration::from_millis(300));
    kill(watch.id() as i32, libc::SIGSTOP);
    for size in [6_000_000, 12_000_000] {
        let out = env.run(&["prompt", "send", "sess-1", "--wait", &format!("big {size}")]);
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
fn adr_0010_every_watcher_sees_the_exit() {
    let env = Env::new("exitall");
    env.start(&[]);
    let watchers: Vec<_> = (0..8)
        .map(|_| {
            env.brnr(&["event", "watch", "sess-1", "--json", "--events", "process_exited"])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    sleep(Duration::from_millis(500));
    assert!(env.run(&["process", "stop", &env.pid()]).status.success());
    for watch in watchers {
        let out = watch.wait_with_output().unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(text.contains(r#""event":"process_exited""#), "no exited: {}", stderr(&out));
    }
}

/// A host that doesn't answer is reported as such, not as an errno.
#[test]
fn unresponsive_host_is_reported() {
    let env = Env::new("unresp");
    env.start(&[]);
    let host = env.host_pid();
    kill(host, libc::SIGSTOP);
    let out = env.run(&["prompt", "send", "sess-1", "hello"]);
    kill(host, libc::SIGCONT);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("not answering"), "{}", stderr(&out));
}

/// A line that isn't a request (not even UTF-8) is answered with an error,
/// and the connection carries on.
#[test]
fn adr_0005_bad_request_line_is_answered() {
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

/// What a started bridge wrote on stderr, as the host log has it
/// (`bridge_stderr`), and its `bridge_exited` record once it has one.
fn bridge_stderr(env: &Env) -> (Vec<String>, Option<Value>) {
    let records: Vec<Value> =
        host_logs(env).lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    let said = (records.iter().filter(|r| r["event"]["event"] == "bridge_stderr"))
        .map(|r| r["event"]["text"].as_str().unwrap().to_owned())
        .collect();
    let exited = records.into_iter().find(|r| r["event"]["event"] == "bridge_exited");
    (said, exited.map(|mut r| r["event"].take()))
}

/// A started bridge that writes a line on stderr that isn't UTF-8 carries
/// on: the line is in the host log, decoded lossily, and so is what it writes
/// after it. It gets its events, and exits 0 when its stdin closes. (Read
/// as text, the bad line ended the reading: the bridge's next write was
/// killed by SIGPIPE.)
#[test]
fn adr_0005_a_bridge_whose_stderr_isnt_utf8_carries_on() {
    let env = Env::new("brerr-bytes");
    let events = env.dir.join("bridge-events");
    // The pause lets the host read the bad line before the next one comes.
    let script = format!(
        "printf 'bad \\377\\376 bytes\\n' >&2; sleep 0.5; echo after >&2; exec cat > '{}'",
        events.display()
    );
    env.write_config(&format!(
        "[[profiles.default.bridges]]\ncommand = [\"sh\", \"-c\", {script:?}]\n"
    ));
    env.start(&[]);
    let after = || bridge_stderr(&env).0.len() == 2;
    assert!(wait_for(Duration::from_secs(5), after), "{:?}", bridge_stderr(&env));
    assert_eq!(bridge_stderr(&env).0, ["bad \u{fffd}\u{fffd} bytes", "after"]);
    env.ok(&["prompt", "send", "sess-1", "--wait", "reply hi"]);
    let ended =
        || fs::read_to_string(&events).unwrap_or_default().contains(r#""event":"turn_ended""#);
    assert!(wait_for(Duration::from_secs(5), ended), "the bridge got no events");
    env.stop();
    assert!(wait_for(Duration::from_secs(5), || bridge_stderr(&env).1.is_some()), "not exited");
    let exited = bridge_stderr(&env).1.unwrap();
    assert_eq!(exited["exit_status"], json!({ "code": 0 }), "killed by a signal: {exited}");
}

/// What a started bridge that runs `script` wrote on stderr, as the host log
/// has it, once it has written `after` too.
fn bridge_says(name: &str, script: &str) -> Vec<String> {
    let env = Env::new(name);
    let script = format!("{script}; echo after >&2; exec cat > /dev/null");
    env.write_config(&format!(
        "[[profiles.default.bridges]]\ncommand = [\"sh\", \"-c\", {script:?}]\n"
    ));
    env.start(&[]);
    let after = || bridge_stderr(&env).0.last().is_some_and(|l| l == "after");
    assert!(wait_for(Duration::from_secs(5), after), "{:?}", bridge_stderr(&env).0.len());
    env.stop();
    bridge_stderr(&env).0
}

/// `n` bytes of `x` on stderr, from a shell.
fn xs(n: usize) -> String {
    format!("head -c {n} /dev/zero | tr '\\0' x >&2")
}

/// A started bridge's stderr line longer than 64 KiB goes to the host log in
/// pieces of that, every byte, as the agent's does (ADR 10.3): the host holds
/// no more of it than a piece.
#[test]
fn adr_0010_a_bridges_long_stderr_line_goes_to_the_host_log_in_pieces() {
    let said = bridge_says("brerr-long", &format!("{}; echo >&2", xs(131073)));
    let pieces: Vec<usize> = said.iter().map(|l| l.len()).collect();
    assert_eq!(pieces, [64 << 10, 64 << 10, 1, 5]);
    assert!(said[..3].iter().all(|l| l.bytes().all(|b| b == b'x')));
}

/// A bridge's stderr line that fills a piece exactly is one note: the newline
/// that comes after it ends it, and isn't an empty line of its own.
#[test]
fn adr_0010_a_bridges_stderr_line_that_fills_a_piece_is_one_note() {
    let said = bridge_says("brerr-full", &format!("{}; echo >&2", xs(64 << 10)));
    let pieces: Vec<usize> = said.iter().map(|l| l.len()).collect();
    assert_eq!(pieces, [64 << 10, 5]);
}

/// A carriage return that ends a piece, not its line, is a byte of the
/// line, and stays.
#[test]
fn adr_0010_a_bridges_stderr_keeps_a_carriage_return_inside_a_line() {
    let script = format!("{}; printf '\\ryz\\n' >&2", xs((64 << 10) - 1));
    let said = bridge_says("brerr-cr", &script);
    let pieces: Vec<usize> = said.iter().map(|l| l.len()).collect();
    assert_eq!(pieces, [64 << 10, 2, 5]);
    assert!(said[0].ends_with("x\r"), "{:?}", &said[0][said[0].len() - 2..]);
    assert_eq!(said[1], "yz");
}

/// A character that a piece's end would cut goes whole into the next piece,
/// which takes no more bytes for it: it isn't two replacement characters.
#[test]
fn adr_0010_a_bridges_stderr_keeps_a_character_a_piece_would_cut() {
    let script = format!("{}; printf '\\303\\251\\n' >&2", xs((64 << 10) - 1));
    let said = bridge_says("brerr-utf8", &script);
    let pieces: Vec<usize> = said.iter().map(|l| l.len()).collect();
    assert_eq!(pieces, [(64 << 10) - 1, 2, 5]);
    assert_eq!(said[1], "\u{e9}");
}

/// `brnr session list | head -1`: a reader that goes away ends brnr quietly, as it
/// would a filter.
#[test]
fn closed_stdout_ends_quietly() {
    let env = Env::new("epipe");
    env.start(&[]);
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);
    let out =
        env.brnr(&["session", "list"]).stdout(writer).stderr(Stdio::piped()).output().unwrap();
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
    assert_eq!(env.ok(&["session", "list"]), "no sessions\n", "a missing dir is no error");
    let run = env.dir.join("run");
    fs::create_dir(&run).unwrap();
    fs::set_permissions(&run, fs::Permissions::from_mode(0o777)).unwrap();
    let err = env.fails(&["session", "list"]);
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
    let list: Value = serde_json::from_str(&env.ok(&["session", "list", "--json"])).unwrap();
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
        env.ok(&["event", "log", "sess-1"]); // Once the transcript is written.
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

/// A session's transcript is two files: its events, which `event log`,
/// `session list` and `session resume` read, and its raw ACP beside them (ADR 5.1).
/// A session's opening is one record, the same in its events file and the
/// host log: the session, its cwd and its three files, and `forked_from`
/// for a fork (ADR 5.9).
#[test]
fn adr_0005_a_sessions_opening_is_the_same_in_both_files() {
    let env = Env::new("opened");
    env.start(&[]);
    env.ok(&["session", "fork", "sess-1"]);
    env.stop();
    assert!(wait_for(Duration::from_secs(15), || env.hosts().is_empty()));
    let (dir, _) = project(&env);
    let host = fs::read_dir(env.dir.join("home/hosts")).unwrap().next().unwrap().unwrap().path();
    let in_host: Vec<Value> = (records(&host).into_iter())
        .map(|r| r["event"].clone())
        .filter(|e| e["event"] == "session_opened")
        .collect();
    for (id, fork) in [("sess-1", Value::Null), ("sess-2", "sess-1".into())] {
        let events = dir.join(format!("{id}.jsonl"));
        let opened = records(&events)[0]["event"].clone();
        let keys: Vec<&String> = opened.as_object().unwrap().keys().collect();
        let mut want = vec!["event", "session", "cwd", "events_log", "acp_log", "host_log"];
        if fork.is_string() {
            want.push("forked_from");
        }
        assert_eq!(keys, want, "{opened}");
        assert_eq!((&opened["session"], &opened["forked_from"]), (&id.into(), &fork));
        assert_eq!(opened["events_log"], events.to_string_lossy().as_ref());
        assert_eq!(
            opened["acp_log"],
            dir.join(format!("{id}.acp.jsonl")).to_string_lossy().as_ref()
        );
        assert_eq!(opened["host_log"], host.to_string_lossy().as_ref());
        assert!(in_host.contains(&opened), "not the host log's: {opened} {in_host:?}");
    }
}

#[test]
fn adr_0005_transcripts_are_two_files() {
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
    assert_eq!(opened["event"], "session_opened");
    assert_eq!(opened["acp_log"], dir.join("sess-1.acp.jsonl").to_string_lossy().as_ref());
    assert_eq!(events.last().unwrap()["event"]["event"], "process_exited");
    // ACP only, the session's, with what joins it to the events.
    assert!(acp.iter().all(|r| r["dir"].is_string() && r.get("event").is_none()), "{acp:?}");
    let host_id = &events[0]["host_id"];
    assert!(acp.iter().all(|r| r["session"] == "sess-1" && r["host_id"] == *host_id));
    assert!(acp.iter().any(|r| r["msg"]["method"] == "session/new"));
    assert!(acp.iter().any(|r| r["msg"]["method"] == "session/prompt"));
    // What belongs to no session is in the host log.
    let host = fs::read_dir(env.dir.join("home/hosts")).unwrap().next().unwrap().unwrap().path();
    assert!(records(&host).iter().any(|r| r["msg"]["method"] == "initialize"));
    // One session, for session list and session resume.
    let list: Value =
        serde_json::from_str(&env.ok(&["session", "list", "--include", "inactive", "--json"]))
            .unwrap();
    assert_eq!(list.as_array().unwrap().len(), 1, "{list}");
    assert_eq!(list[0]["session"], "sess-1");
    let cwd = fs::canonicalize(&env.dir).unwrap();
    assert_eq!(list[0]["cwd"], cwd.to_string_lossy().as_ref());
    env.ok(&["session", "resume", "sess-1", "--wait", "--prompt", "reply again"]);
    assert_eq!(project(&env).1, names, "resumed into other files");
    let acp = env.ok(&["event", "log", "sess-1", "--events", "acp", "--json"]);
    assert_eq!(acp.matches(r#""method":"session/prompt""#).count(), 2, "{acp}");
}

/// Session ids the former `_` for anything unsafe in a file name, or a file
/// system that ignores case or Unicode normalization, put in one file.
const COLLIDING: &[&str] =
    &["a/b", "a_b", "x.acp", "x_acp", "Sess-1", "sess-1", "\u{e9}", "e\u{301}"];

/// `brnr session new` of the fake agent, its session `id`.
fn start_as(env: &Env, id: &str) {
    let out =
        env.brnr(&new_args(&["--wait", "--prompt", "reply hi"])).env("SESSION_ID", id).output();
    let out = out.unwrap();
    assert!(out.status.success(), "start {id:?}: {}", stderr(&out));
}

/// Distinct session ids have distinct transcripts, one after the other in
/// one folder, and each is listed, by its own id (ADR 9.2).
#[test]
fn adr_0009_distinct_ids_never_share_a_transcript() {
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
            records(&dir.join(name)).iter().map(|r| r["session"].clone()).collect();
        assert!(ids.iter().all(|id| *id == ids[0]), "{name}: {ids:?}");
    }
    let list: Value =
        serde_json::from_str(&env.ok(&["session", "list", "--include", "inactive", "--json"]))
            .unwrap();
    let mut listed: Vec<&str> =
        list.as_array().unwrap().iter().map(|r| r["session"].as_str().unwrap()).collect();
    listed.sort();
    let mut want = COLLIDING.to_vec();
    want.sort();
    assert_eq!(listed, want);
    for id in COLLIDING {
        let log = env.ok(&["event", "log", id, "--json"]);
        assert_eq!(log.matches(r#""event":"turn_ended""#).count(), 1, "{id:?}: {log}");
    }
}

/// Distinct session ids are owned apart: each running process holds a lock
/// of its own, and each session is reached in its own (ADR 3.9, ADR 9.2).
#[test]
fn adr_0009_distinct_ids_never_share_a_lock() {
    let env = Env::new("colliding-locks");
    for id in COLLIDING {
        start_as(&env, id);
    }
    // Each lock names its own session and process.
    let mut locked: Vec<(String, Value)> = fs::read_dir(env.dir.join("home/sessions"))
        .unwrap()
        .map(|e| serde_json::from_slice(&fs::read(e.unwrap().path()).unwrap()).unwrap())
        .map(|l: Value| (l["session"].as_str().unwrap().to_owned(), l["pid"].clone()))
        .collect();
    locked.sort_by(|a, b| a.0.cmp(&b.0));
    let mut want = COLLIDING.to_vec();
    want.sort();
    assert_eq!(locked.iter().map(|(s, _)| s.as_str()).collect::<Vec<_>>(), want);
    let list: Value = serde_json::from_str(&env.ok(&["session", "list", "--json"])).unwrap();
    let rows = list.as_array().unwrap();
    assert_eq!(rows.len(), COLLIDING.len(), "{list}");
    for (id, pid) in &locked {
        let row =
            rows.iter().find(|r| r["session"] == id.as_str()).unwrap_or_else(|| panic!("{id:?}"));
        assert_eq!((&row["state"], &row["pid"]), (&Value::from("idle"), pid), "{id:?}: {list}");
        env.ok(&["prompt", "send", id, "reply again"]);
    }
}

/// `log = "events"` leaves out the raw ACP file, and `log --events acp`
/// says there is none; what belongs to no session is still in the host log.
#[test]
fn adr_0005_log_events_leaves_out_the_raw_acp() {
    let env = Env::new("logevents");
    env.write_config("[profiles.default]\nlog = \"events\"\n");
    env.start(&["--wait", "--prompt", "reply hi"]);
    for running in [true, false] {
        let out = env.run(&["event", "log", "sess-1", "--events", "acp,agent_message"]);
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

/// A transcript that ends partway through a record, as a process that died
/// mid-write leaves it, is still its session's (ADR 5.1): `session list` takes
/// the last record it can read, `log` shows everything before the cut and
/// says where it is, and a resume puts its first record on a line of its own.
/// Nothing rewrites what was there.
#[test]
fn adr_0005_a_transcript_cut_short_is_still_its_sessions() {
    let env = Env::new("cutshort");
    env.start(&["--wait", "--prompt", "reply hi"]);
    env.stop();
    assert!(wait_for(Duration::from_secs(15), || env.hosts().is_empty()));
    let (dir, _) = project(&env);
    let file = dir.join("sess-1.jsonl");
    let listed = || -> Value {
        serde_json::from_str(&env.ok(&["session", "list", "--include", "inactive", "--json"]))
            .unwrap()
    };
    // Ending in a newline, as written: the last record is `exited`.
    let whole = fs::read(&file).unwrap();
    assert!(whole.ends_with(b"\n"));
    let exited = records(&file).last().unwrap().clone();
    assert_eq!(exited["event"]["event"], "process_exited");
    assert_eq!(listed()[0]["last_active"], exited["ts"]);

    // A record cut short: the session, as of its last whole record.
    fs::OpenOptions::new()
        .append(true)
        .open(&file)
        .unwrap()
        .write_all(br#"{"ts":"partial"#)
        .unwrap();
    let cut = fs::read(&file).unwrap();
    let list = listed();
    assert_eq!(list.as_array().unwrap().len(), 1, "{list}");
    assert_eq!((&list[0]["session"], &list[0]["last_active"]), (&"sess-1".into(), &exited["ts"]));
    for json in [false, true] {
        let args: &[&str] =
            if json { &["event", "log", "sess-1", "--json"] } else { &["event", "log", "sess-1"] };
        let out = env.run(args);
        assert!(out.status.success(), "{}", stderr(&out));
        assert!(String::from_utf8_lossy(&out.stdout).contains("hi"), "json: {json}");
        let err = stderr(&out);
        assert!(err.contains("ends partway through a record (14 bytes), not shown"), "{err}");
    }
    assert_eq!(fs::read(&file).unwrap(), cut, "reading it changed it");

    // A complete line that isn't a record, last: the same, and said.
    fs::write(&file, [&whole[..], b"not a record\n"].concat()).unwrap();
    assert_eq!(listed()[0]["last_active"], exited["ts"]);
    let n = whole.iter().filter(|&&b| b == b'\n').count() + 1;
    let err = stderr(&env.run(&["event", "log", "sess-1"]));
    assert!(
        err.contains(&format!("sess-1.jsonl:{n} isn't a record brnr can read, not shown")),
        "{err}"
    );

    // Resumed: what was there stays, and the next record is on a line of
    // its own.
    fs::write(&file, &cut).unwrap();
    env.ok(&["session", "resume", "sess-1", "--wait", "--prompt", "reply again"]);
    env.stop();
    assert!(wait_for(Duration::from_secs(15), || env.hosts().is_empty()));
    let after = fs::read(&file).unwrap();
    assert!(after.starts_with(&cut), "the transcript was rewritten");
    assert_eq!(after[cut.len()], b'\n');
    let out = env.run(&["event", "log", "sess-1"]);
    let log = String::from_utf8_lossy(&out.stdout);
    assert!(log.contains("agent: hi") && log.contains("agent: again"), "{log}");
    assert!(stderr(&out).contains(&format!("sess-1.jsonl:{n} isn't a record")), "{}", stderr(&out));
    assert_ne!(listed()[0]["last_active"], exited["ts"]);

    // An empty file says nothing of a session: it isn't one.
    fs::write(dir.join("empty-1.jsonl"), "").unwrap();
    assert_eq!(listed().as_array().unwrap().len(), 1);
    assert!(env.fails(&["event", "log", "empty-1"]).contains("no session empty-1"));
}

/// A process that exits is listed, and holds its sessions, until its
/// transcript has `exited`, so that what reads it once the process has gone
/// (`event log`, `session list`, `session resume`) reads it whole (ADR 5.5). A
/// stalled disk holds that up for 2 s at most; the process itself waits on,
/// without a time limit, and exits once the disk has taken the rest.
#[test]
fn adr_0005_an_exit_is_written_before_the_process_goes() {
    let env = Env::new("exitlog");
    let stall = env.dir.join("stall");
    let env = env.agent("BRNR_TEST_LOG_STALL", &stall.to_string_lossy());
    env.start(&["--wait", "--prompt", "reply hi"]);
    let pid = env.host_pid();
    env.ok(&["event", "log", "sess-1"]);
    fs::write(&stall, "").unwrap();
    env.stop();
    sleep(Duration::from_millis(500));
    assert_eq!(env.hosts().len(), 1, "gone before its transcript was written");
    assert!(wait_for(Duration::from_secs(15), || env.hosts().is_empty()), "the disk held it");
    sleep(Duration::from_secs(1));
    assert!(alive(pid), "exited before its records were written");
    fs::remove_file(&stall).unwrap();
    assert!(wait_for(Duration::from_secs(15), || !alive(pid)), "the disk took them, it stayed");
    let exited = env.ok(&["event", "log", "sess-1", "--json", "--events", "process_exited"]);
    assert!(exited.contains(r#""event":"process_exited""#), "{exited}");
}

/// An exit waits for a stalled disk without a time limit, and a signal to
/// the process gives up on it (ADR 5.5).
#[test]
fn adr_0005_a_signal_gives_up_on_a_stalled_disk_at_exit() {
    let env = Env::new("exitsig");
    let stall = env.dir.join("stall");
    let env = env.agent("BRNR_TEST_LOG_STALL", &stall.to_string_lossy());
    env.start(&[]);
    let pid = env.host_pid();
    env.ok(&["event", "log", "sess-1"]);
    fs::write(&stall, "").unwrap();
    env.stop();
    assert!(wait_for(Duration::from_secs(15), || env.hosts().is_empty()), "the disk held it");
    sleep(Duration::from_secs(2));
    assert!(alive(pid), "gave up on its records unasked");
    kill(pid, libc::SIGTERM);
    assert!(wait_for(Duration::from_secs(5), || !alive(pid)), "a signal didn't give up on them");
    fs::remove_file(&stall).unwrap();
}

/// In the foreground too: the exit waits for a stalled disk, Ctrl-C gives
/// up on it, and stderr says how much of the records the process didn't
/// write. Its exit status is the agent's still (ADR 5.5).
#[test]
fn adr_0005_a_foreground_exit_given_up_on_says_what_it_didnt_write() {
    let env = Env::new("fgstall");
    let stall = env.dir.join("stall");
    let env = env.agent("BRNR_TEST_LOG_STALL", &stall.to_string_lossy());
    let mut fg = env
        .brnr(&new_args(&["--foreground", "--quiet"]))
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let active = || env.ok(&["session", "list", "--include", "active"]).contains("sess-1");
    assert!(wait_for(Duration::from_secs(10), active), "no session");
    let pid = env.host_pid();
    env.ok(&["event", "log", "sess-1"]);
    fs::write(&stall, "").unwrap();
    env.stop();
    assert!(wait_for(Duration::from_secs(15), || env.hosts().is_empty()), "the disk held it");
    sleep(Duration::from_secs(2));
    assert!(fg.try_wait().unwrap().is_none(), "gave up on its records unasked");
    kill(fg.id() as i32, libc::SIGINT);
    assert!(wait_exit(&mut fg, Duration::from_secs(5)), "Ctrl-C didn't give up on them");
    let mut err = String::new();
    fg.stderr.take().unwrap().read_to_string(&mut err).unwrap();
    assert!(fg.wait().unwrap().success(), "{err}");
    let said = format!("brnr: process {pid}: ");
    let told = |l: &str| l.starts_with(&said) && l.contains(" bytes of records not written");
    assert!(err.lines().any(told), "{err}");
    fs::remove_file(&stall).unwrap();
}

/// A panic lets go of the process's sessions before it waits for its
/// records, as an exit does: a stalled disk holds the process, not its
/// sessions (ADR 5.5, ADR 3.7).
#[test]
fn adr_0005_a_panic_lets_go_of_its_sessions_before_it_waits() {
    let env = Env::new("panicstall");
    let stall = env.dir.join("stall");
    let env = env.agent("BRNR_TEST_LOG_STALL", &stall.to_string_lossy());
    let out = env.brnr(&new_args(&[])).env("BRNR_TEST_PANIC", "1").output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let host = env.hosts().remove(0);
    let pid = host["host_pid"].as_i64().unwrap() as i32;
    env.ok(&["event", "log", "sess-1"]);
    let lock = env.dir.join("home/sessions/sess-1.lock");
    assert!(lock.exists(), "sess-1 isn't locked");
    fs::write(&stall, "").unwrap();
    let mut conn = UnixStream::connect(host["socket"].as_str().unwrap()).unwrap();
    writeln!(conn, r#"{{"cmd":"panic","on":"loop"}}"#).unwrap();
    assert!(wait_for(Duration::from_secs(10), || !lock.exists()), "it held sess-1");
    assert!(alive(pid), "exited before its records were written");
    let out = unstalled(&env, &resume_args("sess-1", &[])).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    fs::remove_file(&stall).unwrap();
    assert!(wait_for(Duration::from_secs(15), || !alive(pid)), "the disk took them, it stayed");
}

/// What process `pid` wrote to `events` of each kind, by index in the file.
fn written_by(events: &[Value], pid: i32, kind: &str) -> Vec<usize> {
    let mine = |r: &&Value| r["pid"] == pid && r["event"]["event"] == kind;
    events.iter().enumerate().filter(|(_, r)| mine(r)).map(|(i, _)| i).collect()
}

/// `args`, with the process it starts, if any, writing at once whatever the
/// logger of the one before is doing.
fn unstalled(env: &Env, args: &[&str]) -> Command {
    let mut cmd = env.brnr(args);
    cmd.env_remove("BRNR_TEST_LOG_STALL").stdout(Stdio::piped()).stderr(Stdio::piped());
    cmd
}

/// `cmd`, which closes sess-1 in process `first` while `stall` stands:
/// once the agent has closed it, `first` holds it still and `cmd` waits;
/// then the disk goes again, and `cmd` succeeds.
fn waits_for_the_disk(env: &Env, mut cmd: Command, stall: &Path, first: i32, what: &str) {
    let mut waiting = cmd.spawn().unwrap();
    let closed = || !env.calls_of("session/close").is_empty();
    assert!(wait_for(Duration::from_secs(10), closed), "the agent wasn't asked to close it");
    sleep(Duration::from_millis(300));
    let early = waiting.try_wait().unwrap().is_some();
    let lock = fs::read_to_string(env.dir.join("home/sessions/sess-1.lock")).unwrap_or_default();
    let holder = serde_json::from_str::<Value>(&lock).map(|l| l["pid"].clone()).ok();
    fs::remove_file(stall).unwrap();
    let out = waiting.wait_with_output().unwrap();
    assert!(!early, "{what} before the session's records were written: {}", stderr(&out));
    assert_eq!(holder, Some(first.into()), "let go of before its records were written");
    assert!(out.status.success(), "{}", stderr(&out));
}

/// A session closed in one process (by `session close`, or `session resume
/// --take-over`) while that process's logger is behind, and resumed in a
/// new process: the close is answered once the first process has written
/// its records of it, and only then can the new one take it, so they come
/// first in the transcript, `session_closed` last of them (ADR 5.5).
/// `alone`: it is the first process's last session, so that process stops.
fn closed_then_resumed(name: &str, take_over: bool, alone: bool) {
    let env = Env::new(name);
    let stall = env.dir.join("stall");
    let env = env.agent("BRNR_TEST_LOG_STALL", &stall.to_string_lossy());
    env.start(&[]);
    let first = env.host_pid();
    if !alone {
        env.ok(&["session", "fork", "sess-1"]);
    }
    env.ok(&["event", "log", "sess-1"]);
    fs::write(&stall, "").unwrap();
    let resume = resume_args("sess-1", if take_over { &["--take-over"] } else { &[] });
    if take_over {
        let take_over = unstalled(&env, &resume);
        waits_for_the_disk(&env, take_over, &stall, first, "the take-over went ahead");
    } else {
        let close = unstalled(&env, &["session", "close", "sess-1"]);
        waits_for_the_disk(&env, close, &stall, first, "the close was answered");
        let out = unstalled(&env, &resume).output().unwrap();
        assert!(out.status.success(), "{}", stderr(&out));
    }
    if alone {
        let gone = || env.hosts().iter().all(|h| h["host_pid"] != first);
        assert!(wait_for(Duration::from_secs(15), gone), "the first process didn't stop");
    } else {
        env.ok(&["event", "log", "sess-2"]); // The first process's logger, caught up.
    }
    env.ok(&["event", "log", "sess-1"]); // The second's.

    let events = records(&project(&env).0.join("sess-1.jsonl"));
    let pids = events.iter().map(|r| r["pid"].as_i64().unwrap() as i32);
    let second = pids.clone().find(|&p| p != first).expect("no record of the second process");
    let closed = written_by(&events, first, "session_closed");
    let opened = written_by(&events, second, "session_opened");
    assert_eq!((closed.len(), opened.len()), (1, 1), "{events:#?}");
    // Nothing of the first process's after the second's opening.
    let last = pids.rev().position(|p| p == first).map(|i| events.len() - 1 - i);
    assert_eq!(last, Some(closed[0]), "session_closed isn't the first process's last: {events:#?}");
    assert!(closed[0] < opened[0], "the resume wrote before the close: {events:#?}");
}

#[test]
fn adr_0005_a_close_is_written_before_the_session_is_resumed() {
    closed_then_resumed("closelog", false, false);
}

#[test]
fn adr_0005_a_take_over_is_written_before_the_session_is_resumed() {
    closed_then_resumed("takelog", true, false);
}

/// The same for a process's last session, closed: the process stops, and
/// its close is answered once its records are written, as its exit's are.
#[test]
fn adr_0005_a_last_session_closed_is_written_before_it_is_resumed() {
    closed_then_resumed("lastlog", false, true);
}

/// A session taken over into a running process whose agent loads it, from
/// a process whose logger hasn't yet written the session's first record:
/// the transcript exists by the time it is loaded, so the history the agent
/// replays isn't recorded again (ADR 5.5, ADR 5.6).
#[test]
fn adr_0005_a_take_over_finds_the_transcript_it_continues() {
    let env = Env::new("takereplay");
    let stall = env.dir.join("stall");
    fs::write(&stall, "").unwrap();
    let env = env.agent("BRNR_TEST_LOG_STALL", &stall.to_string_lossy()).agent("NO_RESUME", "1");
    env.start(&[]);
    let first = env.host_pid();
    env.ok(&["session", "fork", "sess-1"]);
    let mut new = unstalled(&env, &new_args(&[]));
    let out = new.env("FIRST_SESSION", "10").output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let pids: Vec<i32> =
        env.hosts().iter().map(|h| h["host_pid"].as_i64().unwrap() as i32).collect();
    let second = pids.into_iter().find(|&p| p != first).unwrap().to_string();
    let take_over = env.brnr(&["session", "resume", "--pid", &second, "sess-1", "--take-over"]);
    waits_for_the_disk(&env, take_over, &stall, first, "the take-over went ahead");
    env.ok(&["event", "log", "sess-2"]); // The first process's logger, caught up.
    env.ok(&["event", "log", "sess-1"]); // The second's.

    let events = records(&project(&env).0.join("sess-1.jsonl"));
    let history: Vec<_> = events.iter().filter(|r| r["event"]["event"] == "history").collect();
    assert_eq!(history.len(), 1, "{events:#?}");
    assert_eq!(history[0]["event"]["recorded"], false, "recorded again: {events:#?}");
    let replayed = events.iter().filter(|r| r["event"]["replayed"] == true).count();
    assert_eq!(replayed, 0, "{events:#?}");
}

/// A close waits for a stalled disk 2 s at most, as an exit does, and the
/// process goes on meanwhile: past that the session is let go of, and its
/// close answered, though its records aren't written yet (ADR 5.5).
#[test]
fn adr_0005_a_close_waits_for_a_stalled_disk_2_s_at_most() {
    let env = Env::new("closestall");
    let stall = env.dir.join("stall");
    let env = env.agent("BRNR_TEST_LOG_STALL", &stall.to_string_lossy());
    env.start(&[]);
    env.ok(&["session", "fork", "sess-1"]);
    env.ok(&["event", "log", "sess-1"]);
    fs::write(&stall, "").unwrap();
    let started = Instant::now();
    let close = unstalled(&env, &["session", "close", "sess-1"]).spawn().unwrap();
    sleep(Duration::from_millis(500));
    let asked = Instant::now();
    let status: Value =
        serde_json::from_str(&env.ok(&["session", "status", "sess-2", "--json"])).unwrap();
    assert!(asked.elapsed() < Duration::from_secs(1), "the process waited for the disk");
    assert_eq!(status["session"], "sess-2", "{status}");
    let out = close.wait_with_output().unwrap();
    let took = started.elapsed();
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(took >= Duration::from_millis(1900), "answered before 2 s: {took:?}");
    assert!(took < Duration::from_secs(10), "answered after {took:?}");
    // Let go of: another process takes it, the disk still stalled.
    let out = unstalled(&env, &resume_args("sess-1", &[])).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    fs::remove_file(&stall).unwrap();
}

/// A session closed in a process and resumed there again, by a bridge,
/// before its records are written: the process keeps its lock, and the
/// close is still answered once `session_closed` is written, here 2 s at
/// most, not as the session opens again (ADR 5.5).
#[test]
fn adr_0005_a_close_resumed_in_its_process_is_answered_once_written() {
    let env = Env::new("closeagain");
    let stall = env.dir.join("stall");
    let env = env.agent("BRNR_TEST_LOG_STALL", &stall.to_string_lossy());
    env.start(&[]);
    let pid = env.pid();
    env.ok(&["session", "fork", "sess-1"]);
    env.ok(&["event", "log", "sess-1"]);
    let mut conn = UnixStream::connect(env.hosts()[0]["socket"].as_str().unwrap()).unwrap();
    conn.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut from_host = BufReader::new(conn.try_clone().unwrap()).lines();
    fs::write(&stall, "").unwrap();
    let started = Instant::now();
    let close = unstalled(&env, &["session", "close", "sess-1"]).spawn().unwrap();
    // Refused until the agent has closed it here.
    let mut resume = || {
        writeln!(conn, r#"{{"cmd":"resume","session":"sess-1","req_id":"r"}}"#).unwrap();
        let answer: Value = serde_json::from_str(&from_host.next().unwrap().unwrap()).unwrap();
        answer["ok"] == true
    };
    assert!(wait_for(Duration::from_secs(10), &mut resume), "sess-1 wasn't resumed here");
    let out = close.wait_with_output().unwrap();
    let took = started.elapsed();
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(took >= Duration::from_millis(1900), "answered before it was written: {took:?}");
    assert!(took < Duration::from_secs(10), "answered after {took:?}");
    let lock = fs::read_to_string(env.dir.join("home/sessions/sess-1.lock")).unwrap();
    let holder = serde_json::from_str::<Value>(&lock).unwrap()["pid"].to_string();
    assert_eq!(holder, pid, "the process let go of the session it resumed");
    fs::remove_file(&stall).unwrap();
}

/// A session closing in a process, whose records aren't written yet: what
/// would open or delete it says it is closing there, not opening, and for
/// how long;
/// `event log` waits for its records and reads them, `session_closed` last,
/// rather than failing (ADR 5.5). The process's status lists it as closing.
#[test]
fn adr_0005_a_closing_session_is_read_once_written_and_said_to_be_closing() {
    let env = Env::new("closing");
    let stall = env.dir.join("stall");
    let env = env.agent("BRNR_TEST_LOG_STALL", &stall.to_string_lossy());
    env.start(&[]);
    let pid = env.pid();
    env.ok(&["session", "fork", "sess-1"]);
    env.ok(&["event", "log", "sess-1"]);
    let mut conn = UnixStream::connect(env.hosts()[0]["socket"].as_str().unwrap()).unwrap();
    conn.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut from_host = BufReader::new(conn.try_clone().unwrap()).lines();
    let mut closing = || {
        writeln!(conn, r#"{{"cmd":"status"}}"#).unwrap();
        let status: Value = serde_json::from_str(&from_host.next().unwrap().unwrap()).unwrap();
        status["closing"] == json!(["sess-1"])
    };
    assert!(!closing(), "closing before it was closed");
    fs::write(&stall, "").unwrap();
    let close = unstalled(&env, &["session", "close", "sess-1"]).spawn().unwrap();
    assert!(wait_for(Duration::from_secs(5), &mut closing), "sess-1 isn't listed as closing");
    let said = format!(
        "sess-1 is closing in process {pid}; it's free once its records are written (2 s at most)"
    );
    let take_over = resume_args("sess-1", &["--take-over"]);
    let here = ["session", "resume", "--pid", &pid, "sess-1"];
    let delete = ["session", "delete", "sess-1"];
    for args in [&resume_args("sess-1", &[])[..], &take_over, &here, &delete] {
        let err = env.fails(args);
        assert!(err.contains(&said), "{args:?}: {err}");
    }
    let mut log = unstalled(&env, &["event", "log", "sess-1"]).spawn().unwrap();
    sleep(Duration::from_millis(300));
    let early = log.try_wait().unwrap().is_some();
    fs::remove_file(&stall).unwrap();
    let out = log.wait_with_output().unwrap();
    assert!(!early, "read before its records were written: {}", stderr(&out));
    assert!(out.status.success(), "{}", stderr(&out));
    let shown = String::from_utf8_lossy(&out.stdout);
    assert!(shown.trim_end().ends_with("session closed (close)"), "{shown}");
    let out = close.wait_with_output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
}

/// What `events`, a session's transcript, has of process `first`, then of
/// the process after it: nothing of the first's comes after the second's.
fn first_then_second(events: &[Value], first: i32) {
    let pids: Vec<_> = events.iter().map(|r| r["pid"].as_i64().unwrap()).collect();
    let last_of_first = pids.iter().rposition(|&p| p == i64::from(first));
    let first_of_second = pids.iter().position(|&p| p != i64::from(first));
    assert!(last_of_first.is_some(), "the first process recorded nothing: {events:#?}");
    assert!(first_of_second.is_some(), "the second process recorded nothing: {events:#?}");
    assert!(last_of_first < first_of_second, "the resume wrote before the first: {events:#?}");
}

/// A bridge's load of a session brnr has no transcript of, which fails
/// after the agent has replayed its history (recorded, ADR 5.6), while the
/// process's logger is behind: the process holds the session's lock, and
/// answers the bridge, once its records are written, as a close's are, and
/// says the session is closing meanwhile. The next process's records come
/// after them, and its load finds the transcript (ADR 5.5).
#[test]
fn adr_0005_a_failed_load_is_written_before_the_session_is_resumed() {
    let env = Env::new("loadfail");
    let stall = env.dir.join("stall");
    let env = env.agent("BRNR_TEST_LOG_STALL", &stall.to_string_lossy());
    let env = env.agent("NO_RESUME", "1").agent("LOAD_FAIL", "1");
    env.start(&[]);
    let first = env.host_pid();
    let pid = first.to_string();
    env.ok(&["event", "log", "sess-1"]);
    let mut conn = UnixStream::connect(env.hosts()[0]["socket"].as_str().unwrap()).unwrap();
    conn.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut from_host = BufReader::new(conn.try_clone().unwrap()).lines();
    let mut closing = || {
        writeln!(conn, r#"{{"cmd":"status"}}"#).unwrap();
        let status: Value = serde_json::from_str(&from_host.next().unwrap().unwrap()).unwrap();
        status["closing"] == json!(["old-1"])
    };
    fs::write(&stall, "").unwrap();
    let load = ["session", "resume", "--pid", &pid, "old-1"];
    let mut load = unstalled(&env, &load).spawn().unwrap();
    assert!(wait_for(Duration::from_secs(5), &mut closing), "old-1 isn't listed as closing");
    let said = format!(
        "old-1 is closing in process {pid}; it's free once its records are written (2 s at most)"
    );
    let err = env.fails(&resume_args("old-1", &[]));
    assert!(err.contains(&said), "{err}");
    assert!(load.try_wait().unwrap().is_none(), "answered before its records were written");
    let lock = fs::read_to_string(env.dir.join("home/sessions/old-1.lock")).unwrap();
    assert_eq!(serde_json::from_str::<Value>(&lock).unwrap()["pid"], first);
    fs::remove_file(&stall).unwrap();
    let out = load.wait_with_output().unwrap();
    assert!(!out.status.success(), "the failed load succeeded");
    assert!(stderr(&out).contains("session/load failed: Can't load"), "{}", stderr(&out));
    // Let go of: the next process loads it, finding the transcript.
    let mut resume = unstalled(&env, &resume_args("old-1", &[]));
    let out = resume.env_remove("LOAD_FAIL").output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    env.ok(&["event", "log", "old-1"]); // The second process's logger, caught up.
    let events = records(&project(&env).0.join("old-1.jsonl"));
    first_then_second(&events, first);
    let history: Vec<_> = events.iter().filter(|r| r["event"]["event"] == "history").collect();
    assert_eq!(history.len(), 1, "{events:#?}");
    assert_eq!(history[0]["event"]["recorded"], false, "recorded again: {events:#?}");
}

/// The same for a headless start whose load fails: the process, which
/// stops, holds the session's lock until its records are written, with its
/// exit's at the latest, so the next process's come after them (ADR 5.5).
#[test]
fn adr_0005_a_start_whose_load_failed_is_written_before_the_session_is_resumed() {
    let env = Env::new("startloadfail");
    let stall = env.dir.join("stall");
    fs::write(&stall, "").unwrap();
    let env = env.agent("BRNR_TEST_LOG_STALL", &stall.to_string_lossy());
    let env = env.agent("NO_RESUME", "1").agent("LOAD_FAIL", "1");
    let err = env.fails(&resume_args("old-1", &[]));
    assert!(err.contains("session/load failed: Can't load"), "{err}");
    // Held while its logger is behind: the lock file is there, the process's.
    let lock = fs::read_to_string(env.dir.join("home/sessions/old-1.lock")).unwrap();
    let first = serde_json::from_str::<Value>(&lock).unwrap()["pid"].as_i64().unwrap() as i32;
    assert_eq!(first, env.host_pid(), "held by another process");
    fs::remove_file(&stall).unwrap();
    assert!(wait_for(Duration::from_secs(10), || env.hosts().is_empty()));
    assert!(!env.dir.join("home/sessions/old-1.lock").exists(), "not let go of");
    let mut resume = unstalled(&env, &resume_args("old-1", &[]));
    let out = resume.env_remove("LOAD_FAIL").output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    env.ok(&["event", "log", "old-1"]);
    let events = records(&project(&env).0.join("old-1.jsonl"));
    first_then_second(&events, first);
    let history: Vec<_> = events.iter().filter(|r| r["event"]["event"] == "history").collect();
    assert_eq!(history.len(), 1, "{events:#?}");
    assert_eq!(history[0]["event"]["recorded"], false, "recorded again: {events:#?}");
}

/// Whether `session` has an events file in any project folder.
fn on_disk(env: &Env, session: &str) -> bool {
    let folders = fs::read_dir(env.dir.join("home/projects")).into_iter().flatten().flatten();
    folders.into_iter().any(|f| f.path().join(format!("{session}.jsonl")).exists())
}

/// What old-1's transcript has of the history its loads replayed: each
/// `history` event's `recorded`, and the replayed messages' texts.
fn old_history(env: &Env) -> (Vec<Value>, Vec<Value>) {
    let events = records(&project(env).0.join("old-1.jsonl"));
    let events = events.iter().map(|r| &r["event"]);
    let history = events.clone().filter(|e| e["event"] == "history");
    let replayed = events.filter(|e| e["replayed"] == true && e["event"] == "agent_message");
    (
        history.map(|e| e["recorded"].clone()).collect(),
        replayed.map(|e| e["text"].clone()).collect(),
    )
}

/// A bridge's load of a session this process served and closed while its
/// logger is behind, the transcript not on disk yet: the process has it
/// all the same, so the history the agent replays is recorded once, by the
/// first load (ADR 5.5, ADR 5.6).
#[test]
fn adr_0005_a_bridges_load_of_a_session_served_here_records_nothing_again() {
    let env = Env::new("servedload");
    let stall = env.dir.join("stall");
    let env = env.agent("BRNR_TEST_LOG_STALL", &stall.to_string_lossy()).agent("NO_RESUME", "1");
    env.start(&[]);
    let pid = env.pid();
    env.ok(&["event", "log", "sess-1"]);
    fs::write(&stall, "").unwrap();
    let load = ["session", "resume", "--pid", &pid, "old-1"];
    for args in [&load[..], &["session", "close", "old-1"], &load] {
        let out = unstalled(&env, args).output().unwrap();
        assert!(out.status.success(), "{args:?}: {}", stderr(&out));
    }
    assert!(!on_disk(&env, "old-1"), "written while the disk stalled");
    fs::remove_file(&stall).unwrap();
    env.ok(&["event", "log", "old-1"]); // The logger, caught up.
    let (history, replayed) = old_history(&env);
    assert_eq!(history, [true, false], "recorded again: {replayed:?}");
    assert_eq!(replayed, ["replayed history"]);
}

/// The same for the editor's load of a session its process served and
/// closed, while the logger is behind (ADR 5.6).
#[test]
fn adr_0005_an_editors_load_of_a_session_served_here_records_nothing_again() {
    let env = Env::new("ed-servedload");
    let stall = env.dir.join("stall");
    let env = env.agent("BRNR_TEST_LOG_STALL", &stall.to_string_lossy());
    let (mut editor, mut to_agent, mut from_agent) = editor(&env);
    fs::write(&stall, "").unwrap();
    load_in_editor(&env, &mut to_agent, &mut from_agent, 2, "old-1");
    let close = request_with_id("3", "session/close", json!({ "sessionId": "old-1" }));
    writeln!(to_agent, "{close}").unwrap();
    assert!(response(&mut from_agent, 3)["result"].is_object());
    load_in_editor(&env, &mut to_agent, &mut from_agent, 4, "old-1");
    assert!(!on_disk(&env, "old-1"), "written while the disk stalled");
    fs::remove_file(&stall).unwrap();
    env.ok(&["event", "log", "old-1"]); // The logger, caught up.
    let (history, replayed) = old_history(&env);
    assert_eq!(history, [true, false], "recorded again: {replayed:?}");
    assert_eq!(replayed, ["replayed history"]);
    let _ = editor.kill();
}

/// The editor's load of a session brnr has no transcript of, which the
/// agent replays and then fails, while the logger is behind: what it
/// recorded of the replay counts in the process as it will on disk, so the
/// editor's next load there records the history no more than one in
/// another process would (ADR 5.5, ADR 5.6, #247).
#[test]
fn adr_0005_a_load_that_replays_and_fails_counts_its_replay() {
    let env = Env::new("ed-failedreplay");
    let stall = env.dir.join("stall");
    let env = env.agent("BRNR_TEST_LOG_STALL", &stall.to_string_lossy()).agent("LOAD_FAIL", "1");
    let (mut editor, mut to_agent, mut from_agent) = editor(&env);
    fs::write(&stall, "").unwrap();
    writeln!(to_agent, "{}", editor_load(&env, 2, "old-1")).unwrap();
    assert!(response(&mut from_agent, 2)["error"].is_object());
    load_in_editor(&env, &mut to_agent, &mut from_agent, 3, "old-1");
    assert!(!on_disk(&env, "old-1"), "written while the disk stalled");
    fs::remove_file(&stall).unwrap();
    env.ok(&["event", "log", "old-1"]); // The logger, caught up.
    let messages = logged(&env, "old-1", "user_message");
    let texts: Vec<(&Value, &Value)> =
        messages.iter().map(|m| (&m["text"], &m["replayed"])).collect();
    assert_eq!(texts, [(&"old question".into(), &true.into())], "recorded again");
    let history = logged(&env, "old-1", "history");
    assert_eq!(history.len(), 1, "{history:?}");
    assert_eq!((&history[0]["updates"], &history[0]["recorded"]), (&3.into(), &false.into()));
    let _ = editor.kill();
}

/// The messages old-1's transcript has, each as its text and whether it
/// was replayed.
fn old_messages(env: &Env) -> Vec<Value> {
    let messages = logged(env, "old-1", "user_message,agent_message");
    messages.iter().map(|m| json!([m["text"], m["replayed"]])).collect()
}

/// The editor's load of a session brnr has no transcript of, which the
/// agent replays and then fails: its last agent message, held still as the
/// load fails, is recorded, replayed, as the rest of the replay is (P3,
/// ADR 5.6, #255).
#[test]
fn adr_0005_an_editors_failed_load_records_its_last_replayed_message() {
    let env = Env::new("ed-failedlast").agent("LOAD_FAIL", "1");
    let (mut editor, mut to_agent, mut from_agent) = editor(&env);
    writeln!(to_agent, "{}", editor_load(&env, 2, "old-1")).unwrap();
    assert!(response(&mut from_agent, 2)["error"].is_object());
    let said = [json!(["old question", true]), json!(["replayed history", true])];
    wait_for(Duration::from_secs(5), || on_disk(&env, "old-1") && old_messages(&env) == said);
    assert_eq!(old_messages(&env), said);
    let _ = editor.kill();
}

/// The same for a bridge's load, answered once the replay is written
/// (P3, ADR 5.5, ADR 5.6, #255).
#[test]
fn adr_0005_a_bridges_failed_load_records_its_last_replayed_message() {
    let env = Env::new("failedlast").agent("NO_RESUME", "1").agent("LOAD_FAIL", "1");
    env.start(&[]);
    let pid = env.pid();
    let err = env.fails(&["session", "resume", "--pid", &pid, "old-1"]);
    assert!(err.contains("session/load failed: Can't load"), "{err}");
    let said = [json!(["old question", true]), json!(["replayed history", true])];
    assert_eq!(old_messages(&env), said);
}

/// The same for a headless start's load, whose process stops (P3, ADR 5.6,
/// #291).
#[test]
fn adr_0005_a_starts_failed_load_records_its_last_replayed_message() {
    let env = Env::new("startfailedlast").agent("NO_RESUME", "1").agent("LOAD_FAIL", "1");
    let err = env.fails(&resume_args("old-1", &[]));
    assert!(err.contains("session/load failed: Can't load"), "{err}");
    assert!(wait_for(Duration::from_secs(10), || env.hosts().is_empty()));
    let said = [json!(["old question", true]), json!(["replayed history", true])];
    assert_eq!(old_messages(&env), said);
}

/// The editor's load of a session brnr has no transcript of, which the
/// agent replays and then fails, while the logger is behind: its process
/// holds the session's lock until the replay it recorded is written, as a
/// bridge's and a start's do, so that a resume elsewhere appends after it
/// (ADR 5.5, ADR 5.6, #291).
#[test]
fn adr_0005_an_editors_failed_load_holds_its_lock_until_written() {
    let env = Env::new("ed-loadheld");
    let stall = env.dir.join("stall");
    let env = env.agent("BRNR_TEST_LOG_STALL", &stall.to_string_lossy()).agent("LOAD_FAIL", "1");
    let (mut editor, mut to_agent, mut from_agent) = editor(&env);
    fs::write(&stall, "").unwrap();
    writeln!(to_agent, "{}", editor_load(&env, 2, "old-1")).unwrap();
    assert!(response(&mut from_agent, 2)["error"].is_object());
    let lock = env.dir.join("home/sessions/old-1.lock");
    let held = fs::read_to_string(&lock).unwrap_or_default();
    let held: Value = serde_json::from_str(&held).unwrap_or_default();
    assert_eq!(held["pid"], env.host_pid(), "let go of before its replay was written");
    fs::remove_file(&stall).unwrap();
    assert!(wait_for(Duration::from_secs(5), || !lock.exists()), "not let go of");
    let _ = editor.kill();
}

/// The editor closing its session while the logger is behind, then resuming
/// it within the 2 s its close holds the lock, a resume the agent fails: the
/// resume took that lock back, and its process holds it still until the
/// close's records are written, as a failed load does (ADR 5.5, #304).
#[test]
fn adr_0005_an_editors_failed_resume_holds_its_lock_until_written() {
    let env = Env::new("ed-resumeheld");
    let stall = env.dir.join("stall");
    let env = env.agent("BRNR_TEST_LOG_STALL", &stall.to_string_lossy()).agent("RESUME_FAIL", "1");
    let (mut editor, mut to_agent, mut from_agent) = open_editor(&env);
    fs::write(&stall, "").unwrap();
    let close = request_with_id("3", "session/close", json!({ "sessionId": "sess-1" }));
    writeln!(to_agent, "{close}").unwrap();
    assert!(response(&mut from_agent, 3)["result"].is_object());
    let resume = editor_load(&env, 4, "sess-1").replace("session/load", "session/resume");
    writeln!(to_agent, "{resume}").unwrap();
    assert!(response(&mut from_agent, 4)["error"].is_object());
    let lock = env.dir.join("home/sessions/sess-1.lock");
    let held = fs::read_to_string(&lock).unwrap_or_default();
    let held: Value = serde_json::from_str(&held).unwrap_or_default();
    assert_eq!(held["pid"], env.host_pid(), "let go of before its close was written");
    fs::remove_file(&stall).unwrap();
    assert!(wait_for(Duration::from_secs(5), || !lock.exists()), "not let go of");
    let _ = editor.kill();
}

/// Context held for a session whose load the agent fails is dropped with
/// its `context_dropped`, `by` `close`, as at any end of the session here,
/// though no `session_closed` follows: the session never opened (P3,
/// ADR 5.2, ADR 5.6, #304).
#[test]
fn adr_0005_a_failed_load_drops_its_held_context() {
    let env = Env::new("ed-loadctx");
    let later = env.dir.join("load-later");
    let env = env.agent("LOAD_LATER", later.to_str().unwrap()).agent("LOAD_FAIL", "1");
    env.write_config("[profiles.default.editor]\nexperimental = [\"context\"]\n");
    let (mut editor, mut to_agent, mut from_agent) = editor(&env);
    writeln!(to_agent, "{}", editor_load(&env, 2, "old-1")).unwrap();
    update(&mut from_agent, "session_info_update");
    env.ok(&["prompt", "send", "old-1", "--context", "held context"]);
    fs::write(&later, "").unwrap();
    assert!(response(&mut from_agent, 2)["error"].is_object());
    let ends = logged(&env, "old-1", "context_dropped,session_closed");
    let ends: Vec<_> = ends.iter().map(|e| (&e["event"], &e["text"], &e["reason"])).collect();
    assert_eq!(ends, [(&"context_dropped".into(), &"held context".into(), &"close".into())]);
    let _ = editor.kill();
}

/// A session served and closed while the disk is too slow for its records,
/// all of them skipped, its close's too: its events file holds only its
/// opening, which isn't a transcript of it, and its process doesn't count
/// one either, so a load of it there records the history, as one in
/// another process would (ADR 10.1, ADR 5.6, #247).
#[test]
fn adr_0005_a_session_closed_with_nothing_recorded_is_no_transcript() {
    let env = Env::new("unrecorded");
    let stall = env.dir.join("stall");
    let env = env.agent("BRNR_TEST_LOG_STALL", &stall.to_string_lossy()).agent("NO_RESUME", "1");
    env.start(&[]);
    let pid = env.pid();
    env.ok(&["event", "log", "sess-1"]);
    fs::write(&stall, "").unwrap();
    // Past the logger's 64 MiB: 1000 messages of 50 kB, each an event and
    // raw ACP. Then the queue is full.
    env.ok(&["prompt", "send", "sess-1", "many 1000 50000"]);
    let out = env.run(&["event", "wait", "sess-1", "--timeout", "60"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let new = env.ok(&["session", "new", "--pid", &pid]);
    assert!(new.contains("sess-2"), "{new}");
    env.ok(&["session", "close", "sess-2"]);
    fs::remove_file(&stall).unwrap();
    env.ok(&["event", "log", "sess-1", "--events", "session_closed"]); // Caught up.
    let events = records(&project(&env).0.join("sess-2.jsonl"));
    let names: Vec<&Value> = events.iter().map(|r| &r["event"]["event"]).collect();
    assert_eq!(names, ["session_opened"], "{events:#?}");
    env.ok(&["session", "resume", "--pid", &pid, "sess-2"]);
    let history = logged(&env, "sess-2", "history");
    assert_eq!(history.len(), 1, "{history:?}");
    assert_eq!((&history[0]["updates"], &history[0]["recorded"]), (&3.into(), &true.into()));
}

/// A disk too slow for brnr's own record neither slows the session nor
/// fills the host's memory: past 64 MiB queued for the logger, records are
/// skipped and counted, and once it catches up a `records_skipped` note says
/// how many, in the session's events file and the host log (ADR 10.1). What was
/// queued is all written. (`BRNR_TEST_LOG_STALL` holds the logger while the
/// file it names exists.)
#[test]
fn adr_0010_a_stalled_disk_skips_records_and_says_so() {
    let env = Env::new("logstall");
    let stall = env.dir.join("stall");
    let env = env.agent("BRNR_TEST_LOG_STALL", &stall.to_string_lossy());
    env.start(&[]);
    let (host, pid) = (env.host_pid(), env.host_pid());
    let projects = env.dir.join("home/projects");
    let opened = || fs::read_dir(&projects).is_ok_and(|mut d| d.next().is_some());
    assert!(wait_for(Duration::from_secs(5), opened), "no transcript");
    let events = project(&env).0.join("sess-1.jsonl");
    fs::write(&stall, "").unwrap();
    // Some 400 MB for the logger: 4000 messages of 50 kB, each an event and
    // raw ACP. The turn runs to its end while nothing is written. (`wait`,
    // as `send --wait` would take every message.)
    env.ok(&["prompt", "send", "sess-1", "many 4000 50000"]);
    let out = env.run(&["event", "wait", "sess-1", "--timeout", "60"]);
    assert!(out.status.success(), "the session waited for the logger: {}", stderr(&out));
    // The logger's 64 MiB, and the agent's 16 MiB on its way, with room.
    assert_holds_less(host, 160 << 20);

    fs::remove_file(&stall).unwrap();
    let noted = || fs::read_to_string(&events).is_ok_and(|t| t.contains("records_skipped"));
    assert!(wait_for(Duration::from_secs(30), noted), "no records_skipped note");
    assert_eq!(env.ok(&["prompt", "send", "sess-1", "--wait", "reply after"]), "after\n");
    env.stop();
    assert!(wait_for(Duration::from_secs(15), || env.hosts().is_empty()));

    let events: Vec<Value> = records(&events).into_iter().map(|r| r["event"].clone()).collect();
    let at = |name: &str| events.iter().position(|e| e["event"] == name).unwrap();
    let note = &events[at("records_skipped")];
    let (count, acp) = (note["count"].as_u64().unwrap(), note["acp_count"].as_u64().unwrap());
    assert!(acp > 0 && count > acp, "{note}");
    assert!(note["since"].as_str().unwrap() <= note["until"].as_str().unwrap(), "{note}");
    // The turn's messages up to the gap, in order, none missing, then the
    // note; what came later was written again.
    let turn: Vec<&str> = events[..at("records_skipped")]
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
    assert!(after > at("records_skipped") && events[after]["text"] == "after");
    assert_eq!(events.iter().filter(|e| e["event"] == "records_skipped").count(), 1);
    // The host log counts every record the gap skipped.
    let host = fs::read_dir(env.dir.join("home/hosts")).unwrap().next().unwrap().unwrap().path();
    let host = records(&host);
    let host_note = host.iter().find(|r| r["event"]["event"] == "records_skipped").unwrap();
    assert!(host_note["event"]["count"].as_u64().unwrap() >= count, "{host_note}");
    // log tells its reader of the gap, whatever --events chose.
    let log = env.ok(&["event", "log", "sess-1", "--events", "user_message"]);
    assert!(log.contains(&format!("{count} records not written ({acp} of them raw ACP)")), "{log}");
    let json = env.ok(&["event", "log", "sess-1", "--json", "--events", "user_message"]);
    assert!(json.contains(r#""event":"records_skipped""#), "{json}");
    // Its gaps alone, a note with what every event has, its session's too
    // (ADR 5.9).
    let gaps = env.ok(&["event", "log", "sess-1", "--json", "--events", "records_skipped"]);
    let gaps: Vec<Value> = gaps.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(gaps.len(), 1, "{gaps:?}");
    let gap = &gaps[0];
    assert_eq!((&gap["event"], &gap["session"]), (&"records_skipped".into(), &"sess-1".into()));
    assert!(gap["ts"].is_string() && gap["host_id"].is_string(), "{gap}");
    assert_eq!(gap["pid"], pid, "{gap}");
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
/// ACP, the `process_started` record) and in `acp` events (ADR 9.1).
#[test]
fn adr_0009_a_profiles_mcp_secrets_are_redacted() {
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
    let args = ["event", "watch", "sess-1", "--events", "acp", "--json"];
    let mut watch = env.brnr(&args).stdout(Stdio::piped()).spawn().unwrap();
    sleep(Duration::from_millis(300));
    env.ok(&["session", "fork", "sess-1"]);
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
    redacted(&started_record(&env)["start_request"]["role"]["headless"]["mcp_servers"]);
    let acp = env.ok(&["event", "log", "sess-1", "--events", "acp", "--json"]);
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
/// servers' secrets redacted (ADR 9.1).
#[test]
fn adr_0009_an_editors_mcp_secrets_are_redacted() {
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
    let args = ["event", "watch", "--pid", &env.pid(), "--events", "acp", "--json"];
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
    // As the editor sent it, but for the host's id (ADR 2.4).
    let mut got = env.calls_of("session/new")[0].clone();
    got["id"] = json!(2);
    assert_eq!(got, new);
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
    let out = env.run(&new_args(&[]));
    assert!(!out.status.success(), "started in a symlinked runtime dir");
    assert!(fs::read_dir(&target).unwrap().next().is_none(), "wrote into the symlink target");
}

/// `brnr acp` is what an editor runs as its agent: ACP over stdio, with the
/// session reachable meanwhile. When the editor goes, the agent goes too.
#[test]
fn adr_0003_acp_is_what_an_editor_runs() {
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
    assert!(env.ok(&["process", "list"]).contains("editor"));
    assert!(env.ok(&["session", "list", "--include", "active"]).contains("sess-1"));
    // A message sent from outside is shown to the editor as a completed
    // tool call; its response (to brnr's own prompt id) is kept from it.
    env.ok(&["prompt", "send", "sess-1", "reply hi"]);
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
    assert!(env.ok(&["event", "log", "sess-1"]).contains("title: Fake session"));
    let err = env.fails(&["acp", "--on-disconnect", "headless", "--", AGENT]);
    assert!(err.contains("unknown option: --on-disconnect"), "{err}");
}

const INITIALIZE: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{}}}"#;

/// `brnr acp` resolves the profile itself: a config error is the editor's
/// to see, on its stderr, and no process starts.
#[test]
fn adr_0003_acp_reports_a_config_error() {
    let env = Env::new("ed-config");
    env.write_config("[profiles.default]\nresponse_timeout = 600\n");
    let out = env.brnr(&["acp", "--", AGENT]).stdin(Stdio::null()).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let err = stderr(&out);
    assert!(err.starts_with("brnr acp: "), "{err}");
    assert!(err.contains("profiles.default: response_timeout is for brnr session new and resume only; it goes under [profiles.default.headless]"), "{err}");
    assert!(env.hosts().is_empty());
}

/// An editor's process gets the profile's shared and editor parts, and the
/// editor's process id and signal mask, in its request.
#[test]
fn adr_0003_acp_hands_over_one_request() {
    let env = Env::new("ed-request");
    env.write_config(
        "[profiles.default]\nstrict = true\nlog = \"events\"\n\n[profiles.default.headless]\nmode = \"plan\"\n\n\
         [profiles.default.editor]\nexperimental = [\"send\", \"permission\"]\n\
         client_capabilities = [\"terminal\"]\n\
         echo_as = \"user_message_chunk\"\nfeatures = [\"shared_sessions\"]\n",
    );
    let (mut editor, _to_agent, _from_agent) = open_editor(&env);
    let request = started_record(&env)["start_request"].clone();
    assert_eq!(request["strict"], true, "{request}");
    assert_eq!(request["log"], "events");
    let part = &request["role"]["editor"];
    assert_eq!(part["proxy_pid"], editor.id(), "{request}");
    assert_eq!(part["experimental"], serde_json::json!(["send", "permission"]));
    assert_eq!(part["client_capabilities"], serde_json::json!(["terminal"]));
    assert_eq!(part["features"], serde_json::json!(["shared_sessions"]));
    assert_eq!(part["echo_as"], "user_message_chunk");
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
    initialized(env.brnr(&args))
}

/// `brnr acp`, as `cmd` has it, initialized.
fn initialized(mut cmd: Command) -> (Child, ChildStdin, BufReader<ChildStdout>) {
    let mut editor = cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
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
fn adr_0005_editor_close_is_an_event() {
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
    let open = env.ok(&["session", "list", "--include", "active"]);
    assert!(!open.contains("sess-1"), "the session is still listed");
    let log = env.ok(&["event", "log", "sess-1", "--json", "--events", "session_closed"]);
    let closed: Value =
        serde_json::from_str(log.lines().next().expect("no session_closed")).unwrap();
    assert_eq!((&closed["session"], &closed["by"]), (&"sess-1".into(), &"editor".into()));
    assert_eq!(closed["reason"], "close", "{closed}");
    assert!(env.ok(&["process", "list"]).contains("editor"));
}

/// A fork the editor makes (`session/fork`) opens as `brnr session fork`'s
/// does: in its own cwd, `session list` and `session status` saying which
/// session it came from, its transcript starting with a `session_opened`
/// that says so and holding the request that made it (ADR 3.12, ADR 5.1).
#[test]
fn adr_0003_an_editors_fork_keeps_its_source() {
    let env = Env::new("ed-fork");
    let (_editor, mut to_agent, mut from_agent) = open_editor(&env);
    let cwd = env.dir.join("fork");
    fs::create_dir(&cwd).unwrap();
    let params = json!({ "sessionId": "sess-1", "cwd": cwd, "mcpServers": [] });
    let fork = json!({ "jsonrpc": "2.0", "id": 3, "method": "session/fork", "params": params });
    writeln!(to_agent, "{fork}").unwrap();
    assert_eq!(response(&mut from_agent, 3)["result"]["sessionId"], "sess-2");

    let rows: Value = serde_json::from_str(&env.ok(&["session", "list", "--json"])).unwrap();
    let row = |id: &str| {
        let found = rows.as_array().unwrap().iter().find(|r| r["session"] == id).cloned();
        found.unwrap_or_else(|| panic!("no {id}: {rows}"))
    };
    let (fork, parent) = (row("sess-2"), row("sess-1"));
    assert_eq!((&fork["forked_from"], &parent["forked_from"]), (&json!("sess-1"), &json!(null)));
    assert_eq!(fork["cwd"], json!(cwd), "{fork}");
    let status: Value =
        serde_json::from_str(&env.ok(&["session", "status", "sess-2", "--json"])).unwrap();
    assert_eq!(status["forked_from"], "sess-1", "{status}");

    // `log` answers once the logger has caught up (ADR 5.5).
    let acp = env.ok(&["event", "log", "sess-2", "--events", "acp", "--json"]);
    let mut acp = acp.lines().map(|l| serde_json::from_str::<Value>(l).unwrap());
    let request = |e: &Value| e["dir"] == "editor->agent" && e["msg"]["method"] == "session/fork";
    assert!(acp.any(|e| request(&e)), "no session/fork in sess-2's transcript");
    let folders = fs::read_dir(env.dir.join("home/projects")).unwrap().flatten();
    let mut files = folders.map(|f| f.path().join("sess-2.jsonl"));
    let text = fs::read_to_string(files.find(|p| p.exists()).expect("no transcript")).unwrap();
    let first: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(first["event"]["event"], "session_opened", "{first}");
    assert_eq!(first["event"]["forked_from"], "sess-1", "{first}");
    assert_eq!(first["event"]["cwd"], json!(cwd), "{first}");
}

/// The editor's `session/delete` of `session`, with `id`.
fn editor_delete(id: u64, session: &str) -> String {
    let params = json!({ "sessionId": session });
    json!({ "jsonrpc": "2.0", "id": id, "method": "session/delete", "params": params }).to_string()
}

/// The editor's `session/delete` goes to the agent as it is. Once the agent
/// has deleted the session, its process records a `session_deleted` by the
/// editor: in the transcript of one no process has open, which stays, and
/// in that of one open there, which then closes (ADR 8.1). One the agent
/// didn't delete records nothing.
#[test]
fn adr_0008_an_editors_delete_is_recorded() {
    let env = Env::new("ed-delete");
    env.start(&["--wait", "--prompt", "reply first"]);
    env.stop();
    assert!(wait_for(Duration::from_secs(15), || env.hosts().is_empty()), "still running");
    let (mut editor, mut to_agent, mut from_agent) = editor(&env);
    writeln!(to_agent, "{}", editor_delete(2, "sess-1")).unwrap();
    assert_eq!(response(&mut from_agent, 2)["result"], json!({}));
    assert_eq!(env.calls_of("session/delete")[0]["params"], json!({ "sessionId": "sess-1" }));
    let deleted = || env.ok(&["event", "log", "sess-1", "--json", "--events", "session_deleted"]);
    assert!(wait_for(Duration::from_secs(5), || !deleted().is_empty()), "not recorded");
    let event: Value = serde_json::from_str(deleted().lines().next().unwrap()).unwrap();
    assert_eq!((&event["session"], &event["by"]), (&"sess-1".into(), &"editor".into()));
    assert_eq!(event["host_id"], editor_process(&env)["host_id"]);
    assert!(env.ok(&["event", "log", "sess-1"]).contains("agent: first"));
    assert!(env.ok(&["session", "list"]).contains("sess-1"), "the transcript went");
    writeln!(to_agent, "{}", editor_delete(3, "gone-1")).unwrap();
    assert!(response(&mut from_agent, 3)["error"].is_object());
    assert_eq!(host_logs(&env).matches(r#""event":"session_deleted""#).count(), 1);
    let _ = editor.kill();

    let env = Env::new("ed-delete-open");
    let (mut editor, mut to_agent, mut from_agent) = open_editor(&env);
    writeln!(to_agent, "{}", editor_delete(3, "sess-1")).unwrap();
    assert_eq!(response(&mut from_agent, 3)["result"], json!({}));
    let names = "session_deleted,session_closed";
    let ends = || env.ok(&["event", "log", "sess-1", "--json", "--events", names]);
    assert!(wait_for(Duration::from_secs(5), || ends().lines().count() == 2), "{}", ends());
    let ends: Vec<Value> = ends().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!((&ends[0]["event"], &ends[0]["by"]), (&"session_deleted".into(), &"editor".into()));
    assert_eq!((&ends[1]["event"], &ends[1]["by"]), (&"session_closed".into(), &"editor".into()));
    assert!(!env.ok(&["session", "list", "--include", "active"]).contains("sess-1"), "still open");
    let _ = editor.kill();
}

/// An editor's deletion of a session no process has open isn't appended to
/// a transcript of it an older brnr wrote (ADR 5.9): brnr doesn't read it,
/// and the host log has the event.
#[test]
fn adr_0005_an_editors_delete_passes_an_older_transcript_over() {
    let env = Env::new("ed-delete-older");
    let cwd = env.dir.to_string_lossy().into_owned();
    // In any folder: an event for a session no process has open goes to
    // each of its files.
    let older = older_transcript(&env, "-a-folder", "old-1", &cwd, false);
    let before = fs::read_to_string(&older).unwrap();
    let (mut editor, mut to_agent, mut from_agent) = editor(&env);
    writeln!(to_agent, "{}", editor_delete(2, "old-1")).unwrap();
    assert_eq!(response(&mut from_agent, 2)["result"], json!({}));
    // Exiting, the process writes all it recorded first (ADR 5.5).
    drop(to_agent);
    assert!(wait_exit(&mut editor, Duration::from_secs(15)), "the editor's process stayed");
    assert!(host_logs(&env).contains(r#""event":"session_deleted""#), "not in the host log");
    assert_eq!(fs::read_to_string(&older).unwrap(), before);
}

/// When the editor goes, what the agent started goes too: its process
/// group, as when it is stopped.
#[test]
fn adr_0003_editor_gone_takes_the_agents_children() {
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
fn adr_0010_editor_that_stops_reading_holds_the_agent_back() {
    let env = Env::new("ed-stall").agent("NOISE", "150000");
    let (_editor, mut to_agent, mut from_agent) = open_editor(&env);
    let host = env.host_pid();
    writeln!(to_agent, "{}", editor_prompt(3, "reply done")).unwrap();
    sleep(Duration::from_secs(32));
    assert!(alive(host), "a stalled editor ended its agent");
    assert_holds_less(host, 64 << 20);
    assert!(env.ok(&["process", "list"]).contains("editor"));
    // The agent went on: its answer comes after everything it wrote.
    line_with(&mut from_agent, "end_turn");
    assert!(alive(host) && env.ok(&["process", "list"]).contains("editor"));
}

/// The agent's stdin likewise: an agent that stops reading holds back what
/// the editor writes, and the host keeps no more of it than its cap. The
/// editor going away is still noticed, and takes the agent with it.
#[test]
fn adr_0010_a_stalled_agent_holds_the_editor_back() {
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

/// The `line_too_long` events in sess-1's transcript (ADR 10.3).
fn too_long(env: &Env) -> Vec<Value> {
    let log = env.ok(&["event", "log", "sess-1", "--json", "--events", "line_too_long"]);
    log.lines().map(|l| serde_json::from_str(l).unwrap()).collect()
}

/// Headless, a line from the agent past the limit isn't read, whether it is
/// one message of 40 MB or 40 MiB of no JSON at all: it is dropped, an event
/// says so, and the session goes on (ADR 10.3).
#[test]
fn adr_0010_headless_an_agent_line_past_the_limit_is_dropped_and_said() {
    let env = Env::new("toolong");
    env.start(&[]);
    for prompt in ["big 40000000", "long 40"] {
        let out = env.run(&["prompt", "send", "sess-1", "--wait", prompt]);
        assert!(out.status.success(), "send {prompt}: {}", stderr(&out));
        assert!(out.stdout.is_empty(), "send {prompt} printed {} bytes", out.stdout.len());
    }
    let events = too_long(&env);
    assert_eq!(events.len(), 2, "{events:?}");
    let dropped = |e: &Value| e["from"] == "agent" && e["relayed"] == false;
    assert!(events.iter().all(dropped), "{events:?}");
    assert_eq!(env.ok(&["prompt", "send", "sess-1", "--wait", "reply after"]), "after\n");
    env.stop();
}

/// A line from the agent with no newline in sight is held back as any other
/// output is when the editor stops reading: the host keeps no more of it
/// than its cap and the most of a line it reads, 48 MiB in all (128 MiB
/// comes, its newline last), and the editor gets it unchanged once it reads
/// again (ADR 10.3).
#[test]
fn adr_0010_a_long_line_to_an_editor_that_stops_reading_is_held_back() {
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
/// unread by the host, and an event says so (ADR 10.3).
#[test]
fn adr_0010_an_editor_line_past_the_limit_goes_to_the_agent_unread() {
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
/// with no newline leaves the host holding none of it (ADR 10.3). (No
/// transcripts: the host log's queue isn't what is measured.)
#[test]
fn adr_0010_the_agents_stderr_without_a_newline_is_bounded() {
    let env = Env::new("errlong");
    env.write_config("[profiles.default]\nlog = false\n");
    env.start(&[]);
    let host = env.host_pid();
    let out = env.run(&["prompt", "send", "sess-1", "--wait", "long 96 stderr"]);
    assert!(out.status.success(), "send: {}", stderr(&out));
    assert_holds_less(host, 64 << 20);
    env.stop();
}

/// What the agent wrote on stderr, as the host log has it (`agent_stderr`):
/// each piece's text.
fn agent_stderr(env: &Env) -> Vec<String> {
    (host_logs(env).lines().map(|l| serde_json::from_str::<Value>(l).unwrap()))
        .filter(|r| r["dir"] == "agent_stderr")
        .map(|r| r["raw"].as_str().unwrap().to_owned())
        .collect()
}

/// What `prompt` (`stderr <n> [<t>]`) left of the agent's stderr in the host
/// log, once the process has exited.
fn agent_stderr_after(name: &str, prompt: &str) -> Vec<String> {
    let env = Env::new(name);
    env.start(&[]);
    env.ok(&["prompt", "send", "sess-1", "--wait", prompt]);
    env.stop();
    let exited = || host_logs(&env).contains(r#""event":"process_exited""#);
    assert!(wait_for(Duration::from_secs(5), exited), "not exited");
    agent_stderr(&env)
}

/// An agent's stderr line that fills a piece exactly is one record: the
/// newline after it ends it, and isn't an empty record of its own.
#[test]
fn adr_0010_an_agents_stderr_line_that_fills_a_piece_is_one_record() {
    let said = agent_stderr_after("errfull", "stderr 65536");
    let pieces: Vec<usize> = said.iter().map(String::len).collect();
    assert_eq!(pieces, [64 << 10]);
}

/// A character that a piece of the agent's stderr would cut goes whole into
/// the next piece: the host log has it, not two replacement characters.
#[test]
fn adr_0010_an_agents_stderr_keeps_a_character_a_piece_would_cut() {
    let said = agent_stderr_after("errutf8", "stderr 65535 \u{e9}");
    let pieces: Vec<usize> = said.iter().map(String::len).collect();
    assert_eq!(pieces, [(64 << 10) - 1, 2]);
    assert_eq!(said[1], "\u{e9}");
}

/// A line the detached process itself writes on stderr reaches the host
/// log (`process_stderr`) in pieces of 64 KiB at most, as the agent's and a
/// bridge's do: the reader holds no more of it than a piece. (A panic's
/// message is what a test can make it write.)
#[test]
fn adr_0010_the_processs_own_stderr_goes_to_the_host_log_in_pieces() {
    let env = Env::new("errhost");
    let out = env.brnr(&new_args(&[])).env("BRNR_TEST_PANIC", "1").output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let host = env.hosts().remove(0);
    let host_pid = host["host_pid"].as_i64().unwrap() as i32;
    let mut conn = UnixStream::connect(host["socket"].as_str().unwrap()).unwrap();
    let text = "x".repeat(70_000);
    writeln!(conn, r#"{{"cmd":"panic","on":"thread","text":"{text}"}}"#).unwrap();
    assert!(wait_for(Duration::from_secs(15), || !alive(host_pid)), "still running");
    let said: Vec<usize> = (host_logs(&env).lines())
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .filter(|r| r["event"]["event"] == "process_stderr")
        .filter_map(|r| r["event"]["text"].as_str().filter(|t| t.starts_with("xxx")).map(str::len))
        .collect();
    assert_eq!(said, [64 << 10, 70_000 - (64 << 10)]);
}

/// A signal to `brnr acp` reaches the agent at once, as it would the agent
/// run directly, while the agent's stdin holds back what the editor writes:
/// it doesn't wait behind it. The agent dies of it, and so `brnr acp` does.
#[test]
fn adr_0003_a_signal_doesnt_wait_behind_a_stalled_agents_stdin() {
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
fn adr_0003_an_editors_process_needs_its_signal_link() {
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
        "proxy_pid": std::process::id(), "sigmask": [], "experimental": [],
        "client_capabilities": [], "echo_as": "tool_call", "features": [],
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
fn adr_0003_non_blocking_stdin_is_waited_on() {
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

/// The editor's process, as `brnr process list` has it.
fn editor_process(env: &Env) -> Value {
    env.hosts().into_iter().find(|h| h["proxy_pid"].is_number()).expect("no editor's process")
}

/// An editor's load of a session a headless process holds is answered by
/// brnr, with an error saying how to release it, and the agent never hears
/// of it; once released, it loads (ADR 3.9).
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
    assert!(message.contains("`brnr session close sess-1`"), "{error}");
    assert!(env.calls_of("session/load").is_empty());
    env.ok(&["session", "close", "sess-1"]);
    writeln!(to_agent, "{}", editor_load(&env, 3, "sess-1")).unwrap();
    assert!(response(&mut from_agent, 3)["result"].is_object());
    let status: Value =
        serde_json::from_str(&env.ok(&["session", "status", "sess-1", "--json"])).unwrap();
    assert_eq!(status["held_by"], editor_process(&env)["host_pid"]);
    let _ = editor.kill();
}

/// An editor's load of a session held by a process started with another
/// runtime directory, which this one's commands can't reach, is refused the
/// same way, saying which `BRNR_DIR` releases it (ADR 3.9).
#[test]
fn adr_0003_an_editors_load_held_from_another_runtime_dir_is_refused() {
    let env = Env::new("ed-tworun");
    env.start(&[]);
    let headless = env.pid();
    let mut cmd = env.brnr(&["acp", "--", AGENT]);
    cmd.env("BRNR_DIR", env.dir.join("run2"));
    let (mut editor, mut to_agent, mut from_agent) = initialized(cmd);
    writeln!(to_agent, "{}", editor_load(&env, 2, "sess-1")).unwrap();
    let error = response(&mut from_agent, 2)["error"].clone();
    let message = error["message"].as_str().unwrap_or_default();
    assert!(message.contains(&format!("sess-1 is running in brnr process {headless}")), "{error}");
    let release = format!("`BRNR_DIR={} brnr session close sess-1`", env.dir.join("run").display());
    assert!(message.contains(&release), "{error}");
    assert!(env.calls_of("session/load").is_empty());
    let _ = editor.kill();
}

/// The command an editor's refused load is given runs as shown, its runtime
/// directory one shell word whatever is in it (a space, a quote): it
/// releases the session, which then loads (ADR 3.9).
#[test]
fn adr_0003_the_release_command_runs_as_shown() {
    for (i, name) in ["run 2", "run'2"].into_iter().enumerate() {
        let env = Env::new(&format!("ed-word{i}"));
        let run = env.dir.join(name);
        env.start_in(&run);
        let (mut editor, mut to_agent, mut from_agent) = editor(&env);
        writeln!(to_agent, "{}", editor_load(&env, 2, "sess-1")).unwrap();
        let error = response(&mut from_agent, 2)["error"].clone();
        let message = error["message"].as_str().unwrap_or_default();
        let command =
            format!("BRNR_DIR={} brnr session close sess-1", quoted(&run.to_string_lossy()));
        assert!(message.contains(&format!("`{command}`")), "{error}");
        let out = env.sh(&command);
        assert!(out.status.success(), "{name}: {}", stderr(&out));
        writeln!(to_agent, "{}", editor_load(&env, 3, "sess-1")).unwrap();
        assert!(response(&mut from_agent, 3)["result"].is_object(), "{name}");
        let _ = editor.kill();
    }
}

/// An editor's load of a session another editor holds, whose process has
/// another runtime directory, is refused with that directory and the
/// `BRNR_DIR` that reaches it, as one a headless process holds is (ADR 3.9).
#[test]
fn adr_0003_an_editors_load_held_by_an_editor_elsewhere_names_its_runtime_dir() {
    let env = Env::new("ed-edrun");
    let run = env.dir.join("run 'e");
    let mut cmd = env.brnr(&["acp", "--", AGENT]);
    cmd.env("BRNR_DIR", &run);
    let (mut holder, mut to_holder, mut from_holder) = initialized(cmd);
    let new = format!(
        r#"{{"jsonrpc":"2.0","id":2,"method":"session/new","params":{{"cwd":{:?},"mcpServers":[]}}}}"#,
        env.dir.display().to_string()
    );
    writeln!(to_holder, "{new}").unwrap();
    assert_eq!(response(&mut from_holder, 2)["result"]["sessionId"], "sess-1");
    let hosts = env.hosts_in(&run);
    let pid = hosts.iter().find(|h| h["proxy_pid"].is_number()).unwrap()["host_pid"].clone();
    let (mut editor, mut to_agent, mut from_agent) = editor(&env);
    writeln!(to_agent, "{}", editor_load(&env, 2, "sess-1")).unwrap();
    let error = response(&mut from_agent, 2)["error"].clone();
    let message = error["message"].as_str().unwrap_or_default();
    let said = format!(
        "session sess-1 is open in another editor (brnr process {pid}), with runtime directory {} \
         (BRNR_DIR={} reaches it); close it there first",
        run.display(),
        quoted(&run.to_string_lossy())
    );
    assert!(message.contains(&said), "{error}");
    assert!(env.calls_of("session/load").is_empty());
    let _ = editor.kill();
    let _ = holder.kill();
}

/// An editor's session whose lock can't be taken (a directory where its lock
/// file goes) is passed through, as the proxy passes on what it can't be
/// sure of: the editor opens and loads sessions as it would without brnr,
/// its process keeps the transcript, and `status` says the session isn't
/// locked. A headless resume of it is still refused (ADR 3.9).
#[test]
fn adr_0003_an_editors_session_that_cant_be_locked_is_passed_through() {
    use std::os::unix::fs::DirBuilderExt;
    let env = Env::new("ed-nolock");
    let unlockable = |session: &str| {
        let lock = env.dir.join(format!("home/sessions/{session}.lock"));
        fs::DirBuilder::new().recursive(true).mode(0o700).create(&lock).unwrap();
        lock
    };
    let lock = unlockable("sess-1");
    let (mut editor, mut to_agent, mut from_agent) = open_editor(&env);
    writeln!(to_agent, "{}", editor_prompt(3, "reply from the editor")).unwrap();
    assert!(response(&mut from_agent, 3)["result"].is_object());
    let status: Value =
        serde_json::from_str(&env.ok(&["session", "status", "sess-1", "--json"])).unwrap();
    let why = status["lock_error"].as_str().unwrap_or_default().to_owned();
    assert!(why.starts_with(&format!("{}: ", lock.display())), "{status}");
    assert_eq!(status["held_by"], Value::Null);
    let text = env.ok(&["session", "status", "sess-1"]);
    assert!(text.contains(&format!("not locked: {why}; nothing stops")), "{text}");
    assert!(env.ok(&["event", "log", "sess-1"]).contains("from the editor"));
    let pid = editor_process(&env)["host_pid"].to_string();
    let err = env.fails(&resume_args("sess-1", &[]));
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
/// owner (ADR 3.9, ADR 3.10).
#[test]
fn adr_0003_shared_sessions_let_an_editor_load_a_held_session() {
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
    let status: Value =
        serde_json::from_str(&env.ok(&["session", "status", "sess-1", "--json"])).unwrap();
    assert_eq!(
        (status["pid"].to_string(), status["held_by"].to_string()),
        (headless.clone(), headless)
    );
    assert_eq!(status["shared_by"], serde_json::json!([shared["host_pid"]]));
    let text = env.ok(&["session", "status", "sess-1"]);
    assert!(text.contains(&format!("shared by process {}", shared["host_pid"])), "{text}");
    // Its turn isn't in the session's transcript, but in its host log.
    assert!(!env.ok(&["event", "log", "sess-1"]).contains("from the editor"));
    let host_log = fs::read_to_string(shared["host_log"].as_str().unwrap()).unwrap();
    assert!(host_log.contains("from the editor") && host_log.contains("session_shared"));
    let _ = editor.kill();
}

/// `--take-over` doesn't take a session from an editor whose profile doesn't
/// enable the experimental `close` (ADR 7.1), and the process started for it
/// goes, having done nothing.
#[test]
fn adr_0007_take_over_from_an_editor_is_refused() {
    let env = Env::new("ed-takeover");
    let (mut editor, _to_agent, _from_agent) = open_editor(&env);
    let pid = env.pid();
    let err = env.fails(&resume_args("sess-1", &["--take-over"]));
    let refused = format!(
        "sess-1 is running in process {pid}, an editor's: `close` on an editor's session is \
         experimental"
    );
    assert!(err.contains(&refused), "{err}");
    assert!(env.calls_of("session/close").is_empty());
    assert_eq!(env.hosts().len(), 1, "a second process started");
    let _ = editor.kill();
}

// ---- a load's history ----------------------------------------------------

/// What FULL_HISTORY has the agent replay on `session/load`, by kind.
const REPLAY: [&str; 5] = [
    "user_message_chunk",
    "agent_message_chunk",
    "session_info_update",
    "tool_call",
    "available_commands_update",
];

/// The editor's `session/load` of `session`, with id `id`: the kinds of
/// the updates the editor got before its answer, which must be a result.
fn load_in_editor(
    env: &Env,
    to_agent: &mut ChildStdin,
    from_agent: &mut BufReader<ChildStdout>,
    id: u64,
    session: &str,
) -> Vec<String> {
    writeln!(to_agent, "{}", editor_load(env, id, session)).unwrap();
    let mut kinds = Vec::new();
    let mut line = String::new();
    loop {
        line.clear();
        assert!(from_agent.read_line(&mut line).unwrap() > 0, "no answer to {id}");
        let msg: Value = serde_json::from_str(&line).unwrap();
        if msg["id"] == id {
            assert!(msg["result"].is_object(), "{msg}");
            return kinds;
        }
        if msg["method"] == "session/update" && msg["params"]["sessionId"] == session {
            kinds.push(msg["params"]["update"]["sessionUpdate"].as_str().unwrap().to_owned());
        }
    }
}

/// `session`'s events named `names` in its transcript, as JSON.
fn logged(env: &Env, session: &str, names: &str) -> Vec<Value> {
    let log = env.ok(&["event", "log", session, "--json", "--events", names]);
    log.lines().map(|l| serde_json::from_str(l).unwrap()).collect()
}

/// `session status <session> --json`.
fn status_of(env: &Env, session: &str) -> Value {
    serde_json::from_str(&env.ok(&["session", "status", session, "--json"])).unwrap()
}

/// The commands the agent has announced for `session`, by name, as
/// `prompt commands` has them.
fn command_names(env: &Env, session: &str) -> Vec<String> {
    let listed: Value =
        serde_json::from_str(&env.ok(&["prompt", "commands", session, "--json"])).unwrap();
    let commands = listed["commands"].as_array().cloned().unwrap_or_default();
    commands.iter().map(|c| c["command"].as_str().unwrap_or_default().to_owned()).collect()
}

/// The editor loading a session again, open in its process already, has its
/// replay taken as a replay: the editor gets it as the agent sent it, and the
/// transcript, which has the history from the first load, gets nothing of it
/// again but a `history` event, whether or not the logger has written it
/// yet (a stalled disk holds it here). A tool call the history left
/// unfinished isn't counted as running (ADR 5.6).
#[test]
fn adr_0005_an_editors_load_of_a_session_open_here_records_nothing_again() {
    let env = Env::new("ed-reload");
    let stall = env.dir.join("stall");
    let env = env.agent("FULL_HISTORY", "1").agent("BRNR_TEST_LOG_STALL", &stall.to_string_lossy());
    let (mut editor, mut to_agent, mut from_agent) = editor(&env);
    fs::write(&stall, "").unwrap();
    for id in [2, 3] {
        let replay = load_in_editor(&env, &mut to_agent, &mut from_agent, id, "old-1");
        assert_eq!(replay, REPLAY, "the editor's replay of load {id}");
    }
    fs::remove_file(&stall).unwrap();
    let messages = logged(&env, "old-1", "user_message,agent_message");
    let texts: Vec<(&Value, &Value)> =
        messages.iter().map(|m| (&m["text"], &m["replayed"])).collect();
    let once = [(&"old question".into(), &true.into()), (&"replayed history".into(), &true.into())];
    assert_eq!(texts, once, "{messages:?}");
    let tools = logged(&env, "old-1", "tool_call");
    assert_eq!(tools.len(), 1, "{tools:?}");
    let history = logged(&env, "old-1", "history");
    let said: Vec<(&Value, &Value)> =
        history.iter().map(|h| (&h["updates"], &h["recorded"])).collect();
    assert_eq!(said, [(&5.into(), &true.into()), (&5.into(), &false.into())], "{history:?}");
    let status = status_of(&env, "old-1");
    assert_eq!(status["tools"], json!([]), "{status}");
    assert_eq!(status["title"], "Loaded session", "{status}");
    assert_eq!(command_names(&env, "old-1"), ["replayed"]);
    let _ = editor.kill();
}

/// The editor's load of a session its process doesn't have open records
/// the agent's replay only if brnr has no transcript of the session, marked
/// `replayed`; either way the editor gets the replay as the agent sent it, a
/// `history` event says which, and what the replay says the session is now
/// (its title, its commands) is kept (ADR 5.6).
#[test]
fn adr_0005_an_editors_load_records_the_history_only_without_a_transcript() {
    let env = Env::new("ed-load").agent("FULL_HISTORY", "1");
    env.start(&["--wait", "--prompt", "reply first"]);
    env.stop();
    assert!(wait_for(Duration::from_secs(15), || env.hosts().is_empty()));
    let (mut editor, mut to_agent, mut from_agent) = editor(&env);

    // sess-1 has a transcript: the replay stays out of it.
    let replay = load_in_editor(&env, &mut to_agent, &mut from_agent, 2, "sess-1");
    assert_eq!(replay, REPLAY);
    let all = env.ok(&["event", "log", "sess-1", "--events", "all"]);
    for replayed in ["old question", "replayed history", "Loaded session", "Old tool"] {
        assert!(!all.contains(replayed), "{replayed:?} recorded:\n{all}");
    }
    let history = logged(&env, "sess-1", "history");
    assert_eq!(history.len(), 1, "{history:?}");
    assert_eq!((&history[0]["updates"], &history[0]["recorded"]), (&5.into(), &false.into()));
    let said = "history: 5 updates replayed by the agent, not recorded: brnr's transcript has";
    assert!(env.ok(&["event", "log", "sess-1"]).contains(said));
    let status = status_of(&env, "sess-1");
    assert_eq!(status["tools"], json!([]), "{status}");
    assert_eq!(status["title"], "Loaded session", "{status}");
    assert_eq!(command_names(&env, "sess-1"), ["replayed"]);

    // old-1 has none: the replay is recorded, marked.
    let replay = load_in_editor(&env, &mut to_agent, &mut from_agent, 3, "old-1");
    assert_eq!(replay, REPLAY);
    let messages = logged(&env, "old-1", "user_message,agent_message");
    let texts: Vec<(&Value, &Value)> =
        messages.iter().map(|m| (&m["text"], &m["replayed"])).collect();
    let once = [(&"old question".into(), &true.into()), (&"replayed history".into(), &true.into())];
    assert_eq!(texts, once, "{messages:?}");
    let history = logged(&env, "old-1", "history");
    assert_eq!(history.len(), 1, "{history:?}");
    assert_eq!((&history[0]["updates"], &history[0]["recorded"]), (&5.into(), &true.into()));
    assert!(history[0].get("replayed").is_none(), "{}", history[0]);
    let acp = env.ok(&["event", "log", "old-1", "--events", "acp", "--json"]);
    assert!(acp.contains("old question"), "{acp}");
    let status = status_of(&env, "old-1");
    assert_eq!(status["tools"], json!([]), "{status}");
    assert_eq!(status["title"], "Loaded session", "{status}");
    assert_eq!(command_names(&env, "old-1"), ["replayed"]);
    let _ = editor.kill();
}

/// The editor loading a session again before the agent has answered its
/// first load: both loads reach the agent, and one replay covers them, as
/// which load an update is for can't be told. Every update either replays
/// is recorded, marked `replayed`, so the transcript has the history once
/// per load, and one `history` event says how many updates the two
/// replayed, and that they were two. No tool call of the history is left
/// running (ADR 5.6).
#[test]
fn adr_0005_overlapping_loads_in_an_editor_have_one_history() {
    let env = Env::new("ed-overlap");
    let gate = env.dir.join("load-gate");
    let env = env.agent("FULL_HISTORY", "1").agent("LOAD_GATE", gate.to_str().unwrap());
    let (mut editor, mut to_agent, mut from_agent) = editor(&env);
    writeln!(to_agent, "{}", editor_load(&env, 2, "old-1")).unwrap();
    // The first load's replay is in, and its answer waits for the gate.
    update(&mut from_agent, "available_commands_update");
    writeln!(to_agent, "{}", editor_load(&env, 3, "old-1")).unwrap();
    let both_sent = || {
        let acp = env.ok(&["event", "log", "old-1", "--events", "acp", "--json"]);
        acp.matches(r#""method":"session/load""#).count() == 2
    };
    assert!(wait_for(Duration::from_secs(5), both_sent), "the second load wasn't sent");
    fs::write(&gate, "").unwrap();
    for id in [2, 3] {
        assert!(response(&mut from_agent, id)["result"].is_object(), "load {id}");
    }
    assert_eq!(env.calls_of("session/load").len(), 2);
    // Everything replayed is recorded marked: none of it as live.
    let replayed = logged(&env, "old-1", "user_message,agent_message,tool_call,session_changed");
    let unmarked: Vec<&Value> = replayed.iter().filter(|e| e["replayed"] != true).collect();
    assert!(unmarked.is_empty(), "{unmarked:?}");
    let texts: Vec<&Value> = replayed.iter().filter_map(|e| e.get("text")).collect();
    let twice = ["old question", "replayed history", "old question", "replayed history"];
    assert_eq!(texts, twice, "{replayed:?}");
    // The second replay's t0 is the first's, still running: that call
    // again, not a second start (ADR 5.3).
    let tools = logged(&env, "old-1", "tool_call");
    assert_eq!(tools.len(), 1, "{tools:?}");
    let history = logged(&env, "old-1", "history");
    assert_eq!(history.len(), 1, "{history:?}");
    let said = (&history[0]["updates"], &history[0]["recorded"], &history[0]["loads"]);
    assert_eq!(said, (&10.into(), &true.into(), &2.into()), "{}", history[0]);
    let text = env.ok(&["event", "log", "old-1"]);
    let said = "history: 10 updates replayed by the agent in 2 loads, recorded";
    assert!(text.contains(said), "{text}");
    let status = status_of(&env, "old-1");
    assert_eq!(status["tools"], json!([]), "{status}");
    let _ = editor.kill();
}

/// A turn that ends while a load replays, its tool call still running, ends
/// that call, and the history's isn't running either once the load is
/// answered: `status` has none (ADR 8.2, ADR 5.6). (No transcript, so the
/// replay is recorded, its tool call too.)
#[test]
fn adr_0008_a_turn_ending_in_a_replay_leaves_no_tool_call_running() {
    let env = Env::new("ed-replay-turn").agent("FULL_HISTORY", "1").agent("LOAD_ENDS_TURN", "1");
    env.write_config("[profiles.default]\nlog = false\n");
    let (mut editor, mut to_agent, mut from_agent) = open_editor(&env);
    writeln!(to_agent, "{}", editor_prompt(3, "hang tool")).unwrap();
    let running = || status_of(&env, "sess-1")["tools"].as_array().is_some_and(|t| !t.is_empty());
    assert!(wait_for(Duration::from_secs(10), running), "{}", status_of(&env, "sess-1"));
    writeln!(to_agent, "{}", editor_load(&env, 4, "sess-1")).unwrap();
    assert_eq!(response(&mut from_agent, 3)["result"]["stopReason"], "cancelled");
    assert!(response(&mut from_agent, 4)["result"].is_object());
    let status = status_of(&env, "sess-1");
    assert_eq!((&status["state"], &status["tools"]), (&"idle".into(), &json!([])), "{status}");
    let _ = editor.kill();
}

/// A replay that finishes a tool call running before it, then starts one
/// with its id, leaves that one, the history's, not running (ADR 8.2).
#[test]
fn adr_0008_a_replays_call_with_a_finished_calls_id_is_not_running() {
    let env = Env::new("ed-replay-reuse").agent("REUSED_TOOL", "1");
    env.write_config("[profiles.default]\nlog = false\n");
    let (mut editor, mut to_agent, mut from_agent) = open_editor(&env);
    writeln!(to_agent, "{}", editor_prompt(3, "hang tool")).unwrap();
    let running = || status_of(&env, "sess-1")["tools"].as_array().is_some_and(|t| !t.is_empty());
    assert!(wait_for(Duration::from_secs(10), running), "{}", status_of(&env, "sess-1"));
    writeln!(to_agent, "{}", editor_load(&env, 4, "sess-1")).unwrap();
    assert!(response(&mut from_agent, 4)["result"].is_object());
    let status = status_of(&env, "sess-1");
    assert_eq!(status["tools"], json!([]), "{status}");
    writeln!(to_agent, "{CANCEL}").unwrap();
    let _ = editor.kill();
}

/// The editor resuming a session whose load the agent hasn't answered yet:
/// the resume opens it, so the load failing after doesn't close it again
/// (ADR 5.6).
#[test]
fn adr_0005_a_failed_load_leaves_a_resumed_session_open() {
    let env = Env::new("ed-load-resume");
    let later = env.dir.join("load-later");
    let env = env.agent("LOAD_LATER", later.to_str().unwrap()).agent("LOAD_FAIL", "1");
    let (mut editor, mut to_agent, mut from_agent) = editor(&env);
    writeln!(to_agent, "{}", editor_load(&env, 2, "old-1")).unwrap();
    let resume = editor_load(&env, 3, "old-1").replace("session/load", "session/resume");
    writeln!(to_agent, "{resume}").unwrap();
    assert!(response(&mut from_agent, 3)["result"].is_object());
    fs::write(&later, "").unwrap();
    assert!(response(&mut from_agent, 2)["error"].is_object());
    let status = status_of(&env, "old-1");
    assert_eq!(status["held_by"], editor_process(&env)["host_pid"], "{status}");
    let prompt = json!([{ "type": "text", "text": "reply still here" }]);
    let params = json!({ "sessionId": "old-1", "prompt": prompt });
    writeln!(to_agent, "{}", request_with_id("4", "session/prompt", params)).unwrap();
    assert!(response(&mut from_agent, 4)["result"].is_object());
    let log = env.ok(&["event", "log", "old-1"]);
    assert!(!log.contains("session closed"), "{log}");
    assert!(log.contains("agent: still here"), "{log}");
    let _ = editor.kill();
}

/// Nor does a load that fails leave its history's tool call running, in a
/// session a resume keeps open: `status` has none (ADR 8.2, ADR 5.6).
#[test]
fn adr_0008_a_failed_loads_history_leaves_no_tool_call_running() {
    let env = Env::new("ed-load-fail-tool");
    let later = env.dir.join("load-later");
    let env = env.agent("LOAD_LATER", later.to_str().unwrap()).agent("LOAD_FAIL", "1");
    let env = env.agent("FULL_HISTORY", "1");
    let (mut editor, mut to_agent, mut from_agent) = editor(&env);
    writeln!(to_agent, "{}", editor_load(&env, 2, "old-1")).unwrap();
    let resume = editor_load(&env, 3, "old-1").replace("session/load", "session/resume");
    writeln!(to_agent, "{resume}").unwrap();
    assert!(response(&mut from_agent, 3)["result"].is_object());
    fs::write(&later, "").unwrap();
    assert!(response(&mut from_agent, 2)["error"].is_object());
    let status = status_of(&env, "old-1");
    assert_eq!(status["tools"], json!([]), "{status}");
    let _ = editor.kill();
}

/// The editor closing a session whose load the agent hasn't answered yet:
/// the agent answers the close, then the load, which succeeds. Its last
/// answer wins, so the session is open again, and a `session_reopened` after
/// the `session_closed` says so (ADR 5.6).
#[test]
fn adr_0005_a_load_answered_after_its_close_reopens_the_session() {
    let env = Env::new("ed-load-close");
    let later = env.dir.join("load-later");
    let env = env.agent("LOAD_LATER", later.to_str().unwrap());
    let (mut editor, mut to_agent, mut from_agent) = editor(&env);
    writeln!(to_agent, "{}", editor_load(&env, 2, "old-1")).unwrap();
    let close = request_with_id("3", "session/close", json!({ "sessionId": "old-1" }));
    writeln!(to_agent, "{close}").unwrap();
    assert!(response(&mut from_agent, 3)["result"].is_object());
    fs::write(&later, "").unwrap();
    assert!(response(&mut from_agent, 2)["result"].is_object());
    let status = status_of(&env, "old-1");
    assert_eq!(status["held_by"], editor_process(&env)["host_pid"], "{status}");
    let ends = logged(&env, "old-1", "session_closed,session_reopened");
    let ends: Vec<(&Value, &Value)> = ends.iter().map(|e| (&e["event"], &e["by"])).collect();
    let said = [
        (&"session_closed".into(), &"editor".into()),
        (&"session_reopened".into(), &"editor".into()),
    ];
    assert_eq!(ends, said);
    let log = env.ok(&["event", "log", "old-1"]);
    assert!(
        log.contains("session reopened (editor): its load was answered after its close"),
        "{log}"
    );
    let _ = editor.kill();
}

/// The editor's load of old-1 (id 2), then its close (id 3) once the
/// replay is in, answered before the load: the gate `later` still holds the
/// load's answer.
fn load_then_close(env: &Env, to_agent: &mut ChildStdin, from_agent: &mut BufReader<ChildStdout>) {
    writeln!(to_agent, "{}", editor_load(env, 2, "old-1")).unwrap();
    update(from_agent, "session_info_update");
    let close = request_with_id("3", "session/close", json!({ "sessionId": "old-1" }));
    writeln!(to_agent, "{close}").unwrap();
    assert!(response(from_agent, 3)["result"].is_object());
}

/// A load answered after its session's close, which replayed more after the
/// close: the updates before it are recorded, those after can't be, the
/// session's files being closed, and all are counted. Once the load
/// succeeds, the `session_reopened` is followed by the load's `history`,
/// which says how many of its updates were recorded (ADR 5.6).
#[test]
fn adr_0005_a_loads_history_is_counted_across_its_sessions_close() {
    let env = Env::new("ed-load-close-count");
    let later = env.dir.join("load-later");
    let env = env.agent("LOAD_LATER", later.to_str().unwrap()).agent("LATE_HISTORY", "1");
    let (mut editor, mut to_agent, mut from_agent) = editor(&env);
    load_then_close(&env, &mut to_agent, &mut from_agent);
    fs::write(&later, "").unwrap();
    // The editor gets the late update all the same (ADR 3.1).
    let late = update(&mut from_agent, "agent_message_chunk");
    assert_eq!(late["content"]["text"], "late history");
    assert!(response(&mut from_agent, 2)["result"].is_object());
    let events = logged(&env, "old-1", "session_closed,session_reopened,history");
    let names: Vec<&Value> = events.iter().map(|e| &e["event"]).collect();
    assert_eq!(names, ["session_closed", "session_reopened", "history"], "{events:?}");
    let history = &events[2];
    let said = (&history["updates"], &history["recorded"], &history["recorded_updates"]);
    assert_eq!(said, (&4.into(), &true.into(), &3.into()), "{history}");
    assert_eq!(history["loads"], 1, "{history}");
    let text = env.ok(&["event", "log", "old-1"]);
    assert!(text.contains("history: 4 updates replayed by the agent, 3 recorded"), "{text}");
    assert!(!text.contains("late history"), "recorded after the close:\n{text}");
    let _ = editor.kill();
}

/// The editor's load of a session brnr has no transcript of, which the
/// agent refuses, replaying nothing: the session closes again, its events
/// file holding only its opening, which isn't a transcript of it. The
/// editor's next load records the history the agent replays (ADR 5.6, #191).
#[test]
fn adr_0005_an_editors_load_after_a_failed_load_records_the_history() {
    let env = Env::new("ed-load-refused").agent("LOAD_REFUSED", "1");
    let (mut editor, mut to_agent, mut from_agent) = editor(&env);
    writeln!(to_agent, "{}", editor_load(&env, 2, "old-1")).unwrap();
    assert!(response(&mut from_agent, 2)["error"].is_object());
    // The failed load's opening is written before the next load looks.
    let projects = env.dir.join("home/projects");
    let opened = || {
        let folders = fs::read_dir(&projects).into_iter().flatten().flatten();
        folders
            .map(|f| f.path().join("old-1.jsonl"))
            .any(|p| fs::read_to_string(p).is_ok_and(|t| t.contains("session_opened")))
    };
    assert!(wait_for(Duration::from_secs(5), opened), "the failed load's opening isn't written");
    let replay = load_in_editor(&env, &mut to_agent, &mut from_agent, 3, "old-1");
    assert_eq!(replay, REPLAY[..3]);
    let messages = logged(&env, "old-1", "user_message,agent_message");
    let texts: Vec<(&Value, &Value)> =
        messages.iter().map(|m| (&m["text"], &m["replayed"])).collect();
    let once = [(&"old question".into(), &true.into()), (&"replayed history".into(), &true.into())];
    assert_eq!(texts, once, "{messages:?}");
    let history = logged(&env, "old-1", "history");
    assert_eq!(history.len(), 1, "{history:?}");
    assert_eq!((&history[0]["updates"], &history[0]["recorded"]), (&3.into(), &true.into()));
    let _ = editor.kill();
}

/// A load that fails after its session's close leaves it closed, with no
/// `history`, and nothing of its count stays: the next load's `history` is
/// its own (ADR 5.6).
#[test]
fn adr_0005_a_load_that_fails_after_its_sessions_close_has_no_history() {
    let env = Env::new("ed-load-close-fail");
    let later = env.dir.join("load-later");
    let env = env.agent("LOAD_LATER", later.to_str().unwrap()).agent("LOAD_FAIL", "1");
    let (mut editor, mut to_agent, mut from_agent) = editor(&env);
    load_then_close(&env, &mut to_agent, &mut from_agent);
    fs::write(&later, "").unwrap();
    assert!(response(&mut from_agent, 2)["error"].is_object());
    let open = env.ok(&["session", "list", "--include", "active"]);
    assert!(!open.contains("old-1"), "{open}");
    assert!(logged(&env, "old-1", "session_reopened,history").is_empty());
    // Loaded again, it has a history of its own.
    writeln!(to_agent, "{}", editor_load(&env, 4, "old-1")).unwrap();
    assert!(response(&mut from_agent, 4)["result"].is_object());
    let history = logged(&env, "old-1", "history");
    assert_eq!(history.len(), 1, "{history:?}");
    let said = (&history[0]["updates"], &history[0]["loads"], &history[0]["recorded"]);
    assert_eq!(said, (&3.into(), &1.into(), &false.into()), "{}", history[0]);
    assert_eq!(history[0]["recorded_updates"], 0, "{}", history[0]);
    let _ = editor.kill();
}

/// Two loads of a session that overlap, and its close answered before
/// either: the first answer reopens it, and the one replay goes on until the
/// second, whose `history` covers both loads and all their updates (ADR 5.6).
#[test]
fn adr_0005_overlapping_loads_closed_meanwhile_have_one_history() {
    let env = Env::new("ed-loads-close");
    let later = env.dir.join("load-later");
    let env = env.agent("LOAD_LATER", later.to_str().unwrap());
    let (mut editor, mut to_agent, mut from_agent) = editor(&env);
    for id in [2, 3] {
        writeln!(to_agent, "{}", editor_load(&env, id, "old-1")).unwrap();
        update(&mut from_agent, "session_info_update");
    }
    let close = request_with_id("4", "session/close", json!({ "sessionId": "old-1" }));
    writeln!(to_agent, "{close}").unwrap();
    assert!(response(&mut from_agent, 4)["result"].is_object());
    fs::write(&later, "").unwrap();
    let mut answered = 0;
    while answered < 2 {
        let msg = message(&mut from_agent, |m| m["id"] == 2 || m["id"] == 3);
        assert!(msg["result"].is_object(), "{msg}");
        answered += 1;
    }
    let events = logged(&env, "old-1", "session_closed,session_reopened,history");
    let names: Vec<&Value> = events.iter().map(|e| &e["event"]).collect();
    assert_eq!(names, ["session_closed", "session_reopened", "history"], "{events:?}");
    let history = &events[2];
    let said = (&history["updates"], &history["loads"], &history["recorded_updates"]);
    assert_eq!(said, (&6.into(), &2.into(), &6.into()), "{history}");
    assert_eq!(history["recorded"], true, "{history}");
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
fn adr_0002_a_lone_surrogate_is_read() {
    let env = Env::new("ed-lone");
    let (_editor, mut to_agent, mut from_agent) = open_editor(&env);
    writeln!(to_agent, "{}", editor_prompt(3, "odd surrogate")).unwrap();
    let request = line_with(&mut from_agent, "session/request_permission");
    assert!(request.contains(r#""title": "Edit \ud83d""#), "{request}");
    let pending: Value =
        serde_json::from_str(&env.ok(&["permission", "requests", "sess-1", "--json"])).unwrap();
    assert_eq!(pending[0]["title"], "Edit \u{fffd}", "{pending}");
    writeln!(to_agent, "{ALLOW}").unwrap();
    line_with(&mut from_agent, "end_turn");
}

/// So is nesting deeper than serde_json's 128: the host reads a request
/// 20000 deep, and carries on once it is answered.
#[test]
fn adr_0002_deep_nesting_is_read() {
    let env = Env::new("ed-deep");
    let (_editor, mut to_agent, mut from_agent) = open_editor(&env);
    writeln!(to_agent, "{}", editor_prompt(3, "odd deep 20000")).unwrap();
    let request = line_with(&mut from_agent, "session/request_permission");
    assert!(request.contains(&"[".repeat(20000)), "the request was cut");
    let state = || {
        let status: Value =
            serde_json::from_str(&env.ok(&["session", "status", "sess-1", "--json"])).unwrap();
        status["state"].clone()
    };
    assert_eq!(state(), "waiting");
    writeln!(to_agent, "{ALLOW}").unwrap();
    line_with(&mut from_agent, "end_turn");
    assert_eq!(state(), "idle");
}

/// The response timeout answers a request a million deep (2 MB, the main
/// thread's 8 MiB took 100000) on a stack that takes it, as an event is
/// handled: its turn is cancelled, as a request with no `reject_once` has
/// it (ADR 8.1), and the process lives on.
#[test]
fn adr_0002_a_deep_request_that_times_out_is_answered() {
    let env = Env::new("deep-timeout");
    env.write_config("[profiles.default.headless]\nresponse_timeout = 3\n");
    env.start(&[]);
    let pid = env.host_pid();
    env.ok(&["prompt", "send", "sess-1", "odd deep 1000000"]);
    let answer = || {
        let ours = |c: &Value| c["id"] == "perm-1" && c.get("method").is_none();
        env.calls().into_iter().find(ours).map(|c| c["result"]["outcome"].clone())
    };
    // Read, and waiting for an answer, before the timeout. A slow runner's
    // process can be busy with the line past `status`'s 5 seconds: it isn't
    // answering then, and is asked again. The timeout starts once the line
    // is read, so the process answers that before it fires.
    let waiting = || {
        let out = env.run(&["session", "status", "sess-1", "--json"]);
        let status = || serde_json::from_slice::<Value>(&out.stdout).unwrap();
        out.status.success() && status()["state"] == "waiting"
    };
    assert!(wait_for(Duration::from_secs(15), waiting), "never asked");
    let done = || answer().is_some() || !alive(pid);
    assert!(wait_for(Duration::from_secs(15), done), "unanswered");
    assert!(alive(pid), "the process died");
    assert_eq!(answer().unwrap(), json!({ "outcome": "cancelled" }));
    let ended = || env.ok(&["event", "log", "sess-1"]).contains("turn ended: ");
    assert!(wait_for(Duration::from_secs(5), ended), "the turn didn't end");
    // The request too, which `event log` read as no record (ADR 2.5).
    let out = env.run(&["event", "log", "sess-1"]);
    let (logged, err) = (String::from_utf8_lossy(&out.stdout), stderr(&out));
    assert!(err.is_empty(), "{err}");
    assert!(logged.contains("permission p1: Edit src/lib.rs"), "{logged}");
    assert!(logged.contains("permission p1 cancelled, by timeout"), "{logged}");
    env.stop();
}

/// `brnr permission requests` reads the host's answer as the host reads the
/// agent's line: a request nested past serde_json's 128 is listed, in text
/// and `--json`, and one a million deep doesn't overflow brnr's stack.
#[test]
fn adr_0002_a_deep_request_is_listed() {
    for depth in [200, 1_000_000] {
        let env = Env::new(&format!("ed-deep-{depth}"));
        let (_editor, mut to_agent, mut from_agent) = open_editor(&env);
        writeln!(to_agent, "{}", editor_prompt(3, &format!("odd deep {depth}"))).unwrap();
        line_with(&mut from_agent, "session/request_permission");
        let pending: Value =
            serde_json::from_str(&env.ok(&["permission", "requests", "sess-1", "--json"])).unwrap();
        assert_eq!(pending[0]["request"], "p1", "{depth}: {pending}");
        assert_eq!(pending[0]["title"], "Edit src/lib.rs", "{depth}: {pending}");
        let text = env.ok(&["permission", "requests", "sess-1"]);
        assert!(text.contains("p1") && text.contains("Edit src/lib.rs"), "{depth}: {text}");
        writeln!(to_agent, "{ALLOW}").unwrap();
        line_with(&mut from_agent, "end_turn");
    }
}

/// With no session, `permission requests` and `elicitation requests` ask
/// every process: a request 200,000 deep (its title, or its message), more
/// than the main stack takes, read from one, is let go of on a stack that
/// takes it when a process asked after it fails, which is said, not a stack
/// overflow (#301).
#[test]
fn adr_0002_a_deep_request_is_let_go_of_when_another_process_fails() {
    let env = Env::new("deep-fail");
    env.start(&[]);
    env.ok(&["session", "fork", "sess-1"]);
    // A slow runner's process can be busy with the deep line past the 5 s a
    // command gives it to answer: it is asked again (#276).
    let asked = |session: &str, what: &str| {
        let wait = ["event", "wait", session, "--for", what, "--timeout", "10"];
        assert!(wait_for(Duration::from_secs(40), || env.run(&wait).status.success()), "{what}");
    };
    env.ok(&["prompt", "send", "sess-1", "odd title 200000"]);
    asked("sess-1", "permission");
    env.ok(&["prompt", "send", "sess-2", "elicit deep 200000"]);
    asked("sess-2", "elicitation");
    let gone_files = |pid: u32| {
        for ext in ["json", "sock"] {
            let _ = fs::remove_file(env.dir.join(format!("run/{pid}.{ext}")));
        }
    };
    for what in ["permission", "elicitation"] {
        let mut tries = 0;
        let (out, mut gone) = loop {
            let mut gone = gone_after_status(&env);
            let out = env.run(&[what, "requests"]);
            tries += 1;
            if !stderr(&out).contains("not answering") || tries == 5 {
                break (out, gone);
            }
            let _ = gone.kill();
            let _ = gone.wait();
            gone_files(gone.id());
            sleep(Duration::from_secs(1));
        };
        let err = stderr(&out);
        assert_eq!((out.status.code(), out.status.signal()), (Some(1), None), "{what}: {err}");
        assert!(err.contains(&format!("brnr: process {}: ", gone.id())), "{what}: {err}");
        assert!(gone.wait().unwrap().success(), "{what}");
        gone_files(gone.id());
    }
    env.ok(&["permission", "allow", "sess-1", "p1"]);
    env.stop();
}

/// A process, as far as `discover` can tell, started after any other, so
/// asked last: it answers its status, then exits, and what else it is asked
/// fails. Its metadata and socket are the caller's to remove.
fn gone_after_status(env: &Env) -> Child {
    let script = r#"
import json, os, socket, sys
run, pid = sys.argv[1], os.getpid()
path = f"{run}/{pid}.sock"
listener = socket.socket(socket.AF_UNIX)
listener.bind(path)
listener.listen()
meta = {"id": str(pid), "host_pid": pid, "socket": path, "started": "2999-01-01T00:00:00Z"}
with open(f"{run}/{pid}.json", "w") as f:
    json.dump(meta, f)
print("ready", flush=True)
conn, _ = listener.accept()
conn.makefile().readline()
conn.sendall(b'{"ok":true,"sessions":[]}\n')
"#;
    let mut child = Command::new("python3")
        .args(["-c", script])
        .arg(env.dir.join("run"))
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut ready = String::new();
    BufReader::new(child.stdout.as_mut().unwrap()).read_line(&mut ready).unwrap();
    assert_eq!(ready, "ready\n");
    child
}

/// So do `permission show`, and `elicitation requests` and `show`, whose
/// answer has every request waiting in the process: a request nested past
/// 128 is shown, in text and `--json`, and an elicitation beside one, or
/// beside one a million deep, is listed and shown. Shown, a request is
/// indented: a million deep would be a terabyte of it, so it is shown at
/// 200 only.
#[test]
fn adr_0002_a_deep_request_is_shown() {
    for depth in [200, 1_000_000] {
        let env = Env::new(&format!("deep-show-{depth}"));
        env.start(&[]);
        env.ok(&["session", "fork", "sess-1"]);
        env.ok(&["prompt", "send", "sess-2", "elicit form"]);
        env.ok(&["event", "wait", "sess-2", "--for", "elicitation", "--timeout", "10"]);
        env.ok(&["prompt", "send", "sess-1", &format!("odd deep {depth}")]);
        // A million deep is slow to read on a loaded runner: time enough.
        env.ok(&["event", "wait", "sess-1", "--for", "permission", "--timeout", "60"]);
        if depth == 200 {
            let shown = env.ok(&["permission", "show", "sess-1", "p1"]);
            assert!(shown.starts_with("p1, session sess-1\nEdit src/lib.rs\n"), "{shown}");
            assert!(shown.contains("input:\n  [\n    [\n"), "{shown}");
            let shown = env.ok(&["permission", "show", "sess-1", "p1", "--json"]);
            assert!(shown.contains(r#""request": "p1""#), "{shown}");
            assert!(shown.matches('[').count() >= depth, "{} bytes", shown.len());
        }
        let listed = env.ok(&["elicitation", "requests"]);
        let row = |l: &str| l.split_whitespace().take(2).eq(["sess-2", "e1"]);
        assert!(listed.lines().any(row), "{depth}: {listed}");
        let shown = env.ok(&["elicitation", "show", "sess-2", "e1"]);
        assert!(shown.starts_with("e1, session sess-2\nTell me about you\n"), "{depth}: {shown}");
        let shown = env.ok(&["elicitation", "show", "sess-2", "e1", "--json"]);
        let shown: Value = serde_json::from_str(&shown).unwrap();
        assert_eq!((&shown["request"], &shown["mode"]), (&"e1".into(), &"form".into()), "{depth}");
        env.ok(&["permission", "allow", "sess-1", "p1"]);
        env.stop();
    }
}

/// Its stdout, read as it comes (a deep event fills a pipe), once it exits.
fn read_out(child: &mut Child) -> std::thread::JoinHandle<String> {
    let mut stdout = child.stdout.take().unwrap();
    std::thread::spawn(move || {
        let mut out = String::new();
        stdout.read_to_string(&mut out).unwrap();
        out
    })
}

/// What follows a session reads its events as the host reads the agent's
/// line: a request nested past serde_json's 128 is announced by `prompt
/// send --wait`, shown by `event watch`, as text and `--json`, and waited on
/// by `event wait`, as it comes and once it is pending; and one a million
/// deep doesn't overflow brnr's stack. `event wait --json` indents what it
/// prints: a request a million deep would be a terabyte of it, so it is
/// waited on so at 200 only.
#[test]
fn adr_0002_a_deep_request_is_followed() {
    for depth in [200, 1_000_000] {
        let env = Env::new(&format!("deep-follow-{depth}"));
        env.start(&[]);
        let watch = |json: &[&str]| {
            let args = [&["event", "watch", "sess-1", "--events", "permission_request"], json];
            let mut watch = env.brnr(&args.concat()).stdout(Stdio::piped()).spawn().unwrap();
            let out = read_out(&mut watch);
            (watch, out)
        };
        let (mut watch_json, shown_json) = watch(&["--json"]);
        let (mut watch_text, shown_text) = watch(&[]);
        // A million deep is slow to read on a loaded runner: time enough.
        let args = ["event", "wait", "sess-1", "--for", "permission", "--timeout", "60"];
        let mut wait = env.brnr(&args).stdout(Stdio::piped()).spawn().unwrap();
        let waited = read_out(&mut wait);
        sleep(Duration::from_millis(300));
        let prompt = format!("odd deep {depth}");
        let args = ["prompt", "send", "sess-1", "--wait", "--timeout", "60", &prompt];
        let mut send = env.brnr(&args).stderr(Stdio::piped()).spawn().unwrap();
        let mut err = BufReader::new(send.stderr.take().unwrap()).lines().map_while(Result::ok);
        let notice = err.find(|l| l.contains("waiting for approval") || l.contains("timed out"));
        let notice = notice.unwrap_or_default();
        assert!(notice.contains("approval p1: Edit src/lib.rs"), "{depth}: {notice}");
        assert!(wait.wait().unwrap().success(), "{depth}: event wait failed");
        let waited = waited.join().unwrap();
        assert_eq!(waited.trim(), "approval p1: Edit src/lib.rs", "{depth}");
        // Pending by now: from the process's answer, not an event.
        let args = ["event", "wait", "sess-1", "--for", "permission", "--timeout", "5"];
        assert_eq!(env.ok(&args).trim(), "approval p1: Edit src/lib.rs", "{depth}");
        if depth == 200 {
            let waited = env.ok(&[&args[..], &["--json"]].concat());
            assert!(waited.contains(r#""request": "p1""#), "{} bytes", waited.len());
        }
        env.ok(&["permission", "allow", "sess-1", "p1"]);
        let sent = send.wait().unwrap();
        let said: Vec<String> = err.collect();
        assert!(sent.success(), "{depth}: prompt send {sent}: {said:?}");
        env.stop();
        for watch in [&mut watch_json, &mut watch_text] {
            assert!(wait_exit(watch, Duration::from_secs(15)), "{depth}: watch didn't end");
            assert!(watch.wait().unwrap().success(), "{depth}: watch failed");
        }
        let shown = shown_json.join().unwrap();
        assert!(shown.contains(&"[".repeat(depth)), "{depth}: watch showed {} bytes", shown.len());
        let shown = shown_text.join().unwrap();
        assert!(
            shown.contains("permission p1: Edit src/lib.rs"),
            "{depth}: watch showed {shown:?}"
        );
    }
}

/// So does `event notify`: its command runs for a request nested past 128,
/// with the event whole on its stdin, from the socket and from `--stdin`,
/// and for one a million deep too.
#[test]
fn adr_0002_a_deep_request_is_notified() {
    for depth in [200, 1_000_000] {
        let env = Env::new(&format!("deep-notify-{depth}"));
        env.start(&[]);
        let deep = "[".repeat(depth);
        let notified = |out: &Path| fs::read_to_string(out).is_ok_and(|t| t.contains(&deep));
        let out = env.dir.join("notified");
        let script = format!("cat >> {}", quoted(&out.to_string_lossy()));
        let args = ["event", "notify", "sess-1", "--events", "permission_request", "--"];
        let mut notify = env.brnr(&args).args(["sh", "-c", &script]).spawn().unwrap();
        sleep(Duration::from_millis(300));
        env.ok(&["prompt", "send", "sess-1", &format!("odd deep {depth}")]);
        assert!(wait_for(Duration::from_secs(10), || notified(&out)), "{depth}: not run");
        env.ok(&["permission", "allow", "sess-1", "p1"]);
        env.stop();
        assert!(wait_exit(&mut notify, Duration::from_secs(15)), "{depth}: notify didn't end");
        assert!(notify.wait().unwrap().success(), "{depth}: notify failed");

        let out = env.dir.join("notified-stdin");
        let script = format!("cat >> {}", quoted(&out.to_string_lossy()));
        let request = format!(
            r#"{{"event":"permission_request","request":"p1","tool_call":{deep}{}}}"#,
            "]".repeat(depth)
        );
        // A line that isn't an event, as deep, is passed over.
        let other = format!(r#"{{"note":{deep}{}}}"#, "]".repeat(depth));
        let exited = r#"{"event":"process_exited","exit_status":{"code":0}}"#;
        let input = format!("{other}\n{request}\n{exited}\n");
        let args = ["event", "notify", "--stdin", "--events", "permission_request", "--"];
        let args = [&args[..], &["sh", "-c", &script]].concat();
        let ran = env.run_with_stdin(&args, input.as_bytes());
        assert!(ran.status.success(), "{depth}: {}", stderr(&ran));
        assert!(wait_for(Duration::from_secs(10), || notified(&out)), "{depth}: --stdin: not run");
    }
}

/// And so does catching up (ADR 10.2): a watcher cut off before a request
/// nested past 128, or a million deep, gets it from the transcript.
#[test]
fn adr_0002_a_deep_event_is_caught_up() {
    for depth in [200, 1_000_000] {
        let env = Env::new(&format!("deep-catchup-{depth}"));
        env.start(&[]);
        let args = ["event", "watch", "sess-1", "--json", "--events", "all"];
        let mut watch =
            env.brnr(&args).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        let stdout = BufReader::new(watch.stdout.take().unwrap());
        let lines = std::thread::spawn(move || stdout.lines().map_while(Result::ok).collect());
        sleep(Duration::from_millis(300));
        kill(watch.id() as i32, libc::SIGSTOP);
        for _ in 0..2 {
            env.ok(&["prompt", "send", "sess-1", "--wait", "big 6000000"]);
        }
        let dropped = || host_logs(&env).contains(r#""event":"peer_dropped""#);
        let cut = wait_for(Duration::from_secs(10), dropped);
        // Asked for once it is cut off: the request can only reach it from
        // the transcript.
        if cut {
            env.ok(&["prompt", "send", "sess-1", &format!("odd deep {depth}")]);
            // A slow runner's process can be busy with the deep line past the
            // 5 seconds a command gives it to answer, or past the wait's 10:
            // it is asked again (#276). The request stays pending until it is
            // answered, so a later wait sees it at once.
            let asked = || {
                let wait = ["event", "wait", "sess-1", "--for", "permission", "--timeout", "10"];
                env.run(&wait).status.success()
            };
            assert!(wait_for(Duration::from_secs(40), asked), "{depth}: never asked");
            env.ok(&["permission", "allow", "sess-1", "p1"]);
        }
        kill(watch.id() as i32, libc::SIGCONT);
        assert!(cut, "{depth}: the watcher wasn't cut off");
        env.stop();
        assert!(wait_exit(&mut watch, Duration::from_secs(30)), "{depth}: watch didn't end");
        let mut err = String::new();
        watch.stderr.take().unwrap().read_to_string(&mut err).unwrap();
        assert!(watch.wait().unwrap().success(), "{depth}: watch failed: {err}");
        let lines: Vec<String> = lines.join().unwrap();
        let request = lines.iter().find(|l| l.contains(r#""event":"permission_request""#));
        let request = request.expect("no permission_request");
        let raw = format!(r#""rawInput":{}"#, "[".repeat(depth));
        assert!(request.contains(&raw), "{depth}: {} bytes", request.len());
    }
}

/// And so does `event log` (ADR 5.1): a request nested past 128, or a
/// million deep, is shown from the transcript as `event watch` showed it,
/// in text and `--json`, where it was said not to be a record brnr can read.
#[test]
fn adr_0002_a_deep_request_is_logged() {
    for depth in [200, 1_000_000] {
        let env = Env::new(&format!("deep-log-{depth}"));
        env.start(&[]);
        env.ok(&["prompt", "send", "sess-1", &format!("odd deep {depth}")]);
        // A slow runner's process can be busy with the deep line past the
        // 10 seconds of a wait: it is asked again, as it stays pending.
        let asked = || {
            let wait = ["event", "wait", "sess-1", "--for", "permission", "--timeout", "10"];
            env.run(&wait).status.success()
        };
        assert!(wait_for(Duration::from_secs(40), asked), "{depth}: never asked");
        let args = ["event", "log", "sess-1", "--events", "permission_request"];
        for json_out in [false, true] {
            let out = env.run(&[&args[..], if json_out { &["--json"] } else { &[] }].concat());
            let (shown, err) = (String::from_utf8_lossy(&out.stdout), stderr(&out));
            assert!(out.status.success() && err.is_empty(), "{depth}: {err}");
            let want = match json_out {
                true => format!(r#""rawInput":{}"#, "[".repeat(depth)),
                false => "permission p1: Edit src/lib.rs".to_owned(),
            };
            assert!(shown.contains(&want), "{depth}: event log showed {} bytes", shown.len());
        }
        env.ok(&["permission", "allow", "sess-1", "p1"]);
        env.stop();
    }
}

/// A request that isn't JSON at all can't be tracked, but its answer isn't
/// held back: an answer to an id the host doesn't know goes to the agent.
#[test]
fn adr_0002_an_answer_the_host_cant_place_reaches_the_agent() {
    let env = Env::new("ed-garbled");
    let (_editor, mut to_agent, mut from_agent) = open_editor(&env);
    writeln!(to_agent, "{}", editor_prompt(3, "odd garbled")).unwrap();
    let request = line_with(&mut from_agent, "session/request_permission");
    assert!(serde_json::from_str::<Value>(&request).is_err(), "{request}");
    assert_eq!(env.ok(&["permission", "requests", "sess-1", "--json"]).trim(), "[]");
    writeln!(to_agent, "{ALLOW}").unwrap();
    line_with(&mut from_agent, "end_turn");
}

/// When a turn is cancelled from outside, the host answers the agent's
/// pending request itself; the editor's late answer to it is dropped, so the
/// agent isn't answered twice, and the editor is told so in the session
/// (ADR 2.5).
#[test]
fn adr_0002_a_late_answer_to_a_cancelled_request_is_dropped() {
    let env = Env::new("ed-late");
    let (_editor, mut to_agent, mut from_agent) = experimental_editor(&env, &["cancel"]);
    writeln!(to_agent, "{}", editor_prompt(3, "perm edit")).unwrap();
    line_with(&mut from_agent, "session/request_permission");
    env.ok(&["prompt", "cancel", "sess-1"]);
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

// ---- experimental actions on an editor's session (ADR 7.1) ---------------

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
        let status: Value =
            serde_json::from_str(&env.ok(&["session", "status", "sess-1", "--json"])).unwrap();
        status["state"] == "idle"
    };
    assert!(wait_for(Duration::from_secs(5), idle), "sess-1 is still busy");
}

/// Each action on an editor's session is refused unless its profile enables
/// it, saying what to add; observing it is always allowed, and the agent
/// hears of none of it.
#[test]
fn adr_0007_experimental_actions_are_refused_without_opt_in() {
    let env = Env::new("ex-refused");
    let (_editor, mut to_agent, mut from_agent) = open_editor(&env);
    writeln!(to_agent, "{}", editor_prompt(3, "perm edit")).unwrap();
    line_with(&mut from_agent, "session/request_permission");
    let actions: [(&[&str], &str); 10] = [
        (&["prompt", "send", "sess-1", "hi"], "send"),
        (&["prompt", "send", "sess-1", "--context", "hi"], "context"),
        (&["queue", "clear", "sess-1", "--context"], "context"),
        (&["prompt", "cancel", "sess-1"], "cancel"),
        (&["permission", "allow", "sess-1", "p1"], "permission"),
        (&["permission", "reject", "sess-1", "p1"], "permission"),
        (&["config", "set", "sess-1", "--mode", "plan"], "config"),
        (&["config", "set", "sess-1", "--model", "large"], "config"),
        (&["config", "set", "sess-1", "--option", "model=large"], "config"),
        (&["session", "close", "sess-1"], "close"),
    ];
    for (args, action) in actions {
        let err = env.fails(args);
        let says = format!(
            "`{action}` on an editor's session is experimental; enable it with `experimental = \
             [\"{action}\"]` under `[profiles.default.editor]`"
        );
        assert!(err.contains(&says), "{args:?}: {err}");
    }
    assert!(env.fails(&["session", "fork", "sess-1"]).contains("the editor owns this process"));
    let observing: [&[&str]; 9] = [
        &["session", "status", "sess-1"],
        &["permission", "requests", "sess-1"],
        &["permission", "show", "sess-1", "p1"],
        &["queue", "list", "sess-1"],
        &["config", "get", "sess-1", "--mode"],
        &["config", "get", "sess-1", "--model"],
        &["config", "get", "sess-1"],
        &["prompt", "commands", "sess-1"],
        &["event", "log", "sess-1"],
    ];
    for args in observing {
        env.ok(args);
    }
    // show says where it can be answered, and why not here.
    let show = env.ok(&["permission", "show", "sess-1", "p1"]);
    assert!(show.contains("p1, session sess-1, waiting in the editor"), "{show}");
    let why = "answer it in the editor (`permission` on an editor's session is experimental";
    assert!(show.contains(why), "{show}");
    for method in ["session/cancel", "session/set_mode", "session/set_config_option"] {
        assert!(env.calls_of(method).is_empty(), "the agent got {method}");
    }
    writeln!(to_agent, "{ALLOW}").unwrap();
    line_with(&mut from_agent, "end_turn");
    let answers: Vec<Value> = env.calls().into_iter().filter(|c| c["id"] == "perm-1").collect();
    assert_eq!(answers.len(), 1, "{answers:?}");
    assert_eq!(env.prompts(), ["perm edit"]);
    // The editor's answer is told by its option's kind (ADR 8.1).
    let log = env.ok(&["event", "log", "sess-1", "--json", "--events", "permission_resolved"]);
    let resolved: Value = serde_json::from_str(log.lines().next().unwrap_or_default()).unwrap();
    assert_eq!(
        (&resolved["answer"], &resolved["option_kind"], &resolved["by"]),
        (&"allowed".into(), &"allow_once".into(), &"editor".into()),
        "{resolved}"
    );
}

/// An editor's process opens no session for `--pid`, new or resumed, as
/// it forks none: the session would be a headless one in a process that
/// ends with the editor (ADR 7.1, ADR 8.1).
#[test]
fn adr_0008_pid_refused_in_an_editors_process() {
    let env = Env::new("ex-pid");
    let (_editor, _to_agent, _from_agent) = open_editor(&env);
    let pid = env.pid();
    for args in
        [&["session", "new", "--pid", &pid][..], &["session", "resume", "--pid", &pid, "old-1"]]
    {
        let err = env.fails(args);
        assert!(err.contains("the editor owns this process; open sessions there"), "{err}");
    }
    assert_eq!(env.calls_of("session/new").len(), 1);
    assert!(env.calls_of("session/resume").is_empty());
}

/// In strict mode an editor's session has no experimental actions, whatever
/// its profile enables (ADR 2.1), and fork stays refused.
#[test]
fn adr_0002_strict_mode_has_no_experimental_actions() {
    let env = Env::new("ex-strict");
    env.write_config(
        "[profiles.default]\nstrict = true\n\n[profiles.default.editor]\nexperimental = \
         [\"send\", \"context\", \"cancel\", \"permission\", \"config\", \"close\"]
",
    );
    let (_editor, mut to_agent, mut from_agent) = open_editor(&env);
    writeln!(to_agent, "{}", editor_prompt(3, "perm edit")).unwrap();
    line_with(&mut from_agent, "session/request_permission");
    let actions: [(&[&str], &str); 7] = [
        (&["prompt", "send", "sess-1", "hi"], "send"),
        (&["prompt", "send", "sess-1", "--context", "hi"], "context"),
        (&["prompt", "cancel", "sess-1"], "cancel"),
        (&["permission", "allow", "sess-1", "p1"], "permission"),
        (&["permission", "reject", "sess-1", "p1"], "permission"),
        (&["config", "set", "sess-1", "--mode", "plan"], "config"),
        (&["session", "close", "sess-1"], "close"),
    ];
    for (args, action) in actions {
        let err = env.fails(args);
        let says =
            format!("{action} on an editor's session is an experimental action, and strict mode");
        assert!(err.contains(&says), "{args:?}: {err}");
    }
    assert!(env.fails(&["session", "fork", "sess-1"]).contains("the editor owns this process"));
    writeln!(to_agent, "{ALLOW}").unwrap();
    line_with(&mut from_agent, "end_turn");
}

/// `send` to an editor's session goes as a prompt only while no turn runs,
/// the editor's or brnr's, and is shown to the editor (ADR 7.2); it is never
/// held, steered or interrupting. Enabling `send` enables nothing else, and
/// the refusal names the profile.
#[test]
fn adr_0007_send_to_an_editors_session_waits_for_no_turn() {
    let env = Env::new("ex-send");
    env.write_config("[profiles.work.editor]\nexperimental = [\"send\"]\n");
    let (_editor, mut to_agent, mut from_agent) = open_editor_with(&env, &["--profile", "work"]);
    assert!(env.ok(&["prompt", "send", "sess-1", "reply hi"]).starts_with("delivered"));
    let echo = update(&mut from_agent, "tool_call");
    assert_eq!(
        (&echo["title"], &echo["status"]),
        (&"Message via brnr".into(), &"completed".into())
    );
    wait_idle(&env);
    // The editor's turn.
    writeln!(to_agent, "{}", editor_prompt(3, "hang")).unwrap();
    assert!(wait_for(Duration::from_secs(5), || env.prompts().len() == 2));
    let err = env.fails(&["prompt", "send", "sess-1", "more"]);
    assert!(err.contains("the editor controls this session's turns; one is running"), "{err}");
    for flag in ["--steer", "--interrupt"] {
        let err = env.fails(&["prompt", "send", "sess-1", flag, "more"]);
        let says = format!("the editor controls this session's turns; {flag} is its call");
        assert!(err.contains(&says), "{err}");
    }
    let err = env.fails(&["prompt", "cancel", "sess-1"]);
    assert!(err.contains("`cancel` on an editor's session is experimental"), "{err}");
    assert!(err.contains("under `[profiles.work.editor]`"), "{err}");
    let cancel = r#"{"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":"sess-1"}}"#;
    writeln!(to_agent, "{cancel}").unwrap();
    assert_eq!(response(&mut from_agent, 3)["result"]["stopReason"], "cancelled");
    // brnr's own turn.
    env.ok(&["prompt", "send", "sess-1", "hang on"]);
    assert!(wait_for(Duration::from_secs(5), || env.prompts().len() == 3));
    let err = env.fails(&["prompt", "send", "sess-1", "more"]);
    assert!(err.contains("the editor controls this session's turns; one is running"), "{err}");
    writeln!(to_agent, "{cancel}").unwrap();
    wait_idle(&env);
    sleep(Duration::from_millis(300));
    assert_eq!(env.prompts(), ["reply hi", "hang", "hang on"]);
    assert!(env.calls_of("_session/steering").is_empty());
}

/// `context` joins the editor's own next prompt, and is shown to it.
#[test]
fn adr_0007_context_joins_the_editors_next_prompt() {
    let env = Env::new("ex-context");
    let (_editor, mut to_agent, mut from_agent) = experimental_editor(&env, &["context"]);
    let err = env.fails(&["prompt", "send", "sess-1", "hi"]);
    assert!(err.contains("`send` on an editor's session is experimental"), "{err}");
    env.ok(&["prompt", "send", "sess-1", "--context", "dropped"]);
    env.ok(&["queue", "clear", "sess-1", "--context"]);
    env.ok(&["prompt", "send", "sess-1", "--context", "kept"]);
    writeln!(to_agent, "{}", editor_prompt(3, "reply ok")).unwrap();
    let echo = update(&mut from_agent, "tool_call");
    assert_eq!(echo["title"], "Context via brnr");
    assert_eq!(echo["content"][0]["content"]["text"], "kept");
    response(&mut from_agent, 3);
    assert_eq!(env.prompts(), ["reply ok\nkept"]);
}

/// With `echo_as = "user_message_chunk"`, `send` and `context` are shown to
/// the editor as the user's own message, one chunk per block, and no tool
/// call; brnr's notes are still tool calls.
#[test]
fn adr_0007_messages_can_be_shown_as_user_message_chunks() {
    let env = Env::new("ex-echo-as");
    env.write_config(
        "[profiles.default.editor]\nexperimental = [\"send\", \"context\", \"permission\"]\n\
         echo_as = \"user_message_chunk\"\n",
    );
    let (_editor, mut to_agent, mut from_agent) = open_editor(&env);
    let echo = |from_agent: &mut BufReader<ChildStdout>| {
        let kinds = ["tool_call", "user_message_chunk"];
        let msg = message(from_agent, |m| {
            kinds.iter().any(|k| m["params"]["update"]["sessionUpdate"] == *k)
        });
        msg["params"]["update"].clone()
    };
    env.ok(&["prompt", "send", "sess-1", "reply hi"]);
    let chunk = echo(&mut from_agent);
    assert_eq!(chunk["sessionUpdate"], "user_message_chunk", "{chunk}");
    assert_eq!(chunk["content"], serde_json::json!({ "type": "text", "text": "reply hi" }));
    wait_idle(&env);
    env.ok(&["prompt", "send", "sess-1", "--context", "kept"]);
    writeln!(to_agent, "{}", editor_prompt(3, "reply ok")).unwrap();
    let chunk = echo(&mut from_agent);
    assert_eq!(chunk["sessionUpdate"], "user_message_chunk", "{chunk}");
    assert_eq!(chunk["content"]["text"], "kept");
    response(&mut from_agent, 3);
    writeln!(to_agent, "{}", editor_prompt(4, "perm edit")).unwrap();
    line_with(&mut from_agent, "session/request_permission");
    env.ok(&["permission", "allow", "sess-1", "p1"]);
    let told = update(&mut from_agent, "tool_call");
    assert_eq!(told["title"], "Allowed via brnr", "{told}");
    response(&mut from_agent, 4);
    assert_eq!(env.prompts(), ["reply hi", "reply ok\nkept", "perm edit"]);
    // Events don't change with it.
    let log = env.ok(&["event", "log", "sess-1", "--json"]);
    let by: Vec<Value> = (log.lines())
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|e| e["event"] == "user_message")
        .map(|e| e["by"].clone())
        .collect();
    assert_eq!(by, ["control", "editor", "editor"], "{log}");
}

/// `cancel` answers the agent's pending request `cancelled` and withdraws it
/// from the editor (`$/cancel_request`). The editor acknowledging that is
/// dropped, with nothing to tell it.
#[test]
fn adr_0007_cancel_withdraws_the_editors_requests() {
    let env = Env::new("ex-cancel");
    let (_editor, mut to_agent, mut from_agent) = experimental_editor(&env, &["cancel"]);
    writeln!(to_agent, "{}", editor_prompt(3, "perm edit")).unwrap();
    line_with(&mut from_agent, "session/request_permission");
    assert!(env.ok(&["prompt", "cancel", "sess-1"]).starts_with("cancelling"));
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

/// `permission allow` and `reject` answer the agent in the editor's place:
/// the request is withdrawn from the editor, its tool call updated, and the
/// editor told who answered. The editor's own answer after that is dropped, and it is
/// told who answered first.
#[test]
fn adr_0007_approve_answers_in_the_editors_place() {
    let env = Env::new("ex-approve");
    let (_editor, mut to_agent, mut from_agent) = experimental_editor(&env, &["permission"]);
    writeln!(to_agent, "{}", editor_prompt(3, "perm edit")).unwrap();
    line_with(&mut from_agent, "session/request_permission");
    let show = env.ok(&["permission", "show", "sess-1", "p1"]);
    let here = "answer: in the editor, or brnr permission allow sess-1 p1 or reject sess-1 p1";
    assert!(show.contains(here), "{show}");
    let json: Value =
        serde_json::from_str(&env.ok(&["permission", "show", "sess-1", "p1", "--json"])).unwrap();
    assert_eq!(
        (&json["answerable"], &json["why_not"]),
        (&Value::Bool(true), &Value::Null),
        "{json}"
    );
    assert_eq!(env.ok(&["permission", "allow", "sess-1", "p1"]), "p1 allow\n");
    let withdrawn = message(&mut from_agent, |m| m["method"] == "$/cancel_request");
    assert_eq!(withdrawn["params"]["requestId"], "perm-1");
    let tool = update(&mut from_agent, "tool_call_update");
    assert_eq!((&tool["toolCallId"], &tool["status"]), (&"perm-1".into(), &"in_progress".into()));
    let told = update(&mut from_agent, "tool_call");
    assert_eq!(told["title"], "Allowed via brnr", "{told}");
    let text = told["content"][0]["content"]["text"].as_str().unwrap_or_default();
    let says = "\"Edit src/lib.rs\" was allowed through brnr's control socket";
    assert!(text.contains(says), "{text}");
    assert_eq!(response(&mut from_agent, 3)["result"]["stopReason"], "end_turn");
    writeln!(to_agent, "{ALLOW}").unwrap();
    let note = update(&mut from_agent, "tool_call");
    assert_eq!(note["title"], "Already allowed via brnr", "{note}");
    let text = note["content"][0]["content"]["text"].as_str().unwrap_or_default();
    let says = "\"Edit src/lib.rs\" was already allowed through brnr's control socket";
    assert!(text.contains(says), "{text}");
    // A rejected tool call has failed.
    writeln!(to_agent, "{}", editor_prompt(4, "perm edit")).unwrap();
    line_with(&mut from_agent, "session/request_permission");
    assert_eq!(env.ok(&["permission", "reject", "sess-1", "p2"]), "p2 reject\n");
    let tool = update(&mut from_agent, "tool_call_update");
    assert_eq!((&tool["toolCallId"], &tool["status"]), (&"perm-1".into(), &"failed".into()));
    assert_eq!(update(&mut from_agent, "tool_call")["title"], "Rejected via brnr");
    response(&mut from_agent, 4);
    let answers: Vec<Value> = (env.calls().into_iter())
        .filter(|c| c["id"] == "perm-1" && c.get("method").is_none())
        .map(|c| c["result"]["outcome"]["optionId"].clone())
        .collect();
    assert_eq!(answers, ["allow", "reject"]);
}

/// `brnr acp` with sess-1 open, for an editor that takes elicitations in
/// both modes, with `actions` enabled in its profile.
fn eliciting_editor(env: &Env, actions: &[&str]) -> (Child, ChildStdin, BufReader<ChildStdout>) {
    let names: Vec<String> = actions.iter().map(|a| format!("{a:?}")).collect();
    env.write_config(&format!(
        "[profiles.default.editor]\nexperimental = [{}]\n",
        names.join(", ")
    ));
    let mut editor = env
        .brnr(&["acp", "--", AGENT])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut to_agent = editor.stdin.take().unwrap();
    let mut from_agent = BufReader::new(editor.stdout.take().unwrap());
    let caps = serde_json::json!({ "elicitation": { "form": {}, "url": {} } });
    let params = serde_json::json!({ "protocolVersion": 1, "clientCapabilities": caps });
    let init =
        serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": params });
    writeln!(to_agent, "{init}").unwrap();
    response(&mut from_agent, 1);
    let new = serde_json::json!({ "jsonrpc": "2.0", "id": 2, "method": "session/new",
                                  "params": { "cwd": env.dir, "mcpServers": [] } });
    writeln!(to_agent, "{new}").unwrap();
    assert_eq!(response(&mut from_agent, 2)["result"]["sessionId"], "sess-1");
    (editor, to_agent, from_agent)
}

/// An editor's elicitation reaches it as the agent sent it, and is
/// `elicitation_request`, `owner` `editor`, for watchers and bridges; the
/// editor's answer is `elicitation_resolved` `by` `editor`. Answering it
/// from outside is the experimental `elicitation`: refused unless the
/// profile enables it, and with it the request is withdrawn from the
/// editor, which is told who answered, and its late answer dropped.
#[test]
fn adr_0004_an_editors_elicitation_is_seen_and_reaches_the_editor() {
    let env = Env::new("ex-elicit");
    let (_editor, mut to_agent, mut from_agent) = eliciting_editor(&env, &[]);
    writeln!(to_agent, "{}", editor_prompt(3, "elicit form")).unwrap();
    let asked = message(&mut from_agent, |m| m["method"] == "elicitation/create");
    assert_eq!(asked["id"], "elicit-1");
    assert_eq!(
        (&asked["params"]["mode"], &asked["params"]["requestedSchema"]["title"]),
        (&"form".into(), &"About you".into()),
        "{asked}"
    );
    let pending: Value =
        serde_json::from_str(&env.ok(&["elicitation", "requests", "sess-1", "--json"])).unwrap();
    assert_eq!((&pending[0]["request"], &pending[0]["owner"]), (&"e1".into(), &"editor".into()));
    let err = env.fails(&["elicitation", "decline", "sess-1", "e1"]);
    let says = "`elicitation` on an editor's session is experimental; enable it with \
                `experimental = [\"elicitation\"]` under `[profiles.default.editor]`";
    assert!(err.contains(says), "{err}");
    let show = env.ok(&["elicitation", "show", "sess-1", "e1"]);
    assert!(show.contains("e1, session sess-1, waiting in the editor"), "{show}");
    assert!(show.contains("answer it in the editor (`elicitation` on an editor's"), "{show}");
    let declined = r#"{"jsonrpc":"2.0","id":"elicit-1","result":{"action":"decline"}}"#;
    writeln!(to_agent, "{declined}").unwrap();
    assert_eq!(response(&mut from_agent, 3)["result"]["stopReason"], "end_turn");
    let log = env.ok(&["event", "log", "sess-1", "--json", "--events", "all"]);
    let events: Vec<Value> = log.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    let request = events.iter().find(|e| e["event"] == "elicitation_request").unwrap();
    assert_eq!((&request["owner"], &request["request"]), (&"editor".into(), &"e1".into()));
    let resolved = events.iter().find(|e| e["event"] == "elicitation_resolved").unwrap();
    assert_eq!(
        (&resolved["action"], &resolved["by"]),
        (&"decline".into(), &"editor".into()),
        "{resolved}"
    );
    drop((to_agent, from_agent));

    let env = Env::new("ex-elicit-on");
    let (_editor, mut to_agent, mut from_agent) = eliciting_editor(&env, &["elicitation"]);
    writeln!(to_agent, "{}", editor_prompt(3, "elicit form")).unwrap();
    message(&mut from_agent, |m| m["method"] == "elicitation/create");
    let show = env.ok(&["elicitation", "show", "sess-1", "e1"]);
    assert!(show.contains("answer: in the editor, or brnr elicitation accept sess-1 e1"), "{show}");
    let accept =
        ["elicitation", "accept", "sess-1", "e1", "--field", "name=ab", "--field", "color=red"];
    assert_eq!(env.ok(&accept), "e1 accepted\n");
    let withdrawn = message(&mut from_agent, |m| m["method"] == "$/cancel_request");
    assert_eq!(withdrawn["params"]["requestId"], "elicit-1");
    let told = update(&mut from_agent, "tool_call");
    assert_eq!(told["title"], "Accepted via brnr", "{told}");
    let text = told["content"][0]["content"]["text"].as_str().unwrap_or_default();
    let says = "\"Tell me about you\" was accepted through brnr's control socket";
    assert!(text.contains(says), "{text}");
    assert_eq!(response(&mut from_agent, 3)["result"]["stopReason"], "end_turn");
    let late = r#"{"jsonrpc":"2.0","id":"elicit-1","result":{"action":"decline"}}"#;
    writeln!(to_agent, "{late}").unwrap();
    let note = update(&mut from_agent, "tool_call");
    assert_eq!(note["title"], "Already accepted via brnr", "{note}");
    let answers: Vec<Value> = (env.calls().into_iter())
        .filter(|c| c["id"] == "elicit-1" && c.get("method").is_none())
        .map(|c| c["result"].clone())
        .collect();
    let content = serde_json::json!({ "name": "ab", "color": "red" });
    assert_eq!(answers, [serde_json::json!({ "action": "accept", "content": content })]);
}

/// The agent answers a change of mode or config option only to the host,
/// which asked; the editor is sent the update itself (ADR 6.2).
#[test]
fn adr_0007_settings_are_told_to_the_editor() {
    let env = Env::new("ex-settings").agent("QUIET_MODE", "1");
    let (_editor, _to_agent, mut from_agent) = experimental_editor(&env, &["config"]);
    assert_eq!(env.ok(&["config", "set", "sess-1", "--mode", "plan"]), "mode=plan\n");
    assert_eq!(update(&mut from_agent, "current_mode_update")["currentModeId"], "plan");
    env.ok(&["config", "set", "sess-1", "--model", "large"]);
    let config = update(&mut from_agent, "config_option_update");
    assert_eq!(config["configOptions"][0]["currentValue"], "large", "{config}");
    env.ok(&["config", "set", "sess-1", "--option", "model=small"]);
    let config = update(&mut from_agent, "config_option_update");
    assert_eq!(config["configOptions"][0]["currentValue"], "small", "{config}");
}

/// An editor that negotiates boolean config options gets them from the
/// agent, and `config set` sets one as ACP has it: `type: "boolean"` and a JSON
/// boolean. A value that isn't `true` or `false` fails before it reaches the
/// agent; a select option is still set by its value id.
#[test]
fn adr_0006_a_boolean_option_is_set_as_a_boolean() {
    let env = Env::new("ex-boolean");
    env.write_config("[profiles.default.editor]\nexperimental = [\"config\"]\n");
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
    assert!(env.ok(&["config", "get", "sess-1"]).contains("fast    -         false"));

    assert_eq!(env.ok(&["config", "set", "sess-1", "--option", "fast=true"]), "fast=true\n");
    let set = env.calls_of("session/set_config_option")[0]["params"].clone();
    let typed = serde_json::json!({ "sessionId": "sess-1", "configId": "fast", "type": "boolean", "value": true });
    assert_eq!(set, typed);
    let config = update(&mut from_agent, "config_option_update");
    assert_eq!(config["configOptions"][1]["currentValue"], true, "{config}");
    assert!(env.ok(&["config", "get", "sess-1"]).contains("fast    -         true"));

    let err = env.fails(&["config", "set", "sess-1", "--option", "fast=yes"]);
    assert!(err.contains("fast is a boolean option: true or false, not yes"), "{err}");
    assert_eq!(env.calls_of("session/set_config_option").len(), 1, "yes reached the agent");

    env.ok(&["config", "set", "sess-1", "--option", "model=large"]);
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
fn adr_0007_close_tells_the_editor() {
    let env = Env::new("ex-close");
    let (_editor, mut to_agent, mut from_agent) = experimental_editor(&env, &["close"]);
    writeln!(to_agent, "{}", editor_prompt(3, "hang")).unwrap();
    assert!(wait_for(Duration::from_secs(5), || env.prompts().len() == 1));
    env.ok(&["session", "close", "sess-1"]);
    assert_eq!(update(&mut from_agent, "tool_call")["title"], "Session closed via brnr");
    assert_eq!(response(&mut from_agent, 3)["result"]["stopReason"], "cancelled");
    assert_eq!(env.calls_of("session/close").len(), 1);
    let log = env.ok(&["event", "log", "sess-1", "--json", "--events", "session_closed"]);
    let closed: Value =
        serde_json::from_str(log.lines().next().expect("no session_closed")).unwrap();
    assert_eq!((&closed["by"], &closed["reason"]), (&"control".into(), &"close".into()));
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
fn adr_0007_take_over_from_an_editor() {
    let env = Env::new("ex-takeover");
    let (_editor, mut to_agent, mut from_agent) = experimental_editor(&env, &["close"]);
    let editors = env.pid();
    writeln!(to_agent, "{}", editor_prompt(3, "hang")).unwrap();
    assert!(wait_for(Duration::from_secs(5), || env.prompts().len() == 1));
    let out = env.run(&resume_args("sess-1", &["--take-over"]));
    assert!(out.status.success(), "{}", stderr(&out));
    let said = format!("closed sess-1 in process {editors}");
    assert!(stderr(&out).contains(&said), "{}", stderr(&out));
    let status: Value =
        serde_json::from_str(&env.ok(&["session", "status", "sess-1", "--json"])).unwrap();
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

/// The editor's own steer goes to the agent untouched but for its id, the
/// host's (ADR 2.4), and once the agent has taken it into the turn it is a
/// `user_message` of that turn, by the editor.
#[test]
fn adr_0007_the_editors_own_steer_is_recorded() {
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
    let mut got = env.calls_of("_session/steering");
    assert_ne!(got[0]["id"], 4, "the agent got the editor's id");
    got[0]["id"] = json!(4);
    assert_eq!(got, [steer]);
    let log = env.ok(&["event", "log", "sess-1", "--json", "--events", "user_message"]);
    let said: Vec<Value> = log.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(said.len(), 2, "{said:?}");
    assert_eq!((&said[1]["by"], &said[1]["text"]), (&"editor".into(), &"reply steered".into()));
    assert_eq!(said[1]["prompt"], said[0]["prompt"]);
}

// ---- request ids (ADR 2.4) ----------------------------------------------

/// The editor's `session/cancel` of sess-1's turn.
const CANCEL: &str =
    r#"{"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":"sess-1"}}"#;

/// The editor's request `method` with id `id`, as it is written.
fn request_with_id(id: &str, method: &str, params: Value) -> String {
    format!(r#"{{"jsonrpc":"2.0","id":{id},"method":"{method}","params":{params}}}"#)
}

/// The editor's prompt with id `id`, as it is written.
fn prompt_with_id(id: &str, text: &str) -> String {
    let prompt = json!([{ "type": "text", "text": text }]);
    request_with_id(id, "session/prompt", json!({ "sessionId": "sess-1", "prompt": prompt }))
}

/// The id of a message, as it is written.
fn raw_id(line: &str) -> Option<String> {
    let found = brnr::internals::json::members(line.as_bytes(), "id");
    found.last().map(|span| line[span.clone()].to_owned())
}

/// The answers the editor gets, until the one to `id` (as it is written):
/// each with its id as written.
fn answers_until(from_agent: &mut BufReader<ChildStdout>, id: &str) -> Vec<(String, Value)> {
    let mut answers = Vec::new();
    let mut line = String::new();
    loop {
        line.clear();
        assert!(from_agent.read_line(&mut line).unwrap() > 0, "no answer to {id}");
        let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
        let Some(raw) = raw_id(&line).filter(|_| msg.get("method").is_none()) else { continue };
        let last = raw == id;
        answers.push((raw, msg));
        if last {
            return answers;
        }
    }
}

/// The editor's answer to `id`, as it is written.
fn answer_to(from_agent: &mut BufReader<ChildStdout>, id: &str) -> Value {
    answers_until(from_agent, id).pop().unwrap().1
}

fn state(env: &Env) -> Value {
    let status: Value =
        serde_json::from_str(&env.ok(&["session", "status", "sess-1", "--json"])).unwrap();
    status["state"].clone()
}

/// The ids of the requests the agent got: each its own.
fn assert_distinct_ids(env: &Env) -> Vec<Value> {
    let ids: Vec<Value> = (env.calls().into_iter())
        .filter(|c| c.get("method").is_some() && c.get("id").is_some())
        .map(|c| c["id"].clone())
        .collect();
    for (i, id) in ids.iter().enumerate() {
        assert!(!ids[..i].contains(id), "the agent got {id} twice: {ids:?}");
    }
    ids
}

/// The `turn_ended` events of sess-1.
fn turns_ended(env: &Env) -> Vec<Value> {
    let log = env.ok(&["event", "log", "sess-1", "--json", "--events", "turn_ended"]);
    log.lines().map(|l| serde_json::from_str(l).unwrap()).collect()
}

/// An editor's prompt whose id is one the host gives its own requests
/// (`brnr-1`) runs on while `config set`s are answered: the
/// session is busy until the agent answers the prompt itself, and the editor
/// gets that answer, with its id.
#[test]
fn adr_0002_an_editors_prompt_ends_only_with_its_own_answer() {
    let env = Env::new("id-prompt");
    let (_editor, mut to_agent, mut from_agent) = experimental_editor(&env, &["config"]);
    writeln!(to_agent, "{}", prompt_with_id(r#""brnr-1""#, "hang")).unwrap();
    assert!(wait_for(Duration::from_secs(5), || env.prompts().len() == 1));
    assert_eq!(state(&env), "busy");
    assert_eq!(env.ok(&["config", "set", "sess-1", "--mode", "plan"]), "mode=plan\n");
    assert_eq!(state(&env), "busy", "the mode's answer ended the editor's turn");
    env.ok(&["config", "set", "sess-1", "--model", "large"]);
    env.ok(&["config", "set", "sess-1", "--option", "model=small"]);
    assert_eq!(state(&env), "busy");
    assert!(turns_ended(&env).is_empty());
    writeln!(to_agent, "{CANCEL}").unwrap();
    let answers = answers_until(&mut from_agent, r#""brnr-1""#);
    assert_eq!(answers.len(), 1, "{answers:?}");
    assert_eq!(answers[0].1["result"]["stopReason"], "cancelled");
    wait_idle(&env);
    let ended = turns_ended(&env);
    assert_eq!(ended.len(), 1, "{ended:?}");
    assert_eq!(
        (&ended[0]["by"], &ended[0]["stop_reason"]),
        (&"editor".into(), &"cancelled".into())
    );
    assert_distinct_ids(&env);
}

/// The editor's request with the id of the host's own request, a prompt
/// `send` injected, is the editor's: its answer reaches the editor, and the
/// host's turn runs on until the agent ends it.
#[test]
fn adr_0002_an_editors_request_with_a_host_requests_id_is_the_editors() {
    let env = Env::new("id-host").agent("QUIET_MODE", "1");
    let (_editor, mut to_agent, mut from_agent) = experimental_editor(&env, &["send"]);
    env.ok(&["prompt", "send", "sess-1", "hang on"]);
    assert!(wait_for(Duration::from_secs(5), || env.prompts().len() == 1));
    let host = env.calls_of("session/prompt")[0]["id"].to_string();
    let params = json!({ "sessionId": "sess-1", "modeId": "plan" });
    writeln!(to_agent, "{}", request_with_id(&host, "session/set_mode", params.clone())).unwrap();
    writeln!(to_agent, "{}", request_with_id(r#""after""#, "session/set_mode", params)).unwrap();
    let answers = answers_until(&mut from_agent, r#""after""#);
    assert_eq!(state(&env), "busy", "the editor's answer ended the host's turn");
    let raws: Vec<&str> = answers.iter().map(|(raw, _)| raw.as_str()).collect();
    assert_eq!(raws, [host.as_str(), r#""after""#], "{answers:?}");
    assert_eq!(answers[0].1["result"], json!({}));
    writeln!(to_agent, "{CANCEL}").unwrap();
    wait_idle(&env);
    let ended = turns_ended(&env);
    assert_eq!(ended.len(), 1, "{ended:?}");
    assert_eq!(
        (&ended[0]["by"], &ended[0]["stop_reason"]),
        (&"control".into(), &"cancelled".into())
    );
    assert_distinct_ids(&env);
}

/// Ids of either type, and numbers however they are written, come back to
/// the editor as it wrote them: `7` and `"7"` are two requests, and none of
/// the editor's ids reaches the agent.
#[test]
fn adr_0002_ids_come_back_as_the_editor_wrote_them() {
    let env = Env::new("id-types");
    let (_editor, mut to_agent, mut from_agent) = open_editor(&env);
    writeln!(to_agent, "{}", prompt_with_id("7", "hang")).unwrap();
    assert!(wait_for(Duration::from_secs(5), || env.prompts().len() == 1));
    let ids = [r#""7""#, "1e3", "-0.50", r#""brnr-1""#, r#""brnr-2""#];
    for id in ids {
        let params = json!({ "sessionId": "sess-1", "modeId": "plan" });
        writeln!(to_agent, "{}", request_with_id(id, "session/set_mode", params)).unwrap();
        let answer = answer_to(&mut from_agent, id);
        assert_eq!(answer["result"], json!({}), "{id}: {answer}");
    }
    assert_eq!(state(&env), "busy");
    writeln!(to_agent, "{CANCEL}").unwrap();
    let answers = answers_until(&mut from_agent, "7");
    assert_eq!(answers.len(), 1, "{answers:?}");
    assert_eq!(answers[0].1["result"]["stopReason"], "cancelled");
    // As the agent read them, too.
    let got = assert_distinct_ids(&env);
    for id in [json!(7), json!("7"), json!(1000.0), json!(-0.5)] {
        assert!(!got.contains(&id), "the agent got the editor's id {id}: {got:?}");
    }
}

/// The editor's `$/cancel_request` names its own requests: one of its own
/// goes to the agent with that request's id there, and one naming the id of
/// the host's prompt doesn't go, so the host's turn runs on.
#[test]
fn adr_0002_the_editors_cancel_request_is_for_its_own_request() {
    let env = Env::new("id-cancel").agent("QUIET_MODE", "1");
    let (_editor, mut to_agent, mut from_agent) = experimental_editor(&env, &["send"]);
    let cancel_request = |id: &str| {
        format!(r#"{{"jsonrpc":"2.0","method":"$/cancel_request","params":{{"requestId":{id}}}}}"#)
    };
    env.ok(&["prompt", "send", "sess-1", "hang on"]);
    assert!(wait_for(Duration::from_secs(5), || env.prompts().len() == 1));
    let host = env.calls_of("session/prompt")[0]["id"].to_string();
    writeln!(to_agent, "{}", cancel_request(&host)).unwrap();
    let params = json!({ "sessionId": "sess-1", "modeId": "plan" });
    writeln!(to_agent, "{}", request_with_id(r#""after""#, "session/set_mode", params)).unwrap();
    answer_to(&mut from_agent, r#""after""#);
    assert_eq!(state(&env), "busy", "the editor cancelled the host's prompt");
    assert!(env.calls_of("$/cancel_request").is_empty());
    writeln!(to_agent, "{CANCEL}").unwrap();
    wait_idle(&env);

    writeln!(to_agent, "{}", prompt_with_id(r#""mine""#, "hang")).unwrap();
    assert!(wait_for(Duration::from_secs(5), || env.prompts().len() == 2));
    writeln!(to_agent, "{}", cancel_request(r#""mine""#)).unwrap();
    let answer = answer_to(&mut from_agent, r#""mine""#);
    assert_eq!(answer["error"]["code"], -32800, "{answer}");
    let sent = env.calls_of("$/cancel_request");
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0]["params"]["requestId"], env.calls_of("session/prompt")[1]["id"]);
    wait_idle(&env);
}
