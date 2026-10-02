//! Headless sessions (`brnr start`, `brnr host`) against a fake ACP agent
//! (fake_agent.py). Each test gets its own runtime, state and config
//! directories, and kills whatever it leaves running.

mod common;

use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::Duration;

use common::*;
use serde_json::Value;

// ---- starting ----------------------------------------------------------

/// A `brnr start` that goes away before the session opens must not leave
/// the agent working on its prompt.
#[test]
fn abandoned_start_sends_no_prompt() {
    let env = Env::new("abandon").agent("NEW_DELAY", "2");
    let mut start = env
        .brnr(&start_args("a", &["--prompt", "run the migration"]))
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    assert!(wait_for(Duration::from_secs(5), || !env.hosts().is_empty()), "no host");
    start.kill().unwrap();
    start.wait().unwrap();

    assert!(wait_for(Duration::from_secs(15), || env.hosts().is_empty()), "host kept running");
    assert!(env.prompts().is_empty(), "agent got {:?}", env.prompts());
}

/// When the session doesn't open in time, `brnr start` says so and the
/// host stops instead of carrying on unseen.
#[test]
fn start_timeout_stops_the_host() {
    let env = Env::new("timeout").agent("NEW_DELAY", "4");
    let out = env
        .brnr(&start_args("a", &["--prompt", "run the migration"]))
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
fn duplicate_name_is_refused() {
    let env = Env::new("dup");
    env.start("demo", &[]);
    let out = env.run(&start_args("demo", &[]));
    assert!(!out.status.success());
    assert!(stderr(&out).contains("already running"), "{}", stderr(&out));
    assert_eq!(env.hosts().len(), 1);
}

#[test]
fn empty_prompt_is_refused() {
    let env = Env::new("empty");
    let out = env.run(&start_args("a", &["--prompt", " "]));
    assert!(!out.status.success());
    assert!(stderr(&out).contains("empty"), "{}", stderr(&out));
    assert!(env.hosts().is_empty());
}

/// The prompt reaches the agent without ever being on a command line, where
/// `ps` shows it and Linux caps a single argument at 128 KiB.
#[test]
fn prompt_is_not_on_the_command_line() {
    let env = Env::new("argv");
    env.start("a", &["--prompt", "deploy with sk-SECRET-123"]);
    let ps = Command::new("ps").args(["-o", "args=", "-p", &env.host_pid().to_string()]).output();
    let args = String::from_utf8_lossy(&ps.unwrap().stdout).into_owned();
    assert!(args.contains("brnr"), "ps: {args}");
    assert!(!args.contains("sk-SECRET"), "prompt visible in ps: {args}");
    assert!(wait_for(Duration::from_secs(5), || !env.prompts().is_empty()));
    assert_eq!(env.prompts(), ["deploy with sk-SECRET-123"]);
}

#[test]
fn large_prompt_from_stdin() {
    let env = Env::new("bigprompt");
    let prompt = "x".repeat(300_000);
    let out = env.run_with_stdin(&start_args("a", &["--prompt", "-"]), prompt.as_bytes());
    assert!(out.status.success(), "start failed: {}", stderr(&out));
    assert!(wait_for(Duration::from_secs(5), || !env.prompts().is_empty()));
    assert_eq!(env.prompts()[0].len(), prompt.len());
}

// ---- stopping ----------------------------------------------------------

/// An agent that stops reading its stdin must not wedge the host: it still
/// answers, and `brnr stop` still ends it.
#[test]
fn stalled_agent_can_still_be_stopped() {
    let env = Env::new("stall").agent("STALL", "1");
    env.start("a", &[]);
    let host = env.host_pid();
    let big = vec![b'x'; 1 << 20];
    let out = env.run_with_stdin(&["send", "a", "-"], &big);
    assert!(out.status.success(), "send: {}", stderr(&out));

    let out = env.run(&["status", "a"]);
    assert!(out.status.success(), "status: {}", stderr(&out));
    let out = env.run(&["stop", "a"]);
    assert!(out.status.success(), "stop: {}", stderr(&out));
    assert!(wait_for(Duration::from_secs(15), || !alive(host)), "host still running");
}

/// `brnr stop` escalates to the agent's whole process group.
#[test]
fn stop_kills_the_agents_children() {
    let env = Env::new("stubborn").agent("STUBBORN", "all");
    env.start("a", &[]);
    let (host, child) = (env.host_pid(), env.child_pid());
    assert!(env.run(&["stop", "a"]).status.success());
    assert!(wait_for(Duration::from_secs(15), || !alive(host)), "host still running");
    assert!(wait_for(Duration::from_secs(2), || !alive(child)), "agent's child survived");
}

/// A child that outlives an agent which stopped cleanly is killed too.
#[test]
fn stop_kills_children_left_behind() {
    let env = Env::new("orphan").agent("STUBBORN", "child");
    env.start("a", &[]);
    let (host, child) = (env.host_pid(), env.child_pid());
    assert!(env.run(&["stop", "a"]).status.success());
    assert!(wait_for(Duration::from_secs(15), || !alive(host)), "host still running");
    assert!(wait_for(Duration::from_secs(2), || !alive(child)), "agent's child survived");
}

/// Answering a permission request while the agent is being stopped can't
/// reach it, so it fails rather than claiming success.
#[test]
fn approve_during_stop_fails() {
    let env = Env::new("stopperm").agent("PERMISSION", "1").agent("STUBBORN", "all");
    env.start("a", &["--prompt", "edit it"]);
    let waiting = || String::from_utf8_lossy(&env.run(&["pending", "a"]).stdout).contains("p1");
    assert!(wait_for(Duration::from_secs(5), waiting), "no permission request");
    assert!(env.run(&["stop", "a"]).status.success());
    let out = env.run(&["approve", "a"]);
    assert!(!out.status.success(), "approve succeeded during stop");
    assert!(stderr(&out).contains("no longer"), "{}", stderr(&out));
}

/// Messages still held when the agent exits are reported, not dropped
/// silently.
#[test]
fn held_messages_are_reported_on_exit() {
    let env = Env::new("held");
    env.start("a", &["--prompt", "hang on"]);
    let mut watch = env
        .brnr(&["watch", "a", "--json", "--events", "exited"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    sleep(Duration::from_millis(300));
    let out = env.run(&["send", "a", "--after-turn", "later"]);
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "held (session sess-1, message m2)");
    assert!(env.run(&["stop", "a"]).status.success());

    assert!(wait_exit(&mut watch, Duration::from_secs(15)), "watch didn't end");
    let line = BufReader::new(watch.stdout.take().unwrap()).lines().next().unwrap().unwrap();
    let exited: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(exited["event"], "exited");
    assert_eq!(exited["undelivered"][0]["text"], "later");
    assert_eq!(exited["undelivered"][0]["session"], "sess-1");
}

// ---- sending -----------------------------------------------------------

/// Several interrupts sent before the turn stops are delivered in the order
/// they were sent.
#[test]
fn interrupts_keep_their_order() {
    let env = Env::new("interrupt").agent("CANCEL_DELAY", "1");
    env.start("a", &["--prompt", "hang on"]);
    assert!(wait_for(Duration::from_secs(5), || env.prompts().len() == 1));
    for text in ["first", "second"] {
        let out = env.run(&["send", "a", "--interrupt", text]);
        assert!(out.status.success(), "send: {}", stderr(&out));
    }
    assert!(wait_for(Duration::from_secs(10), || env.prompts().len() == 3), "{:?}", env.prompts());
    assert_eq!(env.prompts(), ["hang on", "first", "second"]);
}

// ---- watching ----------------------------------------------------------

/// A watcher that stops reading is disconnected instead of having every
/// event queued for it in the host.
#[test]
fn slow_watcher_is_disconnected() {
    // About 30 MB of events: more than a peer's queue holds.
    let env = Env::new("slowwatch").agent("FLOOD", "40000");
    env.start("a", &[]);
    let mut watch = env
        .brnr(&["watch", "a", "--raw"])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    sleep(Duration::from_millis(300));
    unsafe { libc::kill(watch.id() as i32, libc::SIGSTOP) };
    assert!(env.run(&["send", "a", "go"]).status.success());
    assert!(wait_for(Duration::from_secs(10), || env.prompts().len() == 1));
    sleep(Duration::from_secs(3));
    unsafe { libc::kill(watch.id() as i32, libc::SIGCONT) };

    assert!(wait_exit(&mut watch, Duration::from_secs(20)), "watch is still connected");
    assert!(!watch.wait().unwrap().success());
    let mut err = String::new();
    watch.stderr.take().unwrap().read_to_string(&mut err).unwrap();
    assert!(err.contains("closed the connection"), "{err}");
    // It may still be working through the burst; it must get there.
    let answers = || env.run(&["status", "a"]).status.success();
    assert!(wait_for(Duration::from_secs(30), answers), "host stopped answering");
}

/// A watcher that keeps reading stays connected through a burst, and ends
/// cleanly when the host exits.
#[test]
fn reading_watcher_stays_connected() {
    let env = Env::new("fastwatch").agent("FLOOD", "20000");
    env.start("a", &[]);
    let mut watch = env
        .brnr(&["watch", "a", "--raw"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = watch.stdout.take().unwrap();
    let lines = std::thread::spawn(move || BufReader::new(stdout).lines().count());
    sleep(Duration::from_millis(300));
    assert!(env.run(&["send", "a", "go"]).status.success());
    assert!(wait_for(Duration::from_secs(10), || env.prompts().len() == 1));
    sleep(Duration::from_secs(2));
    assert!(env.run(&["stop", "a"]).status.success());

    assert!(wait_exit(&mut watch, Duration::from_secs(15)), "watch didn't end");
    let mut err = String::new();
    watch.stderr.take().unwrap().read_to_string(&mut err).unwrap();
    assert!(watch.wait().unwrap().success(), "watch failed: {err}");
    assert!(lines.join().unwrap() > 20000, "watch missed events");
}

/// One message bigger than a peer's whole queue still reaches a peer that
/// keeps up, and the status report (which quotes it) still gets out.
#[test]
fn huge_message_reaches_watchers() {
    let env = Env::new("huge");
    env.start("a", &[]);
    let mut watch = env
        .brnr(&["watch", "a", "--json", "--events", "agent_message"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = watch.stdout.take().unwrap();
    let first = std::thread::spawn(move || BufReader::new(stdout).lines().next());
    sleep(Duration::from_millis(300));
    let size = 20_000_000;
    assert!(env.run(&["send", "a", &format!("big {size}")]).status.success());
    let line = first.join().unwrap().expect("no message").unwrap();
    let event: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(event["text"].as_str().unwrap().len(), size);

    let out = env.run(&["status", "a", "--json"]);
    assert!(out.status.success(), "status: {}", stderr(&out));
    let status: Value = serde_json::from_slice(&out.stdout).unwrap();
    let preview = status["sessions"][0]["last_message"].as_str().unwrap();
    assert!(preview.chars().count() <= 4001, "status quotes {} chars", preview.chars().count());
    let _ = watch.kill();
    let _ = watch.wait();
}

/// Installed the way Homebrew does it: `bin/brnr` and the adapters are
/// symlinks in the prefix's `bin`, which isn't on the editor's PATH. brnr is
/// started by that path and finds an adapter by its bare name next to it.
#[test]
fn adapters_next_to_a_symlinked_brnr() {
    let env = Env::new("linked");
    let bin = env.dir.join("prefix").join("bin");
    fs::create_dir_all(&bin).unwrap();
    symlink(env!("CARGO_BIN_EXE_brnr"), bin.join("brnr")).unwrap();
    symlink(AGENT, bin.join("claude-agent-acp")).unwrap();
    // Enough PATH for the fake agent's python3, not the prefix.
    let python = Command::new("sh").args(["-c", "command -v python3"]).output().unwrap();
    let python = String::from_utf8(python.stdout).unwrap();
    let path = format!("{}:/usr/bin:/bin", Path::new(python.trim()).parent().unwrap().display());

    let doctor = env.brnr_at(&bin.join("brnr"), &["doctor"]).env("PATH", &path).output().unwrap();
    let doctor = String::from_utf8_lossy(&doctor.stdout).into_owned();
    let want = format!("ok    claude-agent-acp: {}", bin.join("claude-agent-acp").display());
    assert!(doctor.contains(&want), "{doctor}");

    let args =
        ["start", "--name", "a", "--wait", "--prompt", "reply linked", "--", "claude-agent-acp"];
    let out = env.brnr_at(&bin.join("brnr"), &args).env("PATH", &path).output().unwrap();
    assert!(out.status.success(), "start: {}", stderr(&out));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "linked\n");
}

/// A host that doesn't answer is reported as such, not as an errno.
#[test]
fn unresponsive_host_is_reported() {
    let env = Env::new("unresp");
    env.start("a", &[]);
    let host = env.host_pid();
    unsafe { libc::kill(host, libc::SIGSTOP) };
    let out = env.run(&["send", "a", "hello"]);
    unsafe { libc::kill(host, libc::SIGCONT) };
    assert!(!out.status.success());
    assert!(stderr(&out).contains("not answering"), "{}", stderr(&out));
}

// ---- transcripts -------------------------------------------------------

#[test]
fn transcripts_are_private() {
    let env = Env::new("perms");
    env.start("a", &["--prompt", "hello"]);
    assert!(wait_for(Duration::from_secs(5), || !env.prompts().is_empty()));
    let home = env.dir.join("home");
    let mut checked = 0;
    let mut check = |path: &Path| {
        let mode = fs::metadata(path).unwrap().permissions().mode() & 0o777;
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
    assert!(checked >= 6, "only {checked} paths checked");
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
    let out = env.run(&start_args("a", &[]));
    assert!(!out.status.success(), "started in a symlinked runtime dir");
    assert!(fs::read_dir(&target).unwrap().next().is_none(), "wrote into the symlink target");
}
