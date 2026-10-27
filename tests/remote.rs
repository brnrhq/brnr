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
os.execv('/bin/sh', ['sh', '-c', sys.argv[-1]])
"#,
        )
        .unwrap();
        fs::set_permissions(&ssh, fs::Permissions::from_mode(0o755)).unwrap();
        local.write_config(&format!(
            "[remotes.linux]\nhost = 'agent@linux'\nexecutable = {:?}\n",
            env!("CARGO_BIN_EXE_brnr")
        ));
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
    assert_eq!(s.arguments()[0][5], "agent@linux");
}

#[test]
fn adr_0012_profiles_route_once_and_explicit_remote_wins() {
    let s = Setup::new("profiles");
    let original = fs::read_to_string(s.local.config()).unwrap();
    s.local.write_config(&format!(
        "{original}\n[profiles.work.remote]\ntarget = 'linux'\nprofile = 'coding'\n"
    ));
    let new = s.run(&["session", "new", "--profile", "work", "--json"]);
    let id = new["session"].as_str().unwrap();
    let status = s.run(&["session", "status", id, "--profile", "work", "--json"]);
    assert_eq!(status["session"], id);
    s.run(&["session", "list", "--remote", "linux", "--profile", "coding", "--json"]);
    assert!(!s.arguments().iter().any(|a| a[6].as_str().unwrap().contains("--profile work")));
    s.remote.write_config("[profiles.coding.remote]\ntarget = 'another-host'\n");
    let out = s.cmd(&["acp", "--profile", "work"]).output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("nested remote routing"));
    assert_eq!(s.arguments().len(), 4);
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
    let wrong = s.local.dir.join("wrong-version");
    fs::write(&wrong, "#!/bin/sh\nprintf 'brnr 99.0.0\\n'\n").unwrap();
    fs::set_permissions(&wrong, fs::Permissions::from_mode(0o755)).unwrap();
    s.local.write_config(&format!(
        "[remotes.linux]\nhost='linux'\nexecutable={:?}\n",
        wrong.to_str().unwrap()
    ));
    let out = s.cmd(&["session", "list", "--remote", "linux"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("version mismatch"));
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

#[test]
fn adr_0012_remote_inspection_never_writes_configuration() {
    let s = Setup::new("inspection");
    let original = fs::read(s.local.config()).unwrap();
    fs::set_permissions(s.local.config(), fs::Permissions::from_mode(0o400)).unwrap();
    fs::set_permissions(s.local.config().parent().unwrap(), fs::Permissions::from_mode(0o500))
        .unwrap();
    let listed = s.run(&["remote", "list", "--json"]);
    assert_eq!(listed["remotes"][0]["name"], "linux");
    assert!(s.arguments().is_empty(), "listing must not connect");
    let checked = s.cmd(&["remote", "check", "linux", "--json"]).output().unwrap();
    let result: Value = serde_json::from_slice(&checked.stdout).unwrap();
    assert!(result["checks"].is_array(), "{result}");
    assert!(s.arguments().last().unwrap()[6].as_str().unwrap().contains("_remote doctor --json"));
    for verb in ["init", "remove"] {
        let out = s.cmd(&["remote", verb, "linux", "--json"]).output().unwrap();
        assert_eq!(out.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&out.stderr).contains("unknown command"));
    }
    assert_eq!(s.arguments().len(), 1, "removed commands must not connect");
    assert_eq!(fs::read(s.local.config()).unwrap(), original);
    assert_eq!(fs::metadata(s.local.config()).unwrap().permissions().mode() & 0o777, 0o400);
    fs::set_permissions(s.local.config().parent().unwrap(), fs::Permissions::from_mode(0o700))
        .unwrap();
    fs::set_permissions(s.local.config(), fs::Permissions::from_mode(0o600)).unwrap();
    fs::remove_file(s.local.config()).unwrap();
    fs::remove_dir(s.local.config().parent().unwrap()).unwrap();
    assert_eq!(s.run(&["remote", "list", "--json"])["remotes"], json!([]));
    let checked = s.cmd(&["remote", "check", "linux", "--json"]).output().unwrap();
    assert!(serde_json::from_slice::<Value>(&checked.stdout).unwrap()["checks"].is_array());
    assert!(
        !s.local.config().parent().unwrap().exists(),
        "inspection must not create local config"
    );
}

#[test]
fn adr_0012_remote_environment_paths_and_default_launch_profiles_are_resolved_once() {
    let s = Setup::new("environment");
    s.local.write_config(&format!(
        "[remotes.linux]\nhost='agent@linux'\nexecutable={:?}\nruntime_dir={:?}\nhome={:?}\nconfig={:?}\ncwd={:?}\n[profiles.default.remote]\ntarget='linux'\nprofile='coding'\n",
        env!("CARGO_BIN_EXE_brnr"), s.remote.dir.join("run"), s.remote.dir.join("home"), s.remote.config(), s.remote.dir
    ));
    let new = s.run(&["session", "new", "--json"]);
    let id = new["session"].as_str().unwrap();
    let status = s.run(&["session", "status", id, "--remote", "linux", "--json"]);
    assert_eq!(status["session"], id);
    let command = s.arguments()[0][6].as_str().unwrap().to_owned();
    assert!(command.contains("BRNR_DIR="));
    assert!(command.contains("BRNR_HOME="));
    assert!(command.contains("BRNR_CONFIG="));
    assert!(command.starts_with("cd "));
    let local = s.local.run(&["session", "list", "--include", "active", "--json"]);
    assert_eq!(serde_json::from_slice::<Value>(&local.stdout).unwrap()["sessions"], json!([]));
    assert_eq!(s.arguments().len(), 2);
}

#[test]
fn adr_0012_acp_has_clean_streams_and_remote_profile_resolution() {
    let s = Setup::new("acp");
    acp(&s);
    assert_eq!(s.arguments().len(), 1);
}

fn acp(s: &Setup) {
    let mut child = s
        .cmd(&["acp", "--remote=linux", "--profile=coding"])
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
        "Port {port}\nListenAddress 127.0.0.1\nHostKey {}/host-key\nAuthorizedKeysFile {}/client-key.pub\nPidFile {}/sshd.pid\nPasswordAuthentication no\nKbdInteractiveAuthentication no\nUsePAM no\nPermitRootLogin yes\nStrictModes no\nLogLevel ERROR\n",
        dir.display(), dir.display(), dir.display()
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
    s.local.write_config(&format!(
        "[remotes.linux]\nhost='linux'\nexecutable={:?}\nruntime_dir={:?}\nhome={:?}\nconfig={:?}\n",
        env!("CARGO_BIN_EXE_brnr"), s.remote.dir.join("run"), s.remote.dir.join("home"), s.remote.config()
    ));
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
    acp(&s);
}
