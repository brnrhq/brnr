//! tests/flake-loop.sh leaves nothing it started running, however it ends
//! (#123). Each process is found by a pid it is told, never by its name.

mod common;

use common::*;
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::PathBuf;
use std::process::{Child, ChildStdout, Command, Stdio};
use std::time::Duration;

const SCRIPT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/flake-loop.sh");
/// How long a stressor may take to stop: it says within a second.
const GONE: Duration = Duration::from_secs(5);

/// The script with one stressor beside one run of `sleep secs`, and the
/// stressor's pid; the stressor is killed on drop if it is still there, and
/// the directory of the runs' output removed.
struct Loop {
    child: Child,
    out: BufReader<ChildStdout>,
    stressor: i32,
    dir: PathBuf,
}

impl Loop {
    fn start(name: &str, args: &[&str], secs: &str) -> Loop {
        let dir = std::env::temp_dir().join(format!("brnr-t{}-{name}", std::process::id()));
        let mut child = Command::new(SCRIPT)
            .args(["-r", "1", "-p", "1", "-s", "1", "-o", dir.to_str().unwrap()])
            .args(args)
            .args(["/bin/sleep", secs])
            .stdout(Stdio::piped())
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

#[test]
fn sigterm_stops_what_a_run_started() {
    let dir = std::env::temp_dir().join(format!("brnr-t{}-tree", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let file = dir.join("pid");
    let mut child = Command::new(SCRIPT)
        .args(["-r", "1", "-p", "1", "-s", "0", "-o", dir.to_str().unwrap()])
        .args(["/bin/sh", "-c", "/bin/sleep 60 & echo $! >\"$1\"; wait", "sh"])
        .arg(&file)
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let mut pid = None;
    wait_for(GONE, || {
        pid = fs::read_to_string(&file).ok().and_then(|p| p.trim().parse::<i32>().ok());
        pid.is_some()
    });
    let pid = pid.expect("the run started its sleep");
    kill(child.id() as i32, libc::SIGTERM);
    assert!(wait_exit(&mut child, GONE), "SIGTERM ends the script");
    let stopped = wait_for(GONE, || !alive(pid));
    if !stopped {
        kill(pid, libc::SIGKILL);
    }
    let _ = fs::remove_dir_all(&dir);
    assert!(stopped, "what a run started outlived the script's SIGTERM");
}

#[test]
fn a_hunt_that_would_run_nothing_is_refused() {
    for flag in ["-r", "-p", "-t"] {
        let out = Command::new(SCRIPT).args([flag, "0", "/bin/sh"]).output().unwrap();
        assert_eq!(out.status.code(), Some(2), "{flag} 0");
        assert!(stderr(&out).contains("from 1"), "{flag} 0: {}", stderr(&out));
    }
}
