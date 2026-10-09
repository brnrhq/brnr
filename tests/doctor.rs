//! `brnr doctor`: what it finds, and what `--fix` repairs.

mod common;

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::os::unix::net::UnixListener;
use std::path::Path;
use std::process::{Command, Output};
use std::time::Duration;

use common::*;
use serde_json::Value;

fn doctor(env: &Env, args: &[&str]) -> (bool, String) {
    let mut all = vec!["doctor"];
    all.extend_from_slice(args);
    let out: Output = env.run(&all);
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), stderr(&out));
    (out.status.success(), text)
}

fn lines<'a>(text: &'a str, level: &str) -> Vec<&'a str> {
    text.lines().filter(|l| l.starts_with(level)).collect()
}

fn mode(path: &Path) -> u32 {
    fs::symlink_metadata(path).unwrap().permissions().mode() & 0o777
}

fn mkdir(path: &Path, mode: u32) {
    fs::create_dir_all(path).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

fn write(path: &Path, text: &str, mode: u32) {
    fs::write(path, text).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

/// A pid that no process has: a child that has been reaped.
fn dead_pid() -> u32 {
    let mut child = Command::new("true").spawn().unwrap();
    child.wait().unwrap();
    child.id()
}

#[test]
fn a_used_setup_is_healthy() {
    let env = Env::new("dr-ok");
    env.start(&["--prompt", "hello"]);
    let (ok, text) = doctor(&env, &[]);
    assert!(ok, "{text}");
    assert!(lines(&text, "warn").is_empty() && lines(&text, "FAIL").is_empty(), "{text}");
    assert!(text.contains("1 running and answering"), "{text}");
    // Its host log has no `exited` yet: it is running.
    assert!(
        text.contains("ok    host logs: no process died without recording it (1 log)"),
        "{text}"
    );
}

#[test]
fn readable_transcripts_are_made_private() {
    let env = Env::new("dr-logs");
    let home = env.dir.join("home");
    let project = home.join("projects").join("-tmp-x");
    mkdir(&home, 0o755);
    mkdir(&home.join("projects"), 0o755);
    mkdir(&project, 0o755);
    // A session's events and its raw ACP (ADR 22).
    write(&project.join("s.jsonl"), "{}\n", 0o644);
    write(&project.join("s.acp.jsonl"), "{}\n", 0o644);

    let (ok, text) = doctor(&env, &[]);
    assert!(ok, "warnings alone don't fail: {text}");
    assert_eq!(lines(&text, "warn").len(), 1, "{text}");
    assert!(text.contains("5 of 5 paths can be read by others"), "{text}");

    let (ok, text) = doctor(&env, &["--fix"]);
    assert!(ok, "{text}");
    assert!(text.contains("made 5 paths private"), "{text}");
    for dir in [&home, &home.join("projects"), &project] {
        assert_eq!(mode(dir), 0o700, "{}", dir.display());
    }
    assert_eq!(mode(&project.join("s.jsonl")), 0o600);
    assert_eq!(mode(&project.join("s.acp.jsonl")), 0o600);
    let (_, text) = doctor(&env, &[]);
    assert!(lines(&text, "warn").is_empty(), "{text}");
}

#[test]
fn open_runtime_dir_fails_until_fixed() {
    let env = Env::new("dr-run");
    let run = env.dir.join("run");
    mkdir(&run, 0o755);

    let (ok, text) = doctor(&env, &[]);
    assert!(!ok, "{text}");
    assert!(text.contains("has mode 755; brnr won't use it"), "{text}");

    let (ok, text) = doctor(&env, &["--fix"]);
    assert!(ok, "{text}");
    assert_eq!(mode(&run), 0o700);
}

#[test]
fn symlinked_runtime_dir_is_not_fixed() {
    let env = Env::new("dr-link");
    let target = env.dir.join("elsewhere");
    mkdir(&target, 0o700);
    symlink(&target, env.dir.join("run")).unwrap();

    let (ok, text) = doctor(&env, &["--fix"]);
    assert!(!ok, "{text}");
    assert!(text.contains("is a symlink"), "{text}");
    assert!(fs::symlink_metadata(env.dir.join("run")).unwrap().file_type().is_symlink());
}

#[test]
fn stale_metadata_is_removed() {
    let env = Env::new("dr-stale");
    let run = env.dir.join("run");
    mkdir(&run, 0o700);
    let pid = dead_pid();
    write(
        &run.join(format!("{pid}.json")),
        &format!(r#"{{"id":"{pid}","host_pid":{pid}}}"#),
        0o600,
    );
    write(&run.join(format!("{pid}.sock")), "", 0o600);
    write(&run.join(format!("{pid}.json.tmp")), "", 0o600);

    let (ok, text) = doctor(&env, &[]);
    assert!(ok, "{text}");
    assert!(text.contains("left by processes that are gone"), "{text}");
    assert_eq!(fs::read_dir(&run).unwrap().count(), 3, "doctor without --fix removed something");

    let (ok, text) = doctor(&env, &["--fix"]);
    assert!(ok, "{text}");
    assert_eq!(fs::read_dir(&run).unwrap().count(), 0, "{text}");
}

/// A session lock nobody holds was left by a process that died; one held is
/// a session running.
#[test]
fn stale_session_locks_are_removed() {
    let env = Env::new("dr-locks");
    env.start(&[]);
    let sessions = env.dir.join("run/sessions");
    let stale = sessions.join("old-1.lock");
    write(&stale, &format!(r#"{{"pid":{},"session":"old-1"}}"#, dead_pid()), 0o600);
    let (ok, text) = doctor(&env, &[]);
    assert!(ok, "{text}");
    assert!(text.contains("left by processes that are gone: sessions/old-1.lock"), "{text}");
    assert!(!text.contains("sess-1.lock"), "{text}");
    let (ok, text) = doctor(&env, &["--fix"]);
    assert!(ok, "{text}");
    assert!(!stale.exists(), "{text}");
    assert!(sessions.join("sess-1.lock").exists(), "{text}");
}

/// A directory or a symlink where a session's lock file goes keeps the
/// session from being locked: a failure, not a lock left by a process that
/// is gone, and --fix leaves it to the user. Nor is it locked with sessions/
/// itself open to others (ADR 50).
#[test]
fn adr_0050_what_keeps_a_session_from_being_locked_fails() {
    let env = Env::new("dr-nolock");
    let sessions = env.dir.join("run/sessions");
    mkdir(&sessions.join("sess-1.lock"), 0o700);
    symlink("/dev/null", sessions.join("sess-2.lock")).unwrap();
    for dir in ["run", "run/sessions"] {
        fs::set_permissions(env.dir.join(dir), fs::Permissions::from_mode(0o700)).unwrap();
    }
    for args in [&[][..], &["--fix"]] {
        let (ok, text) = doctor(&env, args);
        assert!(!ok, "{text}");
        let fails = lines(&text, "FAIL");
        assert_eq!(fails.len(), 2, "{text}");
        assert!(fails[0].contains("session locks: sessions/sess-1.lock is a directory, not a lock file: its session can't be locked or started headless; remove it"), "{text}");
        assert!(fails[1].contains("sessions/sess-2.lock is a symlink"), "{text}");
        assert!(!text.contains("left by processes that are gone"), "{text}");
    }
    assert!(sessions.join("sess-1.lock").is_dir());
    fs::set_permissions(&sessions, fs::Permissions::from_mode(0o755)).unwrap();
    let (ok, text) = doctor(&env, &[]);
    assert!(!ok, "{text}");
    assert!(text.contains("no session can be locked or started headless"), "{text}");
}

/// A process binds its socket and listens before it writes its metadata:
/// doctor --fix must leave a process that is starting alone.
#[test]
fn a_starting_process_is_left_alone() {
    let env = Env::new("dr-start");
    let run = env.dir.join("run");
    mkdir(&run, 0o700);
    // This test, starting.
    let pid = std::process::id();
    let _listening = UnixListener::bind(run.join(format!("{pid}.sock"))).unwrap();
    write(&run.join(format!("{pid}.json.tmp")), "", 0o600);

    let (ok, text) = doctor(&env, &["--fix"]);
    assert!(ok, "{text}");
    assert!(!text.contains("removed"), "{text}");
    assert_eq!(fs::read_dir(&run).unwrap().count(), 2, "{text}");
}

#[test]
fn bad_profiles_fail() {
    let env = Env::new("dr-config");
    let config = r#"
[profiles.good]
agent = ["true"]

[profiles.bad]
agent = ["no-such-agent-brnr"]

[profiles.bad.headless]
cwd = "/no/such/dir"

[[profiles.bad.bridges]]
command = ["true"]
events = ["nope"]
"#;
    write(&env.dir.join("none.toml"), config, 0o600);

    let (ok, text) = doctor(&env, &[]);
    assert!(!ok, "{text}");
    assert!(text.contains("ok    profile good"), "{text}");
    let fails = lines(&text, "FAIL  profile bad").len();
    assert_eq!(fails, 3, "{text}");
    for problem in ["no-such-agent-brnr not found", "/no/such/dir", "nope"] {
        assert!(text.contains(problem), "missing {problem}: {text}");
    }
}

/// Each problem with the layout is a check of its own, saying where.
#[test]
fn misplaced_keys_fail() {
    let env = Env::new("dr-layout");
    let config = "[profiles.old]\ncwd = \"/tmp\"\npermission_timeout = 5\n\n[profiles.ok.editor]\nexperimental = [\"nope\"]\n";
    write(&env.dir.join("none.toml"), config, 0o600);
    let (ok, text) = doctor(&env, &[]);
    assert!(!ok, "{text}");
    let fails = lines(&text, "FAIL  config");
    assert_eq!(fails.len(), 3, "{text}");
    assert!(
        fails[0].contains("profiles.ok.editor.experimental: unknown action \"nope\""),
        "{text}"
    );
    assert!(
        fails[1].contains(
            "profiles.old: cwd is for brnr start only; it goes under [profiles.old.headless]"
        ),
        "{text}"
    );
    assert!(fails[2].contains("profiles.old: permission_timeout is for brnr start only"), "{text}");

    write(
        &env.dir.join("none.toml"),
        "[profiles.ok]\nagent = [\"true\"]\nstrict = true\n\n[profiles.ok.editor]\nexperimental = [\"send\", \"approve\"]\nfeatures = [\"shared_sessions\"]\n",
        0o600,
    );
    let (ok, text) = doctor(&env, &[]);
    assert!(ok, "{text}");
    assert!(
        text.contains("ok    profile ok: agent true, 0 bridges, strict, experimental send approve, features shared_sessions"),
        "{text}"
    );
}

#[test]
fn unparseable_config_fails() {
    let env = Env::new("dr-toml");
    write(&env.dir.join("none.toml"), "[profiles.x]\nagnet = 1\n", 0o600);
    let (ok, text) = doctor(&env, &[]);
    assert!(!ok);
    assert!(text.contains("FAIL  config"), "{text}");
}

#[test]
fn runtime_dir_too_long_for_sockets() {
    let env = Env::new("dr-long");
    let long = env.dir.join("x".repeat(100));
    let out = env.brnr(&["doctor"]).env("BRNR_DIR", &long).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(!out.status.success(), "{text}");
    assert!(text.contains("socket paths would be"), "{text}");
}

#[test]
fn relative_runtime_dir_fails() {
    let env = Env::new("dr-rel");
    let out = env.brnr(&["doctor"]).env("BRNR_DIR", "run").output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(!out.status.success(), "{text}");
    assert!(text.contains("BRNR_DIR is relative"), "{text}");
}

/// A process that doesn't answer is running, and what it has is left alone.
#[test]
fn unresponsive_host_warns() {
    let env = Env::new("dr-stuck");
    env.start(&[]);
    let host = env.host_pid();
    kill(host, libc::SIGSTOP);
    let (ok, text) = doctor(&env, &["--fix"]);
    kill(host, libc::SIGCONT);
    assert!(ok, "{text}");
    let warning = format!("{host} is running but not answering; it serves sess-1");
    assert!(text.contains(&warning), "{text}");
    assert!(!text.contains("removed"), "{text}");
    assert!(env.dir.join(format!("run/{host}.sock")).exists(), "{text}");
    assert_eq!(env.hosts().len(), 1, "{text}");
}

/// macOS refuses connections to a stopped process once its backlog is
/// full, as every command that waited on it leaves one queued: it is still
/// running, and not answering.
#[cfg(target_vendor = "apple")]
#[test]
fn a_stopped_host_with_a_full_backlog_is_running() {
    use std::os::unix::net::UnixStream;
    let env = Env::new("dr-full");
    env.start(&[]);
    let host = env.host_pid();
    let socket = env.dir.join(format!("run/{host}.sock"));
    kill(host, libc::SIGSTOP);
    let mut queued = Vec::new();
    while let Ok(conn) = UnixStream::connect(&socket) {
        queued.push(conn);
        assert!(queued.len() < 10_000, "never refused");
    }
    let (ok, text) = doctor(&env, &["--fix"]);
    let list = env.run(&["list", "--json"]);
    kill(host, libc::SIGCONT);
    assert!(ok, "{text}");
    let warning = format!("{host} is running but not answering; it serves sess-1");
    assert!(text.contains(&warning), "{text}");
    assert!(!text.contains("removed"), "{text}");
    assert!(socket.exists() && env.hosts().len() == 1, "{text}");
    let list: Value = serde_json::from_slice(&list.stdout).unwrap();
    assert_eq!((&list[0]["session"], &list[0]["state"]), (&"sess-1".into(), &"unreachable".into()));
}

/// Metadata and a socket whose pid is alive, but another process's now:
/// nobody listens on the socket, so the process that left them is gone.
#[test]
fn a_pid_taken_by_another_process_is_gone() {
    let env = Env::new("dr-ghost");
    let mut sleep = ghost(&env);
    let pid = sleep.id();
    let (ok, text) = doctor(&env, &[]);
    assert!(ok, "{text}");
    let left =
        format!("left by processes that are gone: {pid}.json, {pid}.sock (brnr doctor --fix)");
    assert!(text.contains(&left), "{text}");
    assert!(text.contains("ok    processes: none running"), "{text}");
    assert_eq!(fs::read_dir(env.dir.join("run")).unwrap().count(), 2, "{text}");

    let (ok, text) = doctor(&env, &["--fix"]);
    assert!(ok, "{text}");
    let removed =
        format!("removed what processes that are gone left behind: {pid}.json, {pid}.sock");
    assert!(text.contains(&removed), "{text}");
    assert_eq!(fs::read_dir(env.dir.join("run")).unwrap().count(), 0, "{text}");
    let _ = sleep.kill();
    let _ = sleep.wait();
}

/// Kills the process `info` describes, and its agent, with SIGKILL: the
/// process first, and once it is gone, the agent's group. The other way
/// round, the process can see its agent die and record its own exit
/// (`exited`) before its SIGKILL lands.
fn kill_unrecorded(info: &Value) {
    let pid = |key: &str| info[key].as_i64().unwrap() as i32;
    kill(pid("host_pid"), libc::SIGKILL);
    assert!(wait_for(Duration::from_secs(10), || !alive(pid("host_pid"))), "it didn't die");
    kill(-pid("agent_pid"), libc::SIGKILL);
}

/// A process killed without a word (SIGKILL) leaves a host log without
/// `exited` (ADR 11): doctor says so, with the sessions it had open, and
/// --fix leaves the record. One that stopped recorded it.
#[test]
fn a_death_without_a_record_is_reported() {
    let env = Env::new("dr-died");
    env.start(&[]);
    let stopped = env.hosts().remove(0);
    env.stop();
    let pid = stopped["host_pid"].as_i64().unwrap() as i32;
    assert!(wait_for(Duration::from_secs(10), || !alive(pid)), "it didn't stop");
    let (_, text) = doctor(&env, &[]);
    assert!(
        text.contains("ok    host logs: no process died without recording it (1 log)"),
        "{text}"
    );

    env.start(&[]);
    let killed = env.hosts().remove(0);
    // Once its log has the session (`log` waits for its logger to write).
    env.ok(&["log", "sess-1"]);
    kill_unrecorded(&killed);
    let (ok, text) = doctor(&env, &["--fix"]);
    assert!(ok, "{text}");
    let run = killed["host_id"].as_str().unwrap();
    let line = lines(&text, "--    host logs");
    let start =
        format!("--    host logs: 1 process died without recording it: {run} (last record ");
    assert!(line.len() == 1 && line[0].starts_with(&start), "{text}");
    assert!(line[0].ends_with("; session sess-1)"), "{text}");
    assert!(!text.contains(stopped["host_id"].as_str().unwrap()), "{text}");
    assert!(env.dir.join(format!("home/hosts/{run}.jsonl")).exists(), "--fix removed it");

    // The same check in JSON.
    let out = env.run(&["doctor", "--json"]);
    let checks: Value = serde_json::from_slice(&out.stdout).unwrap();
    let check = checks.as_array().unwrap().iter().find(|c| c["check"] == "host logs").unwrap();
    assert_eq!(check["level"], "info", "{check}");
    assert!(check["message"].as_str().unwrap().contains(run), "{check}");
}

/// A start that fails as its bridges start records its end too.
#[test]
fn a_failed_start_is_no_death() {
    let env = Env::new("dr-failed");
    env.write_config("[[profiles.b.bridges]]\ncommand = [\"/no/such/brnr-bridge\"]\n");
    let out = env.run(&start_args(&["--profile", "b"]));
    assert!(!out.status.success(), "it started");
    let (_, text) = doctor(&env, &[]);
    assert!(
        text.contains("ok    host logs: no process died without recording it (1 log)"),
        "{text}"
    );
}

#[test]
fn adr_0039_adapter_versions() {
    let env = Env::new("dr-versions");
    let bin = env.dir.join("bin");
    mkdir(&bin, 0o755);
    // A brnr adapter that knows --version.
    let claude = "#!/bin/sh\n[ \"$1\" = --version ] && echo 'brnr-claude-adapter 0.85.1 (@agentclientprotocol/claude-agent-acp)'\n";
    script(&bin.join("brnr-claude-adapter"), claude);
    // One built before --version: it waits for an editor instead.
    script(&bin.join("brnr-codex-adapter"), "#!/bin/sh\nexec sleep 30\n");
    // codex-acp from npm: a bin link into the package.
    let package = env.dir.join("lib/node_modules/@agentclientprotocol/codex-acp");
    mkdir(&package.join("dist"), 0o755);
    write(
        &package.join("package.json"),
        r#"{"name":"@agentclientprotocol/codex-acp","version":"2.1.1"}"#,
        0o644,
    );
    script(&package.join("dist/index.js"), "#!/usr/bin/env node\n");
    symlink(
        "../lib/node_modules/@agentclientprotocol/codex-acp/dist/index.js",
        bin.join("codex-acp"),
    )
    .unwrap();

    // A copy of brnr with nothing next to it (where it looks first: in a
    // build, the adapters may well be next to the real one).
    let alone = env.dir.join("alone");
    mkdir(&alone, 0o755);
    install(Path::new(env!("CARGO_BIN_EXE_brnr")), &alone.join("brnr"));
    let path = format!("{}:/usr/bin:/bin", bin.display());
    let out = env.brnr_at(&alone.join("brnr"), &["doctor"]).env("PATH", path).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let line = |name: &str| {
        text.lines().find(|l| l.contains(&format!(" {name}: "))).unwrap_or("").to_owned()
    };
    assert!(
        line("brnr-claude-adapter").ends_with("(@agentclientprotocol/claude-agent-acp 0.85.1)"),
        "{text}"
    );
    assert!(
        line("brnr-codex-adapter").ends_with("(version unknown: built before brnr 0.3.0)"),
        "{text}"
    );
    assert!(line("codex-acp").ends_with("(@agentclientprotocol/codex-acp 2.1.1)"), "{text}");
    assert!(line("claude-agent-acp").starts_with("--"), "{text}");
}

/// `doctor --report` (ADR 45): the version, the OS, the adapters, the checks
/// that aren't ok, and the end of the latest host log and of the latest of a
/// process that panicked or died without recording it, with secrets
/// redacted, even where an older brnr didn't, and the home directory as `~`.
/// `--json` has the same data.
#[test]
fn adr_0045_the_report_is_what_to_paste() {
    let env = Env::new("dr-report");
    let secret = "s3cret-token-value";
    env.write_config(&format!(
        "[[profiles.default.headless.mcp_servers]]\nname = \"files\"\ncommand = \"true\"\nenv = {{ TOKEN = \"{secret}\" }}\n"
    ));
    let hosts = env.dir.join("home/hosts");
    mkdir(&env.dir.join("home"), 0o700);
    mkdir(&hosts, 0o700);
    // An older brnr's log of a panic, its secret not redacted, and a log of
    // a process that exited, older still: not shown.
    let opened = format!(
        r#"{{"ts":"t","dir":"control->agent","msg":{{"jsonrpc":"2.0","id":1,"method":"session/new","params":{{"cwd":"{}","mcpServers":[{{"name":"files","command":"true","args":[],"env":[{{"name":"TOKEN","value":"{secret}"}}]}}]}}}}}}"#,
        env.dir.display()
    );
    let panicked = format!(
        "{opened}\n{}\n{}\n",
        r#"{"ts":"t","event":{"event":"panic","error":"brnr panicked at src/x.rs:1:1: oops"}}"#,
        r#"{"ts":"t","event":{"event":"exited","status":null,"reason":"brnr panicked at src/x.rs:1:1: oops"}}"#
    );
    write(&hosts.join("20260101T000000-1.jsonl"), &panicked, 0o600);
    write(&hosts.join("20250101T000000-2.jsonl"), "{\"event\":{\"event\":\"exited\"}}\n", 0o600);
    let old = std::time::SystemTime::now() - Duration::from_secs(3600);
    for (name, age) in [("20260101T000000-1", 1), ("20250101T000000-2", 2)] {
        let file = fs::File::options().write(true).open(hosts.join(format!("{name}.jsonl")));
        file.unwrap().set_modified(old - Duration::from_secs(age * 60)).unwrap();
    }
    env.start(&[]);
    let running = env.hosts().remove(0)["host_id"].as_str().unwrap().to_owned();
    // Once its log has its start (`log` waits for its logger to write it).
    env.ok(&["log", "sess-1"]);
    // Something not ok.
    write(&hosts.join("open.txt"), "", 0o644);

    let home = env.dir.to_str().unwrap();
    let out = env.brnr(&["doctor", "--report"]).env("HOME", home).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(text.starts_with("<!-- brnr doctor --report: read it before you paste it"), "{text}");
    let version = format!("**brnr {}** on ", env!("CARGO_PKG_VERSION"));
    assert!(text.contains(&version), "{text}");
    assert!(text.contains("- brnr-claude-adapter: "), "{text}");
    assert!(text.contains("- warn transcripts: 1 of "), "{text}");
    assert!(!text.contains("- ok "), "{text}");
    assert!(text.contains(&format!("**Host log `{running}`**: running; its last ")), "{text}");
    let panic = "**Host log `20260101T000000-1`**: brnr panicked; its last 3 lines";
    assert!(text.contains(panic), "{text}");
    assert!(!text.contains("20250101T000000-2"), "{text}");
    assert!(text.contains("```jsonl\n"), "{text}");
    assert!(!text.contains(secret), "{text}");
    // The older brnr's session/new, and the running one's and its started
    // request.
    assert_eq!(text.matches(r#""value":"<redacted>""#).count(), 3, "{text}");
    assert!(!text.contains(home) && text.contains(r#""cwd":"~""#), "{text}");

    let out = env.brnr(&["doctor", "--report", "--json"]).env("HOME", home).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["brnr"], env!("CARGO_PKG_VERSION"));
    assert!(text.contains(&format!(" on {}\n", report["os"].as_str().unwrap())), "{report}");
    assert_eq!(report["adapters"].as_array().unwrap().len(), 4, "{report}");
    assert_eq!(report["checks"][0]["level"], "warn", "{report}");
    let logs = report["host_logs"].as_array().unwrap();
    assert_eq!(logs.len(), 2, "{report}");
    assert_eq!((&logs[0]["run"], &logs[0]["ended"]), (&running.as_str().into(), &"running".into()));
    assert_eq!(logs[1]["ended"], "brnr panicked", "{report}");
    for log in logs {
        for line in log["lines"].as_array().unwrap() {
            assert!(text.contains(line.as_str().unwrap()), "{line}");
        }
    }

    assert!(env.fails(&["doctor", "--fix", "--report"]).contains("don't go together"));
}

/// A process killed without a word is the one the report shows, besides
/// the latest.
#[test]
fn adr_0045_the_report_shows_a_death_without_a_record() {
    let env = Env::new("dr-rdied");
    env.start(&[]);
    let killed = env.hosts().remove(0);
    kill_unrecorded(&killed);
    let out = env.run(&["doctor", "--report", "--json"]);
    let report: Value = serde_json::from_slice(&out.stdout).expect("a report");
    let logs = report["host_logs"].as_array().unwrap();
    assert_eq!(logs.len(), 1, "the latest is the one that died: {report}");
    assert_eq!(logs[0]["run"], killed["host_id"], "{report}");
    assert_eq!(logs[0]["ended"], "died without recording it", "{report}");
    let checks = report["checks"].as_array().unwrap();
    let check = checks.iter().find(|c| c["check"] == "host logs").expect("host logs");
    assert_eq!(check["level"], "info", "{report}");
}
