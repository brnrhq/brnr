//! `brnr doctor`: what it finds, and what `--fix` repairs.

mod common;

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::Path;
use std::process::{Command, Output};

use common::*;

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

/// A process binds its socket before it writes its metadata: doctor --fix
/// must leave a process that is starting alone.
#[test]
fn a_starting_process_is_left_alone() {
    let env = Env::new("dr-start");
    let run = env.dir.join("run");
    mkdir(&run, 0o700);
    let pid = std::process::id();
    write(&run.join(format!("{pid}.sock")), "", 0o600);
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

#[test]
fn unresponsive_host_warns() {
    let env = Env::new("dr-stuck");
    env.start(&[]);
    let host = env.host_pid();
    unsafe { libc::kill(host, libc::SIGSTOP) };
    let (ok, text) = doctor(&env, &[]);
    unsafe { libc::kill(host, libc::SIGCONT) };
    assert!(ok, "{text}");
    assert!(text.contains(&format!("{host} is running but not answering")), "{text}");
}

#[test]
fn adapter_versions() {
    let env = Env::new("dr-versions");
    let bin = env.dir.join("bin");
    mkdir(&bin, 0o755);
    // A brnr adapter that knows --version.
    let claude = "#!/bin/sh\n[ \"$1\" = --version ] && echo 'brnr-claude-adapter 0.85.1 (@agentclientprotocol/claude-agent-acp)'\n";
    write(&bin.join("brnr-claude-adapter"), claude, 0o755);
    // One built before --version: it waits for an editor instead.
    write(&bin.join("brnr-codex-adapter"), "#!/bin/sh\nexec sleep 30\n", 0o755);
    // codex-acp from npm: a bin link into the package.
    let package = env.dir.join("lib/node_modules/@agentclientprotocol/codex-acp");
    mkdir(&package.join("dist"), 0o755);
    write(
        &package.join("package.json"),
        r#"{"name":"@agentclientprotocol/codex-acp","version":"2.1.1"}"#,
        0o644,
    );
    write(&package.join("dist/index.js"), "#!/usr/bin/env node\n", 0o755);
    symlink(
        "../lib/node_modules/@agentclientprotocol/codex-acp/dist/index.js",
        bin.join("codex-acp"),
    )
    .unwrap();

    // A copy of brnr with nothing next to it (where it looks first: in a
    // build, the adapters may well be next to the real one).
    let alone = env.dir.join("alone");
    mkdir(&alone, 0o755);
    fs::copy(env!("CARGO_BIN_EXE_brnr"), alone.join("brnr")).unwrap();
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
