//! tests/flake-loop.sh leaves nothing it started running, however it ends
//! (#123). Each process is found by a pid it is told, never by its name.

mod common;

use common::*;
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::PathBuf;
use std::process::{Child, ChildStdout, Command, Stdio};
use std::thread::sleep;
use std::time::Duration;

const SCRIPT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/flake-loop.sh");
/// How long a stressor may take to stop: it says within a second.
const GONE: Duration = Duration::from_secs(5);

/// The script with one stressor beside one run of `sleep secs`, and the
/// stressor's pid; the stressor is killed on drop if it is still there, and
/// the directory of the runs' output removed. Its stderr is piped.
struct Loop {
    child: Child,
    out: BufReader<ChildStdout>,
    stressor: i32,
    dir: PathBuf,
}

impl Loop {
    /// With a load limit the host's load can't reach, so that the tests don't
    /// depend on it.
    fn start(name: &str, args: &[&str], secs: &str) -> Loop {
        Loop::start_with(name, args, secs, None)
    }

    /// With the load read from a file in the directory, `load` in it, as
    /// /proc/loadavg would have it.
    fn start_with(name: &str, args: &[&str], secs: &str, load: Option<&str>) -> Loop {
        let dir = std::env::temp_dir().join(format!("brnr-t{}-{name}", std::process::id()));
        let mut cmd = Command::new(SCRIPT);
        if let Some(load) = load {
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("loadavg"), load).unwrap();
            cmd.env("FLAKE_LOOP_LOADAVG", dir.join("loadavg"));
        }
        let mut child = cmd
            .args(["-l", "1000000", "-r", "1", "-p", "1", "-s", "1", "-o", dir.to_str().unwrap()])
            .args(args)
            .args(["/bin/sleep", secs])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut out = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        out.read_line(&mut line).unwrap();
        let stressor = line.strip_prefix("stressors: ").and_then(|p| p.trim().parse().ok());
        let stressor = stressor.unwrap_or_else(|| panic!("no stressor pid in {line:?}"));
        assert!(alive(stressor), "the stressor runs");
        Loop { child, out, stressor, dir }
    }

    fn stressor_stops(&self) -> bool {
        wait_for(GONE, || !alive(self.stressor))
    }

    /// Changes the load at once: a file the script reads half written would
    /// be no load.
    fn set_load(&self, load: &str) {
        fs::write(self.dir.join("loadavg.new"), load).unwrap();
        fs::rename(self.dir.join("loadavg.new"), self.dir.join("loadavg")).unwrap();
    }

    /// Ends the script with SIGTERM, and what it wrote to stderr.
    fn stop(&mut self) -> String {
        kill(self.child.id() as i32, libc::SIGTERM);
        assert!(wait_exit(&mut self.child, GONE), "SIGTERM ends the script");
        let mut err = String::new();
        self.child.stderr.take().unwrap().read_to_string(&mut err).unwrap();
        err
    }
}

impl Drop for Loop {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if alive(self.stressor) {
            kill(self.stressor, libc::SIGKILL);
        }
        let _ = fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn sigkill_leaves_no_stressor() {
    let mut run = Loop::start("kill", &[], "10");
    run.child.kill().unwrap();
    run.child.wait().unwrap();
    assert!(run.stressor_stops(), "a stressor outlived the script's SIGKILL");
}

#[test]
fn sigterm_leaves_no_stressor() {
    let mut run = Loop::start("term", &[], "10");
    kill(run.child.id() as i32, libc::SIGTERM);
    assert!(wait_exit(&mut run.child, GONE), "SIGTERM ends the script");
    assert!(run.stressor_stops(), "a stressor outlived the script's SIGTERM");
    assert_eq!(run.child.wait().unwrap().code(), Some(143));
}

#[test]
fn an_end_leaves_no_stressor() {
    let mut run = Loop::start("end", &[], "0.2");
    let mut rest = String::new();
    run.out.read_to_string(&mut rest).unwrap();
    assert!(run.child.wait().unwrap().success());
    assert_eq!(rest, "runs=1 fails=0\n");
    assert!(run.stressor_stops(), "a stressor outlived the script's end");
}

#[test]
fn a_stressor_stops_at_its_time_limit() {
    let mut run = Loop::start("limit", &["-t", "1"], "10");
    assert!(run.stressor_stops(), "a stressor ran past -t");
    assert!(run.child.try_wait().unwrap().is_none(), "the script still runs");
    kill(run.child.id() as i32, libc::SIGTERM);
    assert!(wait_exit(&mut run.child, GONE), "SIGTERM ends the script");
}

#[test]
fn a_load_at_the_limit_stops_the_stressors() {
    let mut run = Loop::start_with("load", &["-l", "4"], "30", Some("0.50 0.50 0.50 1/1 1\n"));
    // The script looks at the load every second.
    sleep(Duration::from_millis(2500));
    assert!(alive(run.stressor), "a load below -l stopped the stressor");
    run.set_load("4.00 0.50 0.50 1/1 1\n");
    assert!(run.stressor_stops(), "a load at -l left the stressor");
    assert!(run.child.try_wait().unwrap().is_none(), "the run goes on");
    let err = run.stop();
    let says = "the load average is 4.00, at or above -l 4: the stressors are stopped";
    assert!(err.contains(says), "{err}");
}

#[test]
fn an_unreadable_load_is_refused() {
    let out = Command::new(SCRIPT)
        .env("FLAKE_LOOP_LOADAVG", "/nonexistent/loadavg")
        .args(["-r", "1", "-p", "1", "-s", "1", "/bin/sh"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains("can't read the load average"), "{}", stderr(&out));
    assert!(!String::from_utf8_lossy(&out.stdout).contains("stressors:"), "a stressor started");
}

#[test]
fn a_load_at_the_limit_starts_no_stressor() {
    let dir = std::env::temp_dir().join(format!("brnr-t{}-high", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("loadavg"), "4.00 0.50 0.50 1/1 1\n").unwrap();
    let out = Command::new(SCRIPT)
        .env("FLAKE_LOOP_LOADAVG", dir.join("loadavg"))
        .args(["-l", "4", "-r", "1", "-p", "1", "-s", "1", "-o", dir.to_str().unwrap()])
        .arg("/usr/bin/true")
        .output()
        .unwrap();
    let _ = fs::remove_dir_all(&dir);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "runs=1 fails=0\n", "a stressor started");
    let says = "the load average is 4.00, at or above -l 4: no stressors, the runs go on";
    assert!(stderr(&out).contains(says), "{}", stderr(&out));
}

#[test]
fn a_run_takes_ctrl_c_as_cargo_test_leaves_it() {
    // A run that SIGINTs itself dies of it, as under cargo test, unless the
    // script started it with SIGINT ignored: then it says "ignored".
    let dir = std::env::temp_dir().join(format!("brnr-t{}-int", std::process::id()));
    let out = Command::new(SCRIPT)
        .args(["-r", "1", "-p", "1", "-s", "0", "-o", dir.to_str().unwrap()])
        .args(["/bin/sh", "-c", "kill -INT $$; echo ignored"])
        .output()
        .unwrap();
    let log = fs::read_to_string(dir.join("1.1.log"));
    let _ = fs::remove_dir_all(&dir);
    assert_eq!(String::from_utf8_lossy(&out.stdout).lines().next(), Some("runs=1 fails=1"));
    assert_eq!(log.unwrap(), "", "the run went on past its SIGINT");
}

/// The script with one run of `sh -c <script> sh <file>`, which writes the
/// pid of what it starts to file; on drop the script, and that pid if it is
/// still there, are killed, and the directory removed.
struct Tree {
    child: Child,
    dir: PathBuf,
    pid: Option<i32>,
}

impl Tree {
    fn start(name: &str, script: &str) -> Tree {
        let dir = std::env::temp_dir().join(format!("brnr-t{}-{name}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let child = Command::new(SCRIPT)
            .args(["-r", "1", "-p", "1", "-s", "0", "-o", dir.to_str().unwrap()])
            .args(["/bin/sh", "-c", script, "sh"])
            .arg(dir.join("pid"))
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        Tree { child, dir, pid: None }
    }

    /// The pid of what the run started, once it has written it.
    fn started(&mut self) -> i32 {
        let file = self.dir.join("pid");
        wait_for(GONE, || {
            self.pid = fs::read_to_string(&file).ok().and_then(|p| p.trim().parse().ok());
            self.pid.is_some()
        });
        self.pid.expect("the run started its sleep")
    }

    fn started_stops(&self) -> bool {
        wait_for(GONE, || !alive(self.pid.unwrap()))
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(pid) = self.pid.filter(|&pid| alive(pid)) {
            kill(pid, libc::SIGKILL);
        }
        let _ = fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn sigterm_stops_what_a_run_started() {
    let mut run = Tree::start("tree", "/bin/sleep 60 & echo $! >\"$1\"; wait");
    run.started();
    kill(run.child.id() as i32, libc::SIGTERM);
    assert!(wait_exit(&mut run.child, GONE), "SIGTERM ends the script");
    assert!(run.started_stops(), "what a run started outlived the script's SIGTERM");
}

#[test]
fn an_end_stops_what_a_run_left() {
    let mut run = Tree::start("left", "/bin/sleep 60 & echo $! >\"$1\"; exit 0");
    run.started();
    let mut out = String::new();
    run.child.stdout.take().unwrap().read_to_string(&mut out).unwrap();
    assert!(wait_exit(&mut run.child, GONE), "the script ends");
    assert!(run.child.wait().unwrap().success());
    assert_eq!(out, "runs=1 fails=0\n");
    assert!(run.started_stops(), "what a run left outlived the script");
}

#[test]
fn a_hunt_that_would_run_nothing_is_refused() {
    for flag in ["-r", "-p", "-t", "-l"] {
        let out = Command::new(SCRIPT).args([flag, "0", "/bin/sh"]).output().unwrap();
        assert_eq!(out.status.code(), Some(2), "{flag} 0");
        assert!(stderr(&out).contains("from 1"), "{flag} 0: {}", stderr(&out));
    }
    // One past bash's largest integer would wrap around.
    let out = Command::new(SCRIPT).args(["-l", "9223372036854775808", "/bin/sh"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
}
