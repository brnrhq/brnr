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
    env.start("a", &["--prompt", "hello"]);
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
    write(&project.join("s.jsonl"), "{}\n", 0o644);

    let (ok, text) = doctor(&env, &[]);
    assert!(ok, "warnings alone don't fail: {text}");
    assert_eq!(lines(&text, "warn").len(), 1, "{text}");
    assert!(text.contains("4 of 4 paths can be read by others"), "{text}");

    let (ok, text) = doctor(&env, &["--fix"]);
    assert!(ok, "{text}");
    assert!(text.contains("made 4 paths private"), "{text}");
    for dir in [&home, &home.join("projects"), &project] {
        assert_eq!(mode(dir), 0o700, "{}", dir.display());
    }
    assert_eq!(mode(&project.join("s.jsonl")), 0o600);
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
    write(&run.join("1.json.tmp"), "", 0o600);

    let (ok, text) = doctor(&env, &[]);
    assert!(ok, "{text}");
    assert!(text.contains("left by hosts that are gone"), "{text}");
    assert_eq!(fs::read_dir(&run).unwrap().count(), 3, "doctor without --fix removed something");

    let (ok, text) = doctor(&env, &["--fix"]);
    assert!(ok, "{text}");
    assert_eq!(fs::read_dir(&run).unwrap().count(), 0, "{text}");
}

#[test]
fn bad_profiles_fail() {
    let env = Env::new("dr-config");
    let config = r#"
[profiles.good]
agent = ["true"]

[profiles.bad]
agent = ["no-such-agent-brnr"]
cwd = "/no/such/dir"
permissions = "maybe"
on_disconnect = "later"

[[profiles.bad.bridges]]
command = ["true"]
events = ["nope"]
"#;
    write(&env.dir.join("none.toml"), config, 0o600);

    let (ok, text) = doctor(&env, &[]);
    assert!(!ok, "{text}");
    assert!(text.contains("ok    profile good"), "{text}");
    let fails = lines(&text, "FAIL  profile bad").len();
    assert_eq!(fails, 5, "{text}");
    for problem in ["no-such-agent-brnr not found", "/no/such/dir", "maybe", "later", "nope"] {
        assert!(text.contains(problem), "missing {problem}: {text}");
    }
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
    env.start("a", &[]);
    let host = env.host_pid();
    unsafe { libc::kill(host, libc::SIGSTOP) };
    let (ok, text) = doctor(&env, &[]);
    unsafe { libc::kill(host, libc::SIGCONT) };
    assert!(ok, "{text}");
    assert!(text.contains(&format!("(pid {host}) is running but not answering")), "{text}");
}

#[test]
#[allow(clippy::zombie_processes)] // Reaped by the environment; see below.
fn shared_name_warns() {
    let env = Env::new("dr-name");
    let run = env.dir.join("run");
    mkdir(&run, 0o700);
    // Two editors' hosts may share a name; a live process stands in for
    // them. The environment kills it when the test ends; it isn't reaped
    // before then, so its pid can't be reused meanwhile.
    let stand_in = Command::new("sleep").arg("60").spawn().unwrap();
    let pid = stand_in.id();
    for id in ["1", "2"] {
        let meta =
            format!(r#"{{"id":"{id}","name":"x","host_pid":{pid},"socket":"/nonexistent"}}"#);
        write(&run.join(format!("{id}.json")), &meta, 0o600);
    }
    let (_, text) = doctor(&env, &[]);
    assert!(text.contains("x names several hosts: 1, 2"), "{text}");
}
