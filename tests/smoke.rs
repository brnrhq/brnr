//! `adapters/smoke.py`, the smoke check for agents that speak ACP
//! themselves, run against fake_agent.py as the agent (ADR 11.9): what it
//! checks, what it calls unsupported, and that it fails a check that fails.

mod common;

use std::process::Command;

use common::AGENT;

const SMOKE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/adapters/smoke.py");

/// smoke.py's exit status and output, with fake_agent.py as the agent,
/// set up by `vars`. Its prompts are the fake agent's: "reply ok" and
/// "hang", which runs until cancelled.
fn smoke(vars: &[(&str, &str)]) -> (Option<i32>, String) {
    let out = Command::new(SMOKE)
        .args(["--brnr", env!("CARGO_BIN_EXE_brnr"), "--", AGENT])
        .env("BRNR_SMOKE_PROMPT", "reply ok")
        .env("BRNR_SMOKE_LONG_PROMPT", "hang")
        .env("BRNR_SMOKE_TIMEOUT", "30")
        .envs(vars.iter().copied())
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    (out.status.code(), text + &String::from_utf8_lossy(&out.stderr))
}

fn has(output: &str, lines: &[&str]) {
    for line in lines {
        assert!(output.lines().any(|l| l.starts_with(line)), "no {line:?} in:\n{output}");
    }
}

#[test]
fn adr_0011_the_smoke_check_runs_every_check_an_agent_advertises() {
    let (status, out) = smoke(&[]);
    assert_eq!(status, Some(0), "{out}");
    has(
        &out,
        &[
            "brnr: brnr ",
            "agent: ",
            "ok   initialize: fake-agent 1.2.3; advertises resume, load, list, fork, close",
            "ok   session/new: sess-1",
            "ok   prompt: replied 'ok' (end_turn)",
            "ok   cancel: the turn ended cancelled",
            "ok   fork sess-2, its prompt: replied 'ok'",
            "ok   fork, closed: sess-2",
            "ok   shutdown: process ",
            "ok   list: the agent lists sess-1",
            "ok   resume (session/resume): replied 'ok'",
            "ok   close: closed; process ",
        ],
    );
    assert!(!out.contains("FAIL"), "{out}");
}

#[test]
fn adr_0011_the_smoke_check_says_what_the_agent_doesnt_advertise() {
    let none = [("NO_FORK", "1"), ("NO_LIST", "1"), ("NO_CLOSE", "1"), ("NO_RESUME", "1")];
    let (status, out) = smoke(&none);
    assert_eq!(status, Some(0), "{out}");
    has(
        &out,
        &[
            "--   fork: unsupported (sessionCapabilities.fork)",
            "--   list: unsupported (sessionCapabilities.list)",
            "ok   resume (session/load): replied 'ok'",
            "--   close: unsupported (sessionCapabilities.close)",
            "ok   shutdown, after resume: process ",
        ],
    );
    let (status, out) = smoke(&[("NO_RESUME", "1"), ("NO_LOAD", "1")]);
    assert_eq!(status, Some(0), "{out}");
    has(
        &out,
        &[
            "--   resume: unsupported (sessionCapabilities.resume, loadSession)",
            "--   close: not run: no session was resumed to close",
        ],
    );
}

#[test]
fn adr_0011_the_smoke_check_fails_what_fails() {
    // A reply that isn't "ok", wherever a prompt is checked.
    let (status, out) = Command::new(SMOKE)
        .args(["--brnr", env!("CARGO_BIN_EXE_brnr"), "--", AGENT])
        .env("BRNR_SMOKE_PROMPT", "reply ok, and more")
        .env("BRNR_SMOKE_LONG_PROMPT", "hang")
        .env("BRNR_SMOKE_TIMEOUT", "30")
        .output()
        .map(|o| (o.status.code(), String::from_utf8_lossy(&o.stdout).into_owned()))
        .unwrap();
    assert_eq!(status, Some(1), "{out}");
    has(
        &out,
        &[
            "FAIL prompt: replied 'ok, and more', stop reason end_turn",
            "FAIL fork sess-2, its prompt: replied 'ok, and more'",
            "FAIL resume (session/resume): replied 'ok, and more'",
        ],
    );
    // A turn that ends before it can be cancelled.
    let (status, out) = smoke(&[("BRNR_SMOKE_LONG_PROMPT", "reply quick")]);
    assert_eq!(status, Some(1), "{out}");
    has(&out, &["FAIL cancel: "]);
    // An agent that doesn't start.
    let (status, out) = smoke(&[("EXIT", "3")]);
    assert_eq!(status, Some(1), "{out}");
    has(&out, &["FAIL initialize: ", "FAIL session/new: no process"]);
}
