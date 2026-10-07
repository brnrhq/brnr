//! Headless sessions (`brnr start`) and editors' (`brnr acp`) against a fake
//! ACP agent (fake_agent.py). Each test gets its own runtime, state and config
//! directories, and kills whatever it leaves running.

mod common;

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
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
        .brnr(&start_args(&["--prompt", "run the migration"]))
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
fn empty_prompt_is_refused() {
    let env = Env::new("empty");
    let out = env.run(&start_args(&["--prompt", " "]));
    assert!(!out.status.success());
    assert!(stderr(&out).contains("empty"), "{}", stderr(&out));
    assert!(env.hosts().is_empty());
}

/// The prompt reaches the agent without ever being on a command line, where
/// `ps` shows it and Linux caps a single argument at 128 KiB.
#[test]
fn prompt_is_not_on_the_command_line() {
    let env = Env::new("argv");
    env.start(&["--prompt", "deploy with sk-SECRET-123"]);
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
    let out = env.run_with_stdin(&start_args(&["--prompt", "-"]), prompt.as_bytes());
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
fn foreground_start_failure_is_reported() {
    let env = Env::new("fg-mode");
    let out = env.run(&start_args(&["--foreground", "--mode", "bogus"]));
    assert!(!out.status.success(), "exited 0: {}", stderr(&out));
    assert!(stderr(&out).contains("setting mode bogus failed: no mode bogus"), "{}", stderr(&out));
}

/// Closing the last session ends a foreground process as it should: no
/// claim that the session never started.
#[test]
fn foreground_close_of_the_last_session() {
    let env = Env::new("fg-close");
    let mut fg = env
        .brnr(&start_args(&["--foreground", "--quiet"]))
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    assert!(wait_for(Duration::from_secs(10), || env.ok(&["list"]).contains("sess-1")), "no session");
    env.ok(&["close", "sess-1"]);
    assert!(wait_exit(&mut fg, Duration::from_secs(15)), "didn't stop with its last session");
    let mut err = String::new();
    fg.stderr.take().unwrap().read_to_string(&mut err).unwrap();
    assert!(!err.contains("before the session started"), "{err}");
    assert!(fg.wait().unwrap().success(), "{err}");
}

/// Answering a permission request while the agent is being stopped can't
/// reach it, so it fails rather than claiming success.
#[test]
fn approve_during_stop_fails() {
    let env = Env::new("stopperm").agent("PERMISSION", "1").agent("STUBBORN", "all");
    env.start(&["--prompt", "edit it"]);
    let waiting = || String::from_utf8_lossy(&env.run(&["pending", "sess-1"]).stdout).contains("p1");
    assert!(wait_for(Duration::from_secs(5), waiting), "no permission request");
    assert!(env.run(&["stop", &env.pid()]).status.success());
    let out = env.run(&["approve", "sess-1", "p1"]);
    assert!(!out.status.success(), "approve succeeded during stop");
    assert!(stderr(&out).contains("no longer"), "{}", stderr(&out));
}

/// Messages still held when the agent exits are reported, not dropped
/// silently.
#[test]
fn held_messages_are_reported_on_exit() {
    let env = Env::new("held");
    env.start(&["--prompt", "hang on"]);
    let mut watch = env
        .brnr(&["watch", "sess-1", "--json", "--events", "exited"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    sleep(Duration::from_millis(300));
    let out = env.run(&["send", "sess-1", "--after-turn", "later"]);
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "held (message m2)");
    assert!(env.run(&["stop", &env.pid()]).status.success());

    assert!(wait_exit(&mut watch, Duration::from_secs(15)), "watch didn't end");
    let line = BufReader::new(watch.stdout.take().unwrap()).lines().next();
    let line = line.expect("watch printed no exited event").unwrap();
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
    env.start(&["--prompt", "hang on"]);
    assert!(wait_for(Duration::from_secs(5), || env.prompts().len() == 1));
    for text in ["first", "second"] {
        let out = env.run(&["send", "sess-1", "--interrupt", text]);
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
    env.start(&[]);
    let mut watch = env
        .brnr(&["watch", "sess-1", "--events", "all"])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    sleep(Duration::from_millis(300));
    unsafe { libc::kill(watch.id() as i32, libc::SIGSTOP) };
    assert!(env.run(&["send", "sess-1", "go"]).status.success());
    assert!(wait_for(Duration::from_secs(10), || env.prompts().len() == 1));
    sleep(Duration::from_secs(3));
    unsafe { libc::kill(watch.id() as i32, libc::SIGCONT) };

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
/// cleanly when the host exits.
#[test]
fn reading_watcher_stays_connected() {
    let env = Env::new("fastwatch").agent("FLOOD", "20000");
    env.start(&[]);
    let mut watch = env
        .brnr(&["watch", "sess-1", "--events", "all"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = watch.stdout.take().unwrap();
    let lines = std::thread::spawn(move || BufReader::new(stdout).lines().count());
    sleep(Duration::from_millis(300));
    assert!(env.run(&["send", "sess-1", "go"]).status.success());
    assert!(wait_for(Duration::from_secs(10), || env.prompts().len() == 1));
    sleep(Duration::from_secs(2));
    assert!(env.run(&["stop", &env.pid()]).status.success());

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

/// Installed the way Homebrew does it: `bin/brnr` and the adapters are
/// symlinks in the prefix's `bin`, which isn't on the editor's PATH. brnr is
/// started by that path and finds an adapter by its bare name next to it.
#[test]
fn adapters_next_to_a_symlinked_brnr() {
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
        ["start", "--wait", "--prompt", "reply linked", "--", "brnr-claude-adapter"];
    let out = env.brnr_at(&bin.join("brnr"), &args).env("PATH", &path).output().unwrap();
    assert!(out.status.success(), "start: {}", stderr(&out));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "linked\n");
}

/// A peer a few MB behind still takes one big message: it is cut off only
/// once its backlog is past the limit, not because the next line is big.
#[test]
fn big_message_to_a_lagging_watcher() {
    let env = Env::new("lagbig");
    env.start(&[]);
    let mut watch = env
        .brnr(&["watch", "sess-1", "--json", "--events", "agent_message"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = watch.stdout.take().unwrap();
    sleep(Duration::from_millis(300));
    unsafe { libc::kill(watch.id() as i32, libc::SIGSTOP) };
    for size in [6_000_000, 12_000_000] {
        let out = env.run(&["send", "sess-1", "--wait", &format!("big {size}")]);
        assert!(out.status.success(), "send: {}", stderr(&out));
    }
    unsafe { libc::kill(watch.id() as i32, libc::SIGCONT) };
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
fn every_watcher_sees_the_exit() {
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
    unsafe { libc::kill(host, libc::SIGSTOP) };
    let out = env.run(&["send", "sess-1", "hello"]);
    unsafe { libc::kill(host, libc::SIGCONT) };
    assert!(!out.status.success());
    assert!(stderr(&out).contains("not answering"), "{}", stderr(&out));
}

/// A line that isn't a request (not even UTF-8) is answered with an error,
/// and the connection carries on.
#[test]
fn bad_request_line_is_answered() {
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
    assert_eq!((status["req_id"].as_i64(), status["ok"].as_bool()), (Some(1), Some(true)), "{status}");
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
    let out = env.run(&start_args(&[]));
    assert!(!out.status.success(), "started in a symlinked runtime dir");
    assert!(fs::read_dir(&target).unwrap().next().is_none(), "wrote into the symlink target");
}

/// `brnr acp` is what an editor runs as its agent: ACP over stdio, with the
/// session reachable meanwhile. When the editor goes, the agent goes too.
#[test]
fn acp_is_what_an_editor_runs() {
    use std::io::Write;
    let env = Env::new("ed-acp");
    let mut editor =
        env.brnr(&["acp", "--", AGENT]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
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

/// `brnr acp` with a session open (sess-1), as an editor has it.
fn open_editor(env: &Env) -> (Child, ChildStdin, BufReader<ChildStdout>) {
    let mut editor =
        env.brnr(&["acp", "--", AGENT]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
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
    let new = format!(
        r#"{{"jsonrpc":"2.0","id":2,"method":"session/new","params":{{"cwd":{:?},"mcpServers":[]}}}}"#,
        env.dir.display().to_string()
    );
    writeln!(to_agent, "{new}").unwrap();
    assert_eq!(answer(2)["result"]["sessionId"], "sess-1");
    (editor, to_agent, from_agent)
}

/// When the editor goes, what the agent started goes too: its process
/// group, as when it is stopped.
#[test]
fn editor_gone_takes_the_agents_children() {
    let env = Env::new("ed-kids").agent("STUBBORN", "child");
    let (mut editor, _to_agent, _from_agent) = open_editor(&env);
    let (host, child) = (env.host_pid(), env.child_pid());
    editor.kill().unwrap();
    editor.wait().unwrap();
    assert!(wait_for(Duration::from_secs(15), || !alive(host)), "the agent outlived the editor");
    assert!(wait_for(Duration::from_secs(2), || !alive(child)), "its child outlived the editor");
}

/// An editor that stops reading for LINK_WRITE_TIMEOUT (30 s) is gone, and
/// the agent with it; until then the editor still owns the session.
#[test]
fn editor_that_stops_reading_is_gone() {
    let env = Env::new("ed-stall").agent("FLOOD", "40000");
    let (mut editor, mut to_agent, from_agent) = open_editor(&env);
    let host = env.host_pid();
    // Some 25 MB of updates, which the editor never reads.
    let prompt = r#"{"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{"sessionId":"sess-1","prompt":[{"type":"text","text":"reply hi"}]}}"#;
    writeln!(to_agent, "{prompt}").unwrap();
    sleep(Duration::from_secs(2));
    assert!(env.ok(&["ps"]).contains("editor"));
    assert!(env.fails(&["fork", "sess-1"]).contains("the editor owns this process"));
    assert!(wait_for(Duration::from_secs(60), || !alive(host)), "the agent outlived a stalled editor");
    drop(from_agent);
    let _ = editor.kill();
}

/// An editor may hand over a non-blocking stdin: nothing to read yet is not
/// the end of it.
#[test]
fn non_blocking_stdin_is_waited_on() {
    let env = Env::new("ed-nonblock");
    let (reader, mut writer) = std::io::pipe().unwrap();
    let fd = reader.as_raw_fd();
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
fn a_lone_surrogate_is_read() {
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
fn deep_nesting_is_read() {
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
fn an_answer_the_host_cant_place_reaches_the_agent() {
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
/// agent isn't answered twice.
#[test]
fn a_late_answer_to_a_cancelled_request_is_dropped() {
    let env = Env::new("ed-late");
    let (_editor, mut to_agent, mut from_agent) = open_editor(&env);
    writeln!(to_agent, "{}", editor_prompt(3, "perm edit")).unwrap();
    line_with(&mut from_agent, "session/request_permission");
    env.ok(&["cancel", "sess-1"]);
    line_with(&mut from_agent, "end_turn");
    writeln!(to_agent, "{ALLOW}").unwrap();
    // Once a later prompt is answered, the host has seen the late answer.
    writeln!(to_agent, "{}", editor_prompt(4, "reply done")).unwrap();
    line_with(&mut from_agent, "end_turn");
    let answers: Vec<Value> = env.calls().into_iter().filter(|c| c["id"] == "perm-1").collect();
    assert_eq!(answers.len(), 1, "{answers:?}");
    assert_eq!(answers[0]["result"]["outcome"]["outcome"], "cancelled");
}
