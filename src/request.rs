//! The one request a brnr process is started with (ADR 8 in docs/adr).
//!
//! Whoever starts a process, `brnr start` or `brnr acp`, resolves everything
//! first: the profile, the agent, the cwd, the role and what goes with it.
//! It writes the request as one JSON value on the process's stdin and closes
//! it. The process reads its stdin to EOF before doing anything else, and
//! refuses to start on a request cut short; it reads no config and takes no
//! flags. Its fd 3 carries the editor link (`acp`) or the start channel
//! (`start`, ADR 7).

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ChildStdin;

use libc::c_int;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::{Bridge, Experimental, Feature, Log, Profile};
use crate::{host, paths, spawn};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    /// The profile's name, for the record (`list --inactive`, `--resume`).
    pub profile: Option<String>,
    /// The agent's command as it is run: an adapter found next to brnr (see
    /// spawn.rs), a profile's with `~` expanded.
    pub agent: Vec<String>,
    pub cwd: PathBuf,
    /// Stable ACP to the letter (ADR 41).
    pub strict: bool,
    /// What to record (ADR 22).
    pub log: Log,
    pub bridges: Vec<Bridge>,
    pub role: Role,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Editor(Editor),
    Headless(Headless),
}

/// `brnr acp`: the editor's process, linked to the proxy on fd 3.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Editor {
    pub proxy_pid: u32,
    /// The editor's signal mask, which the agent gets.
    pub sigmask: Vec<c_int>,
    /// Actions on the editor's session it allows (ADR 4).
    pub experimental: BTreeSet<Experimental>,
    /// Process-management behaviours it turns on (ADR 42).
    pub features: BTreeSet<Feature>,
}

/// `brnr start`: a headless session, reported on the start channel at fd 3.
#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Headless {
    /// Resume this session instead of opening a new one.
    pub resume: Option<String>,
    /// The mode and config options to set before the prompt.
    pub mode: Option<String>,
    pub config: BTreeMap<String, String>,
    /// As ACP has them.
    pub mcp_servers: Vec<Value>,
    /// The login method to run after `initialize` (ADR 30).
    pub auth: Option<String>,
    /// Sent once the start has committed.
    pub prompt: Option<Prompt>,
    /// Seconds until the start fails.
    pub start_timeout: u64,
    pub stop_when_idle: Option<u64>,
    pub permission_timeout: Option<u64>,
    /// What the start channel is subscribed to from the start: the events
    /// `start --wait` follows its turn by; none without `--wait`.
    pub events: Vec<String>,
    /// `start --foreground`: the process is `start`'s child, and shows the
    /// session on its stdout.
    pub foreground: Option<Foreground>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Prompt {
    pub text: String,
    /// Content blocks besides the text (`resource_link`, `image`).
    pub blocks: Vec<Value>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Foreground {
    pub quiet: bool,
    pub json: bool,
}

impl Request {
    /// What every process of `profile` (named `name`) is started with, for
    /// `agent` (or the profile's when empty) in `cwd`.
    pub fn new(
        name: Option<String>,
        profile: &Profile,
        mut agent: Vec<String>,
        cwd: PathBuf,
        role: Role,
    ) -> Result<Request, String> {
        if agent.is_empty() {
            let argv = profile.agent.iter().flatten();
            agent = argv.map(|a| paths::expand(a).to_string_lossy().into_owned()).collect();
        }
        let Some(program) = agent.first_mut() else {
            return Err("no agent: give one after -- or set agent in the profile".into());
        };
        if let Some(bundled) = spawn::bundled(OsStr::new(program.as_str())) {
            *program = bundled.to_string_lossy().into_owned();
        }
        for bridge in &profile.bridges {
            host::check_bridge(bridge)?;
        }
        Ok(Request {
            profile: name,
            agent,
            cwd,
            strict: profile.strict,
            log: profile.log,
            bridges: profile.bridges.clone(),
            role,
        })
    }

    pub fn headless(&self) -> Option<&Headless> {
        match &self.role {
            Role::Headless(h) => Some(h),
            Role::Editor(_) => None,
        }
    }

    /// Writes the request on the new process's stdin, and closes it.
    pub fn send(&self, mut stdin: ChildStdin) -> io::Result<()> {
        stdin.write_all(&serde_json::to_vec(self)?)
    }

    /// The request as the host log's `started` record holds it.
    pub fn recorded(&self) -> Value {
        // ADR 25 redacts the values of the MCP servers' `env` and `headers`
        // here: everything recorded of the request passes through this.
        serde_json::to_value(self).unwrap_or_default()
    }
}
