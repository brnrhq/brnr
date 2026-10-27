//! The one request a brnr process is started with (ADR 3.3 in docs/adr).
//!
//! Whoever starts a process, `brnr session new` (or `resume`) or `brnr acp`,
//! resolves everything first: the profile, the agent and the bridges'
//! commands (a bare name installed next to brnr found there, see spawn.rs),
//! the cwd, the role and what goes with it.
//! It writes the request as one JSON value on the process's stdin and closes
//! it. The process reads its stdin to EOF before doing anything else, and
//! refuses to start on a request cut short; it reads no config and takes no
//! flags. Its fd 3 carries the editor link (`acp`) or the start channel
//! (`session new` and `resume`, ADR 3.4); an editor's process has the proxy's
//! signal link on fd 4 as well (see frame.rs), and doesn't start without it.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ChildStdin;

use libc::c_int;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{Code, Error};
use crate::profiles::{Bridge, ClientCapability, EchoAs, Experimental, Feature, Log, Profile};
use crate::{host, log, paths, spawn};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Request {
    /// The profile's name, for the record (`session resume`).
    pub(crate) profile: Option<String>,
    /// The agent's command as it is run: an adapter found next to brnr (see
    /// spawn.rs), a profile's with `~` expanded.
    pub(crate) agent: Vec<String>,
    pub(crate) cwd: PathBuf,
    /// Stable ACP to the letter (ADR 2.1).
    pub(crate) strict: bool,
    /// What to record (ADR 5.1).
    pub(crate) log: Log,
    /// The profile's, each command as it is run, found as the agent's is.
    pub(crate) bridges: Vec<Bridge>,
    /// The config's `retention`, in seconds: the process prunes what was
    /// last active longer ago as it starts (ADR 9.5).
    pub(crate) retention: Option<u64>,
    pub(crate) role: Role,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Role {
    Editor(Editor),
    Headless(Box<Headless>),
}

/// `brnr acp`: the editor's process, joined to the proxy by the link on
/// fd 3 and the signal link on fd 4.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Editor {
    pub(crate) proxy_pid: u32,
    /// The editor's signal mask, which the agent gets.
    pub(crate) sigmask: Vec<c_int>,
    /// Actions on the editor's session it allows (ADR 7.1).
    pub(crate) experimental: BTreeSet<Experimental>,
    /// The editor's `fs` and `terminal` capabilities that reach the agent
    /// outside strict mode (ADR 2.7).
    pub(crate) client_capabilities: BTreeSet<ClientCapability>,
    /// How a message sent to its session through the side channel is shown
    /// to it (ADR 7.2).
    pub(crate) echo_as: EchoAs,
    /// Process-management behaviours it turns on (ADR 3.10).
    pub(crate) features: BTreeSet<Feature>,
}

/// `brnr session new` or `resume`: a headless session, reported on the
/// start channel at fd 3.
#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Headless {
    /// Resume this session instead of opening a new one.
    pub(crate) resume: Option<String>,
    /// brnr has a transcript of the session resumed: what a `session/load`
    /// replays is in it already, and isn't recorded again (ADR 5.6).
    pub(crate) transcript: bool,
    /// What the start's flags set (`--mode`, `--model`, `--thought-level`,
    /// `--option`), and what the profile does (`mode`, `model`,
    /// `thought_level`, `options`): the flags win, setting by setting, once
    /// the agent has said which option is which (ADR 6.1).
    pub(crate) settings: Settings,
    pub(crate) defaults: Settings,
    /// As ACP has them.
    pub(crate) mcp_servers: Vec<Value>,
    /// The login method to run after `initialize` (ADR 4.7).
    pub(crate) auth: Option<String>,
    /// Sent once the start has committed.
    pub(crate) prompt: Option<Prompt>,
    /// Seconds until the start fails.
    pub(crate) start_timeout: u64,
    pub(crate) stop_when_idle: Option<u64>,
    pub(crate) response_timeout: Option<u64>,
    /// What the start channel is subscribed to once the start commits: the
    /// events `--wait` follows its turn by; none without `--wait`.
    pub(crate) events: Vec<String>,
    /// `--foreground`: the process is the child of `session new` (or
    /// `resume`), and shows the session on its stdout.
    pub(crate) foreground: Option<Foreground>,
}

/// The mode, the model and the thought level (the config options of those
/// categories, ADR 6.2) and config options by id: what one source sets, a
/// start's flags or its profile, or `config set` (ADR 6.1, ADR 8.1).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Settings {
    pub(crate) mode: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) thought_level: Option<String>,
    pub(crate) options: BTreeMap<String, String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Prompt {
    pub(crate) text: String,
    /// Content blocks besides the text (`resource_link`, `image`).
    pub(crate) blocks: Vec<Value>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Foreground {
    pub(crate) quiet: bool,
    pub(crate) json: bool,
}

/// A bare name installed next to brnr, as that path (see `spawn::bundled`),
/// so the process runs it whatever its PATH.
fn next_to_brnr(program: &mut String) {
    if let Some(found) = spawn::bundled(OsStr::new(program.as_str())) {
        *program = found.to_string_lossy().into_owned();
    }
}

impl Request {
    /// What every process of `profile` (named `name`) is started with, for
    /// `agent` (or the profile's when empty) in `cwd`.
    pub(crate) fn new(
        name: Option<String>,
        profile: &Profile,
        mut agent: Vec<String>,
        cwd: PathBuf,
        role: Role,
    ) -> Result<Request, Error> {
        if agent.is_empty() {
            let argv = profile.agent.iter().flatten();
            agent = argv.map(|a| paths::expand(a).to_string_lossy().into_owned()).collect();
        }
        let Some(program) = agent.first_mut() else {
            let error = "no agent: give one after -- or set agent in the profile";
            return Err(Error::new(Code::agent_not_started, error));
        };
        next_to_brnr(program);
        // A bridge's command, `~` expanded, is found the same way, here:
        // the process runs what it is given (ADR 3.3, ADR 11.2).
        let mut bridges = profile.bridges.clone();
        for bridge in &mut bridges {
            host::check_bridge(bridge)?;
            let program = &mut bridge.command;
            *program = paths::expand(program).to_string_lossy().into_owned();
            next_to_brnr(program);
        }
        Ok(Request {
            profile: name,
            agent,
            cwd,
            strict: profile.strict,
            log: profile.log,
            bridges,
            retention: None,
            role,
        })
    }

    pub(crate) fn headless(&self) -> Option<&Headless> {
        match &self.role {
            Role::Headless(h) => Some(h),
            Role::Editor(_) => None,
        }
    }

    /// Writes the request on the new process's stdin, and closes it.
    pub(crate) fn send(&self, mut stdin: ChildStdin) -> io::Result<()> {
        stdin.write_all(&serde_json::to_vec(self)?)
    }

    /// The request as the host log's `process_started` record holds it: the values
    /// of the MCP servers' `env` and `headers` redacted (ADR 9.1).
    pub(crate) fn recorded(&self) -> Value {
        let mut request = serde_json::to_value(self).unwrap_or_default();
        if let Some(servers) = request.pointer_mut("/role/headless/mcp_servers") {
            log::redact_mcp_servers(servers);
        }
        request
    }
}

/// `recorded`, a request as [`Request::recorded`] has it, without what a
/// process's start alone needs: the prompt, which is the session's, and
/// `foreground`, which is the terminal's. What `brnr profile show --pid`
/// shows (ADR 6.5).
pub(crate) fn shown(recorded: &Value) -> Value {
    let mut request = recorded.clone();
    if let Some(Value::Object(headless)) = request.pointer_mut("/role/headless") {
        headless.remove("prompt");
        headless.remove("foreground");
    }
    request
}
