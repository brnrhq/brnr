//! Seeded failures and SIGKILL at live protocol boundaries (ADRs 3.1, 3.4, 3.7, 3.9
//! and 5.7). Each signal targets a pid obtained from this fixture, never a
//! process-name match.
//!
//! A seeded test runs the fixed seeds and one new seed. One that fails says
//! its seed; `BRNR_CHAOS_SEED=<n>` runs that seed alone, to replay it. The
//! start test has no seed: it faults at each setup step in turn.

mod common;

use common::*;
use serde_json::{Value, json};
use std::fs;
use std::io::Write;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const WAIT: Duration = Duration::from_secs(15);
/// 7 and 23 fault at initialize in setup, 41 at session/new; in a turn
/// they fault after 20, 18 and 24 chunks.
const SEEDS: [u64; 3] = [7, 23, 41];

/// The seeds a test runs: `BRNR_CHAOS_SEED` alone, or [`SEEDS`] and a new
/// one. A new seed is below 1000, to keep the socket's path short.
fn seeds() -> Vec<u64> {
    if let Ok(seed) = std::env::var("BRNR_CHAOS_SEED") {
        return vec![seed.parse().expect("BRNR_CHAOS_SEED is a number")];
    }
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    let fresh = (now.subsec_nanos() ^ std::process::id()) % 1000;
    SEEDS.into_iter().chain([u64::from(fresh)]).collect()
}

/// Held while a seed runs: if the test fails meanwhile, says how to replay
/// that seed.
struct Replay(u64);

impl Drop for Replay {
    fn drop(&mut self) {
        if std::thread::panicking() {
            let test = std::thread::current().name().unwrap_or("<test>").to_owned();
            eprintln!(
                "chaos: failed with seed {0}; replay: BRNR_CHAOS_SEED={0} cargo test --release --test chaos {test} -- --exact",
                self.0
            );
        }
    }
}

fn fixture(name: &str, fault: &str, seed: u64) -> Env {
    let env = Env::new(name).agent("FAULT", fault).agent("FAULT_SEED", &seed.to_string());
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

/// What `process list --json` lists.
fn listed(env: &Env) -> Value {
    let text = env.ok(&["process", "list", "--json"]);
    serde_json::from_str::<Value>(&text).unwrap()["processes"].clone()
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
    assert_eq!(listed(env), json!([]));
    // Taking the same session again proves the kernel released its flock,
    // independently of doctor removing the stale lock file (ADR 3.9).
    env.resume("sess-1", &[]);
    assert!(env.ok(&["session", "list", "--include", "active"]).contains("sess-1"));
    env.stop();
}

/// `brnr acp -- fake_agent.py`, as an editor runs it, and its stdin.
fn editor(env: &Env, stderr: Stdio) -> (Child, ChildStdin) {
    let mut proxy = env
        .brnr(&["acp", "--", AGENT])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(stderr)
        .spawn()
        .unwrap();
    let input = proxy.stdin.take().unwrap();
    (proxy, input)
}

/// The editor's initialize and session/new.
fn setup(env: &Env, input: &mut ChildStdin) {
    for message in [
        json!({"id": 1, "method": "initialize", "params": {"protocolVersion": 1, "clientCapabilities": {}}}),
        json!({"id": 2, "method": "session/new", "params": {"cwd": env.dir, "mcpServers": []}}),
    ] {
        writeln!(input, "{message}").unwrap();
    }
}

/// The editor's setup, once the process owns sess-1.
fn opened(env: &Env, input: &mut ChildStdin) {
    setup(env, input);
    assert!(wait_for(WAIT, || env.calls_of("session/new").len() == 1));
    // Wait for the host to own the session, rather than assuming the
    // agent's receipt of session/new means its reply was processed.
    assert!(wait_for(WAIT, || env
        .ok(&["session", "list", "--include", "active"])
        .contains("sess-1")));
}

/// The editor's prompt in sess-1, which faults.
fn prompt(input: &mut ChildStdin) {
    let prompt = json!({"id": 3, "method": "session/prompt", "params": {
        "sessionId": "sess-1", "prompt": [{"type": "text", "text": "fault"}]
    }});
    writeln!(input, "{prompt}").unwrap();
}

/// The one process's host, once its metadata is written.
fn host(env: &Env) -> Process {
    assert!(wait_for(WAIT, || env.hosts().len() == 1), "{:?}", env.hosts());
    Process::new(env.host_pid())
}

#[test]
fn adr_0003_seeded_setup_failure_never_commits_a_prompt() {
    for seed in seeds() {
        let _replay = Replay(seed);
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
            assert!(logs(&env).contains("\"event\":\"process_exited\""));
            assert!(env.ok(&["doctor", "--fix"]).contains("no process died without recording it"));
        }
    }
}

#[test]
fn adr_0003_seeded_agent_crash_records_death_and_releases_session() {
    for seed in seeds() {
        let _replay = Replay(seed);
        let env = fixture(&format!("chaos-crash-{seed}"), "crash", seed).agent("STUBBORN", "child");
        env.start(&[]);
        let (host, agent) = processes(&env);
        let descendant = Process::new(env.child_pid());
        let _ = env.run(&["prompt", "send", "sess-1", "--prompt", "fault"]);
        let fault = marker(&env);
        assert_eq!(fault["seed"], seed);
        host.gone();
        agent.gone();
        descendant.gone();
        assert!(logs(&env).contains("\"event\":\"process_exited\""));
        recovered(&env, false);
    }
}

#[test]
fn adr_0003_killed_host_mid_flood_is_diagnosed_and_releases_session() {
    for seed in seeds() {
        let _replay = Replay(seed);
        let env = fixture(&format!("chaos-host-{seed}"), "flood", seed);
        env.start(&[]);
        let (host, agent) = processes(&env);
        env.ok(&["prompt", "send", "sess-1", "--prompt", "fault"]);
        marker(&env);
        env.ok(&["event", "log", "sess-1"]); // Flush the session identity before SIGKILL.
        host.signal(libc::SIGKILL);
        host.gone();
        agent.gone(); // The severed output pipe, without test-assisted cleanup.
        assert!(!logs(&env).contains("\"event\":\"process_exited\""));
        recovered(&env, true);
    }
}

#[test]
fn adr_0003_killed_proxy_mid_fault_stops_host_and_agent() {
    for seed in seeds() {
        let _replay = Replay(seed);
        for kind in ["hang", "flood"] {
            let env = fixture(&format!("chaos-proxy-{kind}-{seed}"), kind, seed)
                .agent("STUBBORN", "child");
            let (mut proxy, mut input) = editor(&env, Stdio::null());
            opened(&env, &mut input);
            let (host, agent) = processes(&env);
            let descendant = Process::new(env.child_pid());
            prompt(&mut input);
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
fn adr_0003_killed_host_mid_start_commits_nothing() {
    // Each setup step, chosen rather than seeded. A flooding agent exits on
    // its broken pipe; a hung one outlives the host (#391,
    // killed_host_leaves_no_agent_or_child_behind).
    for (n, step) in ["initialize", "session/new"].into_iter().enumerate() {
        let env = fixture(&format!("chaos-start-{n}"), "flood", 0)
            .agent("FAULT_PHASE", "setup")
            .agent("FAULT_AT", step);
        let mut start = env
            .brnr(&new_args(&["--prompt", "must not run"]))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let fault = marker(&env);
        assert_eq!(fault["checkpoint"], step);
        let agent = Process::new(fault["pid"].as_i64().unwrap() as i32);
        let host = host(&env);
        host.signal(libc::SIGKILL);
        host.gone();
        agent.gone();
        // The command sees its start channel close before the commit.
        assert!(wait_exit(&mut start, WAIT), "session new outlived its process");
        let out = start.wait_with_output().unwrap();
        assert!(!out.status.success());
        assert!(stderr(&out).contains("the process exited without a session"), "{}", stderr(&out));
        assert!(env.prompts().is_empty());
        assert!(!logs(&env).contains("\"event\":\"process_exited\""));
        assert!(env.ok(&["doctor", "--fix"]).contains("1 process died without recording it"));
        assert!(env.hosts().is_empty(), "runtime metadata survived: {:?}", env.hosts());
        assert_eq!(listed(&env), json!([]));
    }
}

#[test]
fn adr_0003_killed_proxy_mid_setup_stops_host_and_agent() {
    for seed in seeds() {
        let _replay = Replay(seed);
        for kind in ["hang", "flood"] {
            let env = fixture(&format!("chaos-psetup-{kind}-{seed}"), kind, seed)
                .agent("FAULT_PHASE", "setup")
                .agent("STUBBORN", "child");
            let (mut proxy, mut input) = editor(&env, Stdio::null());
            setup(&env, &mut input);
            let agent = Process::new(marker(&env)["pid"].as_i64().unwrap() as i32);
            let host = host(&env);
            let descendant = Process::new(env.child_pid());
            proxy.kill().unwrap();
            assert!(wait_exit(&mut proxy, WAIT));
            host.gone();
            agent.gone();
            descendant.gone();
            assert!(logs(&env).contains("\"event\":\"process_exited\""));
            assert!(env.ok(&["doctor", "--fix"]).contains("no process died without recording it"));
            assert!(env.hosts().is_empty(), "runtime metadata survived: {:?}", env.hosts());
        }
    }
}

#[test]
fn adr_0003_killed_host_mid_turn_ends_the_editors_proxy() {
    for seed in seeds() {
        let _replay = Replay(seed);
        let env = fixture(&format!("chaos-phost-{seed}"), "flood", seed);
        let (mut proxy, mut input) = editor(&env, Stdio::piped());
        opened(&env, &mut input);
        let (host, agent) = processes(&env);
        prompt(&mut input);
        marker(&env);
        env.ok(&["event", "log", "sess-1"]); // Flush the session identity before SIGKILL.
        host.signal(libc::SIGKILL);
        host.gone();
        agent.gone();
        // The editor sees its agent go, as if the agent itself had died.
        assert!(wait_exit(&mut proxy, WAIT), "brnr acp outlived its process");
        drop(input);
        let out = proxy.wait_with_output().unwrap();
        assert_eq!(out.status.code(), Some(1));
        assert!(stderr(&out).contains("lost the connection to its process"), "{}", stderr(&out));
        assert!(!logs(&env).contains("\"event\":\"process_exited\""));
        recovered(&env, true);
    }
}

/// The gap the threat model names: the host is the only supervisor, and
/// SIGKILL stops it signalling its children. A hung agent and the child it
/// started outlive it.
#[test]
#[ignore = "#391: a SIGKILLed host leaves a hung agent and its children running"]
fn killed_host_leaves_no_agent_or_child_behind() {
    for seed in seeds() {
        let _replay = Replay(seed);
        let env = fixture(&format!("chaos-orphan-{seed}"), "hang", seed).agent("STUBBORN", "child");
        env.start(&[]);
        let (host, agent) = processes(&env);
        let descendant = Process::new(env.child_pid());
        env.ok(&["prompt", "send", "sess-1", "--prompt", "fault"]);
        marker(&env);
        host.signal(libc::SIGKILL);
        host.gone();
        agent.gone();
        descendant.gone();
        recovered(&env, true);
    }
}

#[test]
fn adr_0005_killed_bridge_mid_turn_leaves_owner_and_lock_intact() {
    for seed in seeds() {
        let _replay = Replay(seed);
        let env = fixture(&format!("chaos-bridge-{seed}"), "hang", seed);
        let path = env.dir.join("bridge.pid");
        let script = format!("echo $$ > '{}'; exec cat > /dev/null", path.display());
        env.write_config(&format!(
            "[[profiles.default.bridges]]\ncommand = \"sh\"\nargs = [\"-c\", {script:?}]\n"
        ));
        env.start(&[]);
        let (host, agent) = processes(&env);
        assert!(wait_for(WAIT, || fs::read_to_string(&path)
            .is_ok_and(|s| s.trim().parse::<i32>().is_ok())));
        let bridge = Process::new(fs::read_to_string(path).unwrap().trim().parse().unwrap());
        env.ok(&["prompt", "send", "sess-1", "--prompt", "fault"]);
        marker(&env);
        bridge.signal(libc::SIGKILL);
        bridge.gone();
        assert!(wait_for(WAIT, || logs(&env).contains("bridge_exited")));
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
