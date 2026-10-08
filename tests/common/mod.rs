//! Shared by the integration tests: a scratch environment for brnr with its
//! own runtime, state and config directories, and the fake ACP agent
//! (fake_agent.py). Dropping it kills whatever it left running.

#![allow(dead_code)]

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant, SystemTime};

use serde_json::{Value, json};

pub const AGENT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake_agent.py");

/// How the link a panic prints starts (ADR 45).
pub const ISSUE_LINK: &str = "https://github.com/brnrhq/brnr/issues/new?template=bug.yml&title=";

pub struct Env {
    pub dir: PathBuf,
    agent_env: Vec<(String, String)>,
}

impl Env {
    /// Short paths: the control socket must fit in `sun_path`.
    pub fn new(name: &str) -> Env {
        let dir = std::env::temp_dir().join(format!("brnr-t{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Env { dir, agent_env: Vec::new() }
    }

    pub fn agent(mut self, key: &str, value: &str) -> Env {
        self.agent_env.push((key.into(), value.into()));
        self
    }

    pub fn brnr(&self, args: &[&str]) -> Command {
        self.brnr_at(Path::new(env!("CARGO_BIN_EXE_brnr")), args)
    }

    /// brnr started by another path to it, such as a symlink.
    pub fn brnr_at(&self, path: &Path, args: &[&str]) -> Command {
        let mut cmd = Command::new(path);
        cmd.args(args)
            .current_dir(&self.dir)
            .env("BRNR_DIR", self.dir.join("run"))
            .env("BRNR_HOME", self.dir.join("home"))
            .env("BRNR_CONFIG", self.dir.join("none.toml"))
            .env("PROMPT_LOG", self.dir.join("prompts"))
            .env("CALL_LOG", self.dir.join("calls"))
            .env("CHILD_PID", self.dir.join("child.pid"))
            .envs(self.agent_env.iter().map(|(k, v)| (k, v)));
        cmd
    }

    pub fn run(&self, args: &[&str]) -> Output {
        self.brnr(args).output().unwrap()
    }

    pub fn run_with_stdin(&self, args: &[&str], input: &[u8]) -> Output {
        let mut child = self
            .brnr(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        let input = input.to_vec();
        let writer = std::thread::spawn(move || stdin.write_all(&input));
        let out = child.wait_with_output().unwrap();
        writer.join().unwrap().unwrap();
        out
    }

    /// `brnr start [args] -- fake_agent.py`, which must succeed.
    pub fn start(&self, args: &[&str]) -> Output {
        let out = self.run(&start_args(args));
        assert!(out.status.success(), "start failed: {}", stderr(&out));
        out
    }

    pub fn hosts(&self) -> Vec<Value> {
        let Ok(entries) = fs::read_dir(self.dir.join("run")) else { return Vec::new() };
        entries
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .filter_map(|e| serde_json::from_slice(&fs::read(e.path()).ok()?).ok())
            .collect()
    }

    pub fn host_pid(&self) -> i32 {
        let hosts = self.hosts();
        assert_eq!(hosts.len(), 1, "expected one host: {hosts:?}");
        hosts[0]["host_pid"].as_i64().unwrap() as i32
    }

    /// The one running process's pid, as `brnr ps` and `--pid` have it.
    pub fn pid(&self) -> String {
        self.host_pid().to_string()
    }

    /// `brnr stop` for the one running process, which must succeed.
    pub fn stop(&self) {
        self.ok(&["stop", &self.pid()]);
    }

    pub fn prompts(&self) -> Vec<String> {
        let text = fs::read_to_string(self.dir.join("prompts")).unwrap_or_default();
        text.lines().map(|l| serde_json::from_str(l).unwrap()).collect()
    }

    /// Every message the agent received, in order.
    pub fn calls(&self) -> Vec<Value> {
        let text = fs::read_to_string(self.dir.join("calls")).unwrap_or_default();
        text.lines().map(|l| serde_json::from_str(l).unwrap()).collect()
    }

    /// The agent's requests with `method`.
    pub fn calls_of(&self, method: &str) -> Vec<Value> {
        self.calls().into_iter().filter(|c| c["method"] == method).collect()
    }

    /// `brnr <args>`, which must succeed; its stdout.
    pub fn ok(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert!(out.status.success(), "brnr {args:?} failed: {}", stderr(&out));
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// `brnr <args>`, which must fail; its stderr.
    pub fn fails(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert!(
            !out.status.success(),
            "brnr {args:?} succeeded: {}",
            String::from_utf8_lossy(&out.stdout)
        );
        stderr(&out)
    }

    pub fn write_config(&self, text: &str) {
        fs::write(self.dir.join("none.toml"), text).unwrap();
    }

    pub fn child_pid(&self) -> i32 {
        let path = self.dir.join("child.pid");
        assert!(wait_for(Duration::from_secs(5), || path.exists()), "no child pid");
        fs::read_to_string(path).unwrap().trim().parse().unwrap()
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        for host in self.hosts() {
            for key in ["agent_pid", "host_pid"] {
                if let Some(pid) = host[key].as_i64() {
                    kill(-(pid as i32), libc::SIGKILL);
                    kill(pid as i32, libc::SIGKILL);
                }
            }
        }
        if let Ok(pid) = fs::read_to_string(self.dir.join("child.pid"))
            && let Ok(pid) = pid.trim().parse::<i32>()
        {
            kill(pid, libc::SIGKILL);
        }
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// Metadata and a socket left by a brnr process that is gone, whose pid is
/// another process's now, as after a reboot or pids wrapping around: a
/// `sleep`'s. Nobody listens on the socket, and the metadata was written
/// before the sleep started. The sleep is the caller's to kill.
pub fn ghost(env: &Env) -> Child {
    let run = env.dir.join("run");
    fs::create_dir_all(&run).unwrap();
    fs::set_permissions(&run, fs::Permissions::from_mode(0o700)).unwrap();
    let sleep = Command::new("sleep").arg("60").spawn().unwrap();
    let pid = sleep.id();
    let socket = run.join(format!("{pid}.sock"));
    drop(UnixListener::bind(&socket).unwrap());
    let meta = run.join(format!("{pid}.json"));
    let info = json!({
        "id": pid.to_string(),
        "host_id": format!("20260101T000000-{pid}"),
        "host_pid": pid,
        "agent_pid": null,
        "agent": [AGENT],
        "cwd": env.dir,
        "socket": socket,
        "started": "2026-01-01T00:00:00.000000Z",
    });
    fs::write(&meta, info.to_string()).unwrap();
    let before = SystemTime::now() - Duration::from_secs(3600);
    fs::File::options().write(true).open(&meta).unwrap().set_modified(before).unwrap();
    sleep
}

pub fn start_args<'a>(args: &[&'a str]) -> Vec<&'a str> {
    let mut all = vec!["start"];
    all.extend_from_slice(args);
    all.extend_from_slice(&["--", AGENT]);
    all
}

pub fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// Sends `sig` to `pid` (negative: its process group); whether it was.
pub fn kill(pid: i32, sig: i32) -> bool {
    // SAFETY: kill(2) takes no pointers; any pid and signal are valid
    // arguments.
    unsafe { libc::kill(pid, sig) == 0 }
}

pub fn alive(pid: i32) -> bool {
    kill(pid, 0)
}

pub fn wait_for(timeout: Duration, mut done: impl FnMut() -> bool) -> bool {
    let until = Instant::now() + timeout;
    while Instant::now() < until {
        if done() {
            return true;
        }
        sleep(Duration::from_millis(50));
    }
    done()
}

pub fn wait_exit(child: &mut Child, timeout: Duration) -> bool {
    wait_for(timeout, || child.try_wait().unwrap().is_some())
}
