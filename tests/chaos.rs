//! Seeded failures and SIGKILL at live protocol boundaries (ADRs 3, 7, 11, 35).
//! Each signal targets a pid obtained from this fixture, never a process-name match.

mod common;

use common::*;
use serde_json::{Value, json};
use std::fs;
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(15);
const SEEDS: [&str; 3] = ["7", "23", "41"];

fn fixture(name: &str, fault: &str, seed: &str) -> Env {
    let env = Env::new(name).agent("FAULT", fault).agent("FAULT_SEED", seed);
    let marker = env.dir.join("fault");
    env.agent("FAULT_LOG", marker.to_str().unwrap())
}

fn marker(env: &Env) -> Value {
    let read = || {
        fs::read_to_string(env.dir.join("fault"))
            .ok()
            .and_then(|s| serde_json::from_str::<Value>(s.trim()).ok())
    };
    assert!(wait_for(WAIT, || read().is_some()), "fault never reached: {:?}", env.calls());
    read().unwrap()
}

/// ps distinguishes a dead, unreaped zombie from an executing orphan. Keep a
/// start-time/command identity too, so cleanup cannot signal a reused pid.
struct Process {
    pid: i32,
    identity: String,
}

fn ps(pid: i32, field: &str) -> String {
    let out = Command::new("ps").args(["-p", &pid.to_string(), "-o", field]).output().unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

impl Process {
    fn new(pid: i32) -> Self {
        Self { pid, identity: ps(pid, "lstart=,command=") }
    }
    fn running(&self) -> bool {
        !self.identity.is_empty()
            && ps(self.pid, "lstart=,command=") == self.identity
            && !ps(self.pid, "stat=").starts_with('Z')
    }
    fn signal(&self, sig: i32) {
        assert!(self.running(), "fixture pid {} is no longer running", self.pid);
        assert!(kill(self.pid, sig));
    }
    fn gone(&self) {
        assert!(wait_for(WAIT, || !self.running()), "orphan pid {}: {}", self.pid, self.identity);
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        if self.running() {
            kill(self.pid, libc::SIGKILL);
        }
    }
}

fn processes(env: &Env) -> (Process, Process) {
    let hosts = env.hosts();
    assert_eq!(hosts.len(), 1, "{hosts:?}");
    let pid = |key: &str| hosts[0][key].as_i64().unwrap() as i32;
    (Process::new(pid("host_pid")), Process::new(pid("agent_pid")))
}

fn logs(env: &Env) -> String {
    fs::read_dir(env.dir.join("home/hosts"))
        .unwrap()
        .flatten()
        .map(|e| fs::read_to_string(e.path()).unwrap())
        .collect()
}

fn recovered(env: &Env, unrecorded: bool) {
    let report = env.ok(&["doctor", "--fix"]);
    let want = if unrecorded {
        "1 process died without recording it"
    } else {
        "no process died without recording it"
    };
    assert!(report.contains(want), "{report}");
    assert!(env.hosts().is_empty(), "runtime metadata survived: {:?}", env.hosts());
    let listed: Value = serde_json::from_str(&env.ok(&["process", "list", "--json"])).unwrap();
    assert_eq!(listed, json!([]));
    // Taking the same session again proves the kernel released its flock,
    // independently of doctor removing the stale lock file (ADR 3).
    env.resume("sess-1", &[]);
    assert!(env.ok(&["list"]).contains("sess-1"));
    env.stop();
}

#[test]
fn adr_0007_seeded_setup_failure_never_commits_a_prompt() {
    for seed in SEEDS {
        for kind in ["crash", "hang", "flood"] {
            let env = fixture(&format!("chaos-setup-{kind}-{seed}"), kind, seed)
                .agent("FAULT_PHASE", "setup")
                .agent("STUBBORN", "child");
            let mut start = env
                .brnr(&new_args(&["--prompt", "must not run"]))
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            let fault = marker(&env);
            let descendant = Process::new(env.child_pid());
            let agent = Process::new(fault["pid"].as_i64().unwrap() as i32);
            let hosts: Vec<_> = env
                .hosts()
                .iter()
                .map(|h| Process::new(h["host_pid"].as_i64().unwrap() as i32))
                .collect();
            // In every case the start has not committed. Killing the caller
            // must abandon even a hung or continuously writing setup agent.
            let _ = start.kill();
            assert!(wait_exit(&mut start, WAIT));
            assert!(wait_for(WAIT, || env.hosts().is_empty()));
            for host in hosts {
                host.gone();
            }
            agent.gone();
            descendant.gone();
            assert!(env.prompts().is_empty());
            assert!(logs(&env).contains("\"event\":\"exited\""));
            assert!(env.ok(&["doctor", "--fix"]).contains("no process died without recording it"));
        }
    }
}

#[test]
fn adr_0011_seeded_agent_crash_records_death_and_releases_session() {
    for seed in SEEDS {
        let env = fixture(&format!("chaos-crash-{seed}"), "crash", seed).agent("STUBBORN", "child");
        env.start(&[]);
        let (host, agent) = processes(&env);
        let descendant = Process::new(env.child_pid());
        let _ = env.run(&["prompt", "send", "sess-1", "fault"]);
        let fault = marker(&env);
        assert_eq!(fault["seed"], seed.parse::<u64>().unwrap());
        host.gone();
        agent.gone();
        descendant.gone();
        assert!(logs(&env).contains("\"event\":\"exited\""));
        recovered(&env, false);
    }
}

#[test]
fn adr_0003_killed_host_mid_flood_is_diagnosed_and_releases_session() {
    for seed in SEEDS {
        let env = fixture(&format!("chaos-host-{seed}"), "flood", seed);
        env.start(&[]);
        let (host, agent) = processes(&env);
        env.ok(&["prompt", "send", "sess-1", "fault"]);
        marker(&env);
        env.ok(&["event", "log", "sess-1"]); // Flush the session identity before SIGKILL.
        host.signal(libc::SIGKILL);
        host.gone();
        agent.gone(); // The severed output pipe, without test-assisted cleanup.
        assert!(!logs(&env).contains("\"event\":\"exited\""));
        recovered(&env, true);
    }
}

#[test]
fn adr_0011_killed_proxy_mid_fault_stops_host_and_agent() {
    for seed in SEEDS {
        for kind in ["hang", "flood"] {
            let env = fixture(&format!("chaos-proxy-{kind}-{seed}"), kind, seed)
                .agent("STUBBORN", "child");
            let mut proxy = env
                .brnr(&["acp", "--", AGENT])
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            let mut input = proxy.stdin.take().unwrap();
            for message in [
                json!({"id": 1, "method": "initialize", "params": {"protocolVersion": 1, "clientCapabilities": {}}}),
                json!({"id": 2, "method": "session/new", "params": {"cwd": env.dir, "mcpServers": []}}),
            ] {
                writeln!(input, "{message}").unwrap();
            }
            assert!(wait_for(WAIT, || env.calls_of("session/new").len() == 1));
            // Wait for the host to own the session, rather than assuming the
            // agent's receipt of session/new means its reply was processed.
            assert!(wait_for(WAIT, || env.ok(&["list"]).contains("sess-1")));
            let (host, agent) = processes(&env);
            let descendant = Process::new(env.child_pid());
            writeln!(
                input,
                "{}",
                json!({"id": 3, "method": "session/prompt", "params": {
                    "sessionId": "sess-1", "prompt": [{"type": "text", "text": "fault"}]
                }})
            )
            .unwrap();
            marker(&env);
            proxy.kill().unwrap();
            assert!(wait_exit(&mut proxy, WAIT));
            host.gone();
            agent.gone();
            descendant.gone();
            recovered(&env, false);
        }
    }
}

#[test]
fn adr_0035_killed_bridge_mid_turn_leaves_owner_and_lock_intact() {
    for seed in SEEDS {
        let env = fixture(&format!("chaos-bridge-{seed}"), "hang", seed);
        let path = env.dir.join("bridge.pid");
        let script = format!("echo $$ > '{}'; exec cat > /dev/null", path.display());
        env.write_config(&format!(
            "[[profiles.default.bridges]]\ncommand = [\"sh\", \"-c\", {script:?}]\n"
        ));
        env.start(&[]);
        let (host, agent) = processes(&env);
        assert!(wait_for(WAIT, || fs::read_to_string(&path)
            .is_ok_and(|s| s.trim().parse::<i32>().is_ok())));
        let bridge = Process::new(fs::read_to_string(path).unwrap().trim().parse().unwrap());
        env.ok(&["prompt", "send", "sess-1", "fault"]);
        marker(&env);
        bridge.signal(libc::SIGKILL);
        bridge.gone();
        assert!(wait_for(WAIT, || logs(&env).contains("bridge-exited")));
        assert!(host.running() && agent.running());
        assert!(env.ok(&["doctor", "--fix"]).contains("no process died without recording it"));
        assert_eq!(env.host_pid(), host.pid);
        assert!(env.fails(&resume_args("sess-1", &[])).contains("running in process"));
        env.stop();
        host.gone();
        agent.gone();
        recovered(&env, false);
    }
}
