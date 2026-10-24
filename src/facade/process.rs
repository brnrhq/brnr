//! `brnr process`: the brnr processes, each running one agent for one or
//! more sessions (ADR 8.1).

use serde::{Deserialize, Serialize};

use super::{Agent, Error, Words};

/// `process list --json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Processes {
    pub processes: Vec<Process>,
}

/// A process: `state` is `starting`, `running`, `stopping` or `unreachable`
/// (it doesn't answer: `owner` and `uptime` are `None`, and its agent's
/// `name` and `version`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Process {
    pub pid: u32,
    pub state: String,
    pub owner: Option<String>,
    pub agent: Agent,
    pub sessions: Vec<Session>,
    pub uptime: Option<u64>,
    pub cwd: String,
}

/// A process's session, as a `session list` row with `session` and `state`
/// only.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Session {
    pub session: String,
    pub state: String,
}

/// `process stop --json`: `status` is `stopping`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Stopped {
    pub pid: u32,
    pub status: String,
}

/// `brnr process list`.
pub fn list() -> Result<Processes, Error> {
    Words::new("process", Some("list")).json()
}

/// `brnr process stop <pid>`: it doesn't wait for the process to exit.
pub fn stop(pid: u32) -> Result<Stopped, Error> {
    let mut w = Words::new("process", Some("stop"));
    w.arg(&pid.to_string());
    w.json()
}
