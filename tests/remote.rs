//! SSH routing exercises the real destination command through a shell,
//! with a fake ssh executable so the tests require no keys or network.
mod common;

use common::{AGENT, Env};
use serde_json::{Value, json};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Stdio};

struct Setup {
    local: Env,
    remote: Env,
}

impl Setup {
    fn new(name: &str) -> Self {
        let local = Env::new(&format!("remote-local-{name}"));
        let remote = Env::new(&format!("remote-host-{name}"));
        let bin = local.dir.join("bin");
        fs::create_dir_all(&bin).unwrap();
        std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_brnr"), bin.join("brnr")).unwrap();
        let ssh = bin.join("ssh");
        fs::write(
            &ssh,
            r#"#!/usr/bin/env python3
import json
import os
import sys
with open(os.environ['SSH_ARGUMENTS'], 'a') as log:
    log.write(json.dumps(sys.argv[1:]) + '\n')
if os.environ.get('SSH_FAIL'):
    print('ssh: authentication failed', file=sys.stderr)
    sys.exit(int(os.environ['SSH_FAIL']))
assert sys.argv[1:6] == ['-T', '-o', 'BatchMode=yes', '-o', 'StrictHostKeyChecking=yes']
os.environ['BRNR_CONFIG'] = os.environ['SSH_CONFIG']
os.environ['BRNR_HOME'] = os.environ['SSH_HOME']
os.environ['BRNR_DIR'] = os.environ['SSH_DIR']
if 'SSH_PATH' in os.environ:
    os.environ['PATH'] = os.environ['SSH_PATH']
os.execv('/bin/sh', ['sh', '-c', sys.argv[-1]])
"#,
        )
        .unwrap();
        fs::set_permissions(&ssh, fs::Permissions::from_mode(0o755)).unwrap();
        local.write_config("");
        remote.write_config(&format!("[profiles.coding]\nagent = ['python3', {:?}]\n", AGENT));
        Self { local, remote }
    }

    fn cmd(&self, args: &[&str]) -> Command {
        let mut cmd = self.local.brnr(args);
        let path = std::env::join_paths(
            std::iter::once(self.local.dir.join("bin"))
                .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())),
        )
        .unwrap();
        cmd.env("PATH", path)
            .env("SSH_ARGUMENTS", self.local.dir.join("ssh-arguments"))
            .env("SSH_CONFIG", self.remote.config())
            .env("SSH_HOME", self.remote.dir.join("home"))
            .env("SSH_DIR", self.remote.dir.join("run"));
        cmd
    }

    fn run(&self, args: &[&str]) -> Value {
        let out = self.cmd(args).output().unwrap();
        assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
        serde_json::from_slice(&out.stdout).unwrap()
    }

    fn arguments(&self) -> Vec<Value> {
        fs::read_to_string(self.local.dir.join("ssh-arguments"))
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }
}

#[test]
fn adr_0012_remote_sessions_stdin_and_json_use_the_destination() {
    let s = Setup::new("sessions");
    let new = s.run(&["session", "new", "--remote", "linux", "--profile", "coding", "--json"]);
    let id = new["session"].as_str().unwrap();
    let listed = s.run(&["session", "list", "--remote", "linux", "--include", "active", "--json"]);
    assert_eq!(listed["sessions"][0]["session"], id);
    let local = s.local.run(&["session", "list", "--include", "active", "--json"]);
    assert_eq!(serde_json::from_slice::<Value>(&local.stdout).unwrap()["sessions"], json!([]));
    let mut child = s
        .cmd(&["prompt", "send", id, "--remote", "linux", "--prompt", "-", "--wait", "--json"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"reply hello through stdin").unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let reply: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(reply.to_string().contains("hello through stdin"), "{reply}");
    let log = s.cmd(&["event", "log", id, "--remote", "linux", "--json"]).output().unwrap();
    assert!(log.status.success());
    assert!(String::from_utf8_lossy(&log.stdout).contains("hello through stdin"));
    assert_eq!(s.arguments().len(), 4);
    assert_eq!(s.arguments()[0][5], "linux");
}

/// Without `--remote`, `--profile` is a local profile and nothing connects:
/// a local start stays local, and a verb that takes no `--profile` refuses it.
#[test]
fn adr_0012_profile_without_remote_never_connects() {
    let s = Setup::new("local-profile");
    s.local.write_config(&format!(
        "[profiles.default]\nagent = ['python3', {AGENT:?}]\n\n[profiles.coding]\nagent = ['python3', {AGENT:?}]\n"
    ));
    let new = s.run(&["session", "new", "--profile", "coding", "--json"]);
    let id = new["session"].as_str().unwrap();
    let local = s.run(&["session", "list", "--profile", "coding", "--include", "active", "--json"]);
    assert!(local["sessions"].as_array().unwrap().iter().any(|r| r["session"] == id), "{local}");
    let out = s.cmd(&["session", "status", id, "--profile", "coding"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stderr).contains("unknown option: --profile"));
    assert!(s.arguments().is_empty(), "{:?}", s.arguments());
}

/// With `--remote`, `--profile` goes to the destination for every verb that
/// takes it, and the destination loads it from its own config; a verb that
/// takes none refuses it before connecting.
#[test]
fn adr_0012_with_remote_profile_reaches_the_destination_for_every_verb_that_takes_it() {
    let s = Setup::new("remote-profile");
    let new = s.run(&["session", "new", "--remote", "linux", "--profile", "coding", "--json"]);
    let id = new["session"].as_str().unwrap().to_owned();
    let listed = s.run(&[
        "session",
        "list",
        "--remote",
        "linux",
        "--profile",
        "coding",
        "--include",
        "active",
        "--json",
    ]);
    assert_eq!(listed["sessions"][0]["session"], id.as_str());
    for args in [
        vec!["session", "resume", &id, "--remote", "linux", "--profile", "coding", "--json"],
        vec!["session", "delete", &id, "--remote", "linux", "--profile", "coding", "--json"],
        vec!["acp", "--remote", "linux", "--profile", "coding"],
    ] {
        let out = s.cmd(&args).stdin(Stdio::null()).output().unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_ne!(out.status.code(), Some(2), "{args:?}: {stderr}");
        assert!(!stderr.contains("profile_not_found"), "{args:?}: {stderr}");
    }
    let arguments = s.arguments();
    assert_eq!(arguments.len(), 5);
    for sent in &arguments {
        assert!(sent[6].as_str().unwrap().contains("--profile coding"), "{sent}");
    }
    // The destination's profile isn't the source's: a name only the source
    // has fails there.
    s.local.write_config("[profiles.source-only]\n");
    let out = s
        .cmd(&["session", "list", "--remote", "linux", "--profile", "source-only", "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    common::code_is(&out, "profile_not_found");
    let before = s.arguments().len();
    let out = s.cmd(&["process", "list", "--remote", "linux", "--profile", "coding"]).output();
    assert_eq!(out.unwrap().status.code(), Some(2));
    assert_eq!(s.arguments().len(), before);
}

/// `brnr profile show <name> --remote` shows the destination's profile,
/// from its config, through the fake SSH client; the source's config isn't
/// read (ADR 6.5).
#[test]
fn adr_0006_profile_show_reaches_the_destination() {
    let s = Setup::new("profile-show");
    s.local.write_config("[profiles.coding]\nstrict = true\n");
    let shown = s.run(&["profile", "show", "coding", "--remote", "linux", "--json"]);
    assert_eq!((&shown["name"], &shown["found"]), (&json!("coding"), &json!(true)));
    assert_eq!(shown["config"], s.remote.config().to_str().unwrap());
    assert_eq!(shown["profile"]["agent"], json!(["python3", AGENT]));
    assert_eq!(shown["profile"]["strict"], false);
    let listed = s.run(&["profile", "list", "--remote", "linux", "--json"]);
    assert_eq!(listed["profiles"][0]["name"], "coding");
    let keys = s.run(&["profile", "keys", "--remote", "linux", "--json"]);
    assert!(keys["keys"].as_array().is_some_and(|k| !k.is_empty()));
    let new = s.run(&["session", "new", "--remote", "linux", "--profile", "coding", "--json"]);
    let pid = new["pid"].as_u64().unwrap().to_string();
    let started = s.run(&["profile", "show", "--pid", &pid, "--remote", "linux", "--json"]);
    assert_eq!(started["start_request"]["profile"], "coding");
    let mut out = s.cmd(&["profile", "show", "coding", "--remote", "linux", "--profile", "x"]);
    assert_eq!(out.output().unwrap().status.code(), Some(2));
    assert_eq!(s.arguments().len(), 5);
    for sent in s.arguments() {
        assert!(sent[6].as_str().unwrap().contains("_remote"), "{sent}");
    }
}

/// The destination runs the command itself: a `--remote` it receives is
/// refused, never another hop.
#[test]
fn adr_0012_nested_remote_routing_is_refused() {
    let s = Setup::new("nested");
    let out = s.cmd(&["_remote", "session", "list", "--remote", "other", "--json"]).output();
    let out = out.unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("nested remote routing"));
    assert!(s.arguments().is_empty());
}

#[test]
fn adr_0012_shell_quoting_preserves_prompts_and_delimiters() {
    let s = Setup::new("quoting");
    let new = s.run(&["session", "new", "--remote", "linux", "--profile", "coding", "--json"]);
    let id = new["session"].as_str().unwrap();
    let payload = format!(
        "reply quotes ' \" ; $(touch {})\n--remote hostile",
        s.local.dir.join("injected").display()
    );
    let result = s.run(&[
        "prompt", "send", id, "--remote", "linux", "--prompt", &payload, "--wait", "--json",
    ]);
    assert!(result.to_string().contains("hostile"));
    assert!(!s.local.dir.join("injected").exists());
    // A value that looks like --remote is still a prompt, with no SSH call.
    let before = s.arguments().len();
    let out = s.cmd(&["prompt", "send", "missing", "--prompt", "--remote"]).output().unwrap();
    assert!(!out.status.success());
    assert_eq!(s.arguments().len(), before);
    // Callback words and --remote inside them belong to the callback.
    let out = s
        .cmd(&["event", "notify", "--remote", "linux", "--stdin", "--", "printf", "%s", "--remote"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("stdin closed"));
    assert!(s.arguments().last().unwrap()[6].as_str().unwrap().ends_with("-- printf %s --remote"));
}

#[test]
fn adr_0012_auth_missing_client_version_and_bad_targets_fail() {
    let s = Setup::new("errors");
    let out =
        s.cmd(&["session", "list", "--remote", "linux"]).env("SSH_FAIL", "255").output().unwrap();
    assert_eq!(out.status.code(), Some(255));
    assert!(String::from_utf8_lossy(&out.stderr).contains("authentication failed"));
    let out = s
        .cmd(&["session", "list", "--remote", "linux", "--json"])
        .env("PATH", "")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("cannot execute ssh"));
    assert!(serde_json::from_slice::<Value>(&out.stdout).unwrap().get("error").is_some());
    let before = s.arguments().len();
    for target in ["-oProxyCommand=touch", "x; touch /tmp/injected", "", "user@host\n"] {
        let out = s.cmd(&["session", "list", "--remote", target]).output().unwrap();
        assert!(!out.status.success());
    }
    assert_eq!(s.arguments().len(), before);
    // brnr on the remote PATH, of another version.
    let wrong = s.local.dir.join("wrong");
    fs::create_dir_all(&wrong).unwrap();
    fs::write(wrong.join("brnr"), "#!/bin/sh\nprintf 'brnr 99.0.0\\n'\n").unwrap();
    fs::set_permissions(wrong.join("brnr"), fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!("{}:/usr/bin:/bin", wrong.display());
    let mut list = s.cmd(&["session", "list", "--remote", "linux", "--json"]);
    let out = list.env("SSH_PATH", &path).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("another version"));
    common::code_is(&out, "remote_version_mismatch");
}

/// brnr not on the remote shell's PATH: the error says how to fix that
/// shell's environment, with its code, in text and JSON.
#[test]
fn adr_0012_a_missing_remote_brnr_says_how_to_fix_the_path() {
    let s = Setup::new("missing");
    for json in [false, true] {
        let mut args = vec!["session", "list", "--remote", "linux"];
        if json {
            args.push("--json");
        }
        let out = s.cmd(&args).env("SSH_PATH", "/usr/bin:/bin").output().unwrap();
        assert_eq!(out.status.code(), Some(1));
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("~/.zshenv") && stderr.contains("~/.bashrc"), "{stderr}");
        common::code_is(&out, "remote_brnr_not_found");
        if json {
            let error: Value = serde_json::from_slice(&out.stdout).unwrap();
            assert_eq!(error["error"]["code"], "remote_brnr_not_found");
        } else {
            assert!(out.stdout.is_empty());
        }
    }
}

#[test]
fn adr_0012_direct_user_destinations_use_remote_path_without_initialisation() {
    let s = Setup::new("direct");
    fs::remove_file(s.local.config()).unwrap();
    let listed = s.run(&["session", "list", "--remote", "agent@linux", "--json"]);
    assert_eq!(listed["sessions"], json!([]));
    assert_eq!(s.arguments()[0][5], "agent@linux");
    let version = s.cmd(&["-V", "--remote", "agent@linux"]).output().unwrap();
    assert!(version.status.success());
    assert_eq!(
        String::from_utf8_lossy(&version.stdout),
        concat!("brnr ", env!("CARGO_PKG_VERSION"), "\n")
    );
    let invalid = s.cmd(&["--version", "--remote"]).output().unwrap();
    assert_eq!(invalid.status.code(), Some(2));
    assert_eq!(s.arguments().len(), 2);
}

/// A profile's `remote` table is gone: it is an unknown key, which fails to
/// load, and doctor names it; nothing connects.
#[test]
fn adr_0012_a_remote_table_fails_to_load_and_doctor_names_it() {
    let s = Setup::new("remote-table");
    s.local.write_config("[profiles.work.remote]\ntarget = 'linux'\n");
    for args in
        [vec!["session", "list", "--profile", "work", "--json"], vec!["session", "new", "--json"]]
    {
        let out = s.cmd(&args).output().unwrap();
        assert_eq!(out.status.code(), Some(1), "{args:?}: {out:?}");
        common::code_is(&out, "config_invalid");
        assert!(String::from_utf8_lossy(&out.stderr).contains("unknown key remote"));
    }
    let doctor = s.local.run(&["doctor", "--json"]);
    assert!(!doctor.status.success());
    let checks: Value = serde_json::from_slice(&doctor.stdout).unwrap();
    let named = checks["checks"].as_array().unwrap().iter().any(|c| {
        c["level"] == "fail" && c["message"].as_str().unwrap().contains("unknown key remote")
    });
    assert!(named, "{checks}");
    // `--version` takes no `--profile`; alone it is local.
    let out = s.cmd(&["--version", "--profile", "work"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(s.cmd(&["--version"]).output().unwrap().status.success());
    assert!(s.arguments().is_empty());
}

#[test]
fn adr_0012_remote_checks_never_write_or_require_local_configuration() {
    let s = Setup::new("inspection");
    let original = fs::read(s.local.config()).unwrap();
    fs::set_permissions(s.local.config(), fs::Permissions::from_mode(0o400)).unwrap();
    fs::set_permissions(s.local.config().parent().unwrap(), fs::Permissions::from_mode(0o500))
        .unwrap();
    let checked = s.cmd(&["remote", "check", "agent@linux", "--json"]).output().unwrap();
    let result: Value = serde_json::from_slice(&checked.stdout).unwrap();
    assert!(result["checks"].is_array(), "{result}");
    assert_eq!(s.arguments()[0][5], "agent@linux");
    for verb in ["init", "remove", "list"] {
        let out = s.cmd(&["remote", verb, "--json"]).output().unwrap();
        assert_eq!(out.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&out.stderr).contains("unknown command"));
    }
    assert_eq!(s.arguments().len(), 1);
    assert_eq!(fs::read(s.local.config()).unwrap(), original);
    assert_eq!(fs::metadata(s.local.config()).unwrap().permissions().mode() & 0o777, 0o400);
    fs::set_permissions(s.local.config().parent().unwrap(), fs::Permissions::from_mode(0o700))
        .unwrap();
    fs::set_permissions(s.local.config(), fs::Permissions::from_mode(0o600)).unwrap();
    fs::remove_file(s.local.config()).unwrap();
    fs::remove_dir(s.local.config().parent().unwrap()).unwrap();
    let checked = s.cmd(&["remote", "check", "linux", "--json"]).output().unwrap();
    assert!(serde_json::from_slice::<Value>(&checked.stdout).unwrap()["checks"].is_array());
    assert!(!s.local.config().parent().unwrap().exists());
}

#[test]
fn adr_0012_ssh_destinations_bypass_local_config_and_legacy_registry_is_rejected() {
    let s = Setup::new("no-registry");
    s.local.write_config("[remotes.linux]\nhost='wrong-host'\n");
    let original = fs::read(s.local.config()).unwrap();
    assert_eq!(s.run(&["session", "list", "--remote", "linux", "--json"])["sessions"], json!([]));
    assert_eq!(s.arguments()[0][5], "linux");
    let check = s.cmd(&["remote", "check", "linux", "--json"]).output().unwrap();
    assert!(serde_json::from_slice::<Value>(&check.stdout).unwrap()["checks"].is_array());
    let doctor = s.local.run(&["doctor", "--json"]);
    assert!(!doctor.status.success());
    assert!(String::from_utf8_lossy(&doctor.stdout).contains("unknown key remotes"));
    assert_eq!(fs::read(s.local.config()).unwrap(), original);
}

#[test]
fn adr_0012_acp_has_clean_streams_and_remote_profile_resolution() {
    let s = Setup::new("acp");
    acp(&s);
    assert_eq!(s.arguments().len(), 1);
}

fn acp(s: &Setup) {
    acp_command(s.cmd(&["acp", "--remote=linux", "--profile=coding"]));
}

fn acp_command(mut command: Command) {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    writeln!(input, "{}", json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{}}})).unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    output.read_line(&mut line).unwrap();
    let reply: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(reply["id"], 1);
    assert!(reply.get("result").is_some(), "{reply}");
    drop(input);
    let status = child.wait().unwrap();
    assert!(status.success(), "{status}");
}

#[test]
fn adr_0012_help_and_invalid_commands_never_connect() {
    let s = Setup::new("syntax");
    for args in [
        vec!["session", "list", "--remote", "linux", "--help"],
        vec!["acp", "--remote", "linux", "--help"],
    ] {
        assert!(s.cmd(&args).output().unwrap().status.success());
    }
    for args in [
        vec!["session", "status", "--remote", "linux"],
        vec!["session", "list", "--remote", "linux", "--bogus"],
        vec!["config", "set", "s", "--remote", "linux"],
        vec!["session", "list", "--remote"],
        vec!["remote", "check"],
        vec!["remote", "check", "linux", "--profile", "work"],
        vec!["remote", "check", "--profile"],
    ] {
        assert_eq!(s.cmd(&args).output().unwrap().status.code(), Some(2), "{args:?}");
    }
    assert!(s.arguments().is_empty());
}

#[test]
fn adr_0012_event_streams_deliver_live_events_and_disconnect_without_stopping_headless() {
    let s = Setup::new("streams");
    let new = s.run(&["session", "new", "--remote", "linux", "--profile", "coding", "--json"]);
    let id = new["session"].as_str().unwrap();
    let mut watcher = s
        .cmd(&["event", "watch", id, "--remote", "linux", "--json"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let (sent, received) = std::sync::mpsc::channel();
    let output = watcher.stdout.take().unwrap();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(output).lines() {
            let Ok(line) = line else { break };
            if line.contains("streamed-reply") {
                let _ = sent.send(line);
                break;
            }
        }
    });
    let limit = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while s.remote.subscriptions() == 0 {
        assert!(std::time::Instant::now() < limit, "watcher never subscribed");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    s.run(&[
        "prompt",
        "send",
        id,
        "--remote",
        "linux",
        "--prompt",
        "reply streamed-reply",
        "--wait",
        "--json",
    ]);
    let event = received.recv_timeout(std::time::Duration::from_secs(5));
    watcher.kill().unwrap();
    watcher.wait().unwrap();
    reader.join().unwrap();
    assert!(event.is_ok(), "stream did not deliver the reply");
    let status = s.run(&["session", "status", id, "--remote", "linux", "--json"]);
    assert_eq!(status["session"], id);
}

/// Linux CI runs the transport against an isolated real OpenSSH daemon.
/// Keys, known_hosts and client configuration exist only in the test folder.
#[cfg(target_os = "linux")]
#[test]
fn adr_0012_system_ssh_authenticates_and_runs_cli_and_acp_on_linux() {
    use std::net::{TcpListener, TcpStream};
    use std::time::{Duration, Instant};

    for (program, flag) in [("ssh-keygen", "-?"), ("/usr/sbin/sshd", "-V"), ("/usr/bin/ssh", "-V")]
    {
        // The version/help exit status varies; only inability to execute means
        // the prerequisite is missing. Setup failures below remain failures.
        if let Err(error) = Command::new(program).arg(flag).output() {
            common::skip(&format!("OpenSSH prerequisite {program} unavailable: {error}"));
            return;
        }
    }

    struct Daemon(std::process::Child);
    impl Drop for Daemon {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let s = Setup::new("openssh");
    let dir = &s.local.dir;
    for name in ["host-key", "client-key"] {
        let output = Command::new("ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", "", "-f"])
            .arg(dir.join(name))
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    }
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let user = Command::new("id").arg("-un").output().unwrap();
    let user = String::from_utf8(user.stdout).unwrap();
    let config = dir.join("sshd_config");
    fs::write(&config, format!(
        "Port {port}\nListenAddress 127.0.0.1\nHostKey {}/host-key\nAuthorizedKeysFile {}/client-key.pub\nPidFile {}/sshd.pid\nPasswordAuthentication no\nKbdInteractiveAuthentication no\nUsePAM no\nPermitRootLogin yes\nStrictModes no\nLogLevel ERROR\nSetEnv PATH={}:/usr/bin:/bin BRNR_DIR={} BRNR_HOME={} BRNR_CONFIG={}\n",
        dir.display(), dir.display(), dir.display(), dir.join("bin").display(),
        s.remote.dir.join("run").display(), s.remote.dir.join("home").display(),
        s.remote.config().display()
    )).unwrap();
    let log = dir.join("sshd.log");
    let mut daemon = Daemon(
        Command::new("/usr/sbin/sshd")
            .args(["-D", "-e", "-f"])
            .arg(&config)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(fs::File::create(&log).unwrap())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(
            daemon.0.try_wait().unwrap().is_none() && Instant::now() < deadline,
            "sshd failed: {}",
            fs::read_to_string(&log).unwrap()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let host_key = fs::read_to_string(dir.join("host-key.pub")).unwrap();
    let key = host_key.split_whitespace().take(2).collect::<Vec<_>>().join(" ");
    fs::write(dir.join("known_hosts"), format!("[127.0.0.1]:{port} {key}\n")).unwrap();
    let client = dir.join("ssh_config");
    fs::write(&client, format!(
        "Host linux\n HostName 127.0.0.1\n Port {port}\n User {}\n IdentityFile {}/client-key\n IdentitiesOnly yes\n UserKnownHostsFile {}/known_hosts\n GlobalKnownHostsFile /dev/null\n",
        user.trim(), dir.display(), dir.display()
    )).unwrap();
    let quoted = format!("'{}'", client.to_str().unwrap().replace('\'', "'\\''"));
    fs::write(dir.join("bin/ssh"), format!("#!/bin/sh\nexec /usr/bin/ssh -F {quoted} \"$@\"\n"))
        .unwrap();
    // The destination's environment is its shell's, as ~/.zshenv or
    // ~/.bashrc would set it: here sshd's SetEnv (ADR 12.2).
    let new = s.run(&["session", "new", "--remote", "linux", "--profile", "coding", "--json"]);
    let id = new["session"].as_str().unwrap();
    assert_eq!(
        s.run(&["session", "list", "--remote", "linux", "--json"])["sessions"][0]["session"],
        id
    );
    let mut child = s
        .cmd(&["prompt", "send", id, "--remote", "linux", "--prompt", "-", "--wait", "--json"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"reply authenticated-ssh").unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap()["reply"],
        "authenticated-ssh"
    );
    acp_command(s.cmd(&["acp", "--remote", "linux", "--profile", "coding"]));
}
