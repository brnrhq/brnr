//! `brnr session`: sessions, the agent's conversations, open in a process
//! or with a transcript (ADR 3).

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{Agent, Error, Exit, Reply, Turn, Words};

/// `session new`'s and `session resume`'s flags (`<new flags>`): the
/// process's, the session's and the prompt's. `--foreground` and `--quiet`
/// aren't here: they attach the process to a terminal (ADR 8.5);
/// [`super::event::watch`] shows a session's events.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct StartFlags {
    /// `--pid`: open the session in this running process; the process
    /// flags and `agent` are then a usage error.
    pub pid: Option<u32>,
    /// `--profile`.
    pub profile: Option<String>,
    /// `--auth`: a noninteractive login method of the agent's.
    pub auth: Option<String>,
    /// `--strict`: stable ACP only.
    pub strict: bool,
    /// `--stop-when-idle`, in seconds.
    pub stop_when_idle: Option<u64>,
    /// `--response-timeout`, in seconds.
    pub response_timeout: Option<u64>,
    /// `--cwd`.
    pub cwd: Option<String>,
    /// `--mode`.
    pub mode: Option<String>,
    /// `--model`.
    pub model: Option<String>,
    /// `--thought-level`.
    pub thought_level: Option<String>,
    /// `--option <o>=<v>`: config options by id.
    pub option: Vec<(String, String)>,
    /// `--prompt`: sent as given.
    pub prompt: Option<String>,
    /// `--file`: attached as resource links.
    pub file: Vec<String>,
    /// `--image`: attached as image blocks.
    pub image: Vec<String>,
    /// `--wait`: wait for the turn to end.
    pub wait: bool,
    /// `--timeout`, in seconds, with `wait`.
    pub timeout: Option<u64>,
    /// `--take-over`, `resume`'s only: ask the process holding the session
    /// to close it first.
    pub take_over: bool,
    /// `-- <agent> [args...]`: the agent to run, in place of the profile's.
    pub agent: Option<Vec<String>>,
}

/// What `session new --json` and `session resume --json` print: the
/// session, or with `wait` its turn.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
#[non_exhaustive]
// Returned once per call, not kept in bulk: a box would only cost the caller.
#[allow(clippy::large_enum_variant)]
pub enum Started {
    Turn(Reply),
    Opened(Opened),
}

/// A session opened: its process, and its prompt's message id, `None`
/// without a prompt.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Opened {
    pub session: String,
    pub pid: u32,
    pub message: Option<String>,
}

/// `session status --json`: a running session's state. `tools` are the
/// running turn's tool calls, as `tool_call` has them; `plan` and `usage`
/// are as the agent reported them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Status {
    pub session: String,
    pub title: Option<String>,
    pub pid: u32,
    pub agent: Agent,
    pub owner: String,
    pub lock_pid: Option<u32>,
    pub shared_pids: Vec<u32>,
    pub lock_error: Option<String>,
    pub cwd: String,
    pub forked_from: Option<String>,
    pub state: String,
    pub turn_elapsed: Option<u64>,
    pub mode: Option<String>,
    pub model: Option<String>,
    pub tools: Vec<Value>,
    pub plan: Option<Value>,
    pub pending_count: u64,
    pub elicitation_count: u64,
    pub held_count: u64,
    pub context_count: u64,
    pub usage: Option<Value>,
    pub last_turn: Option<Turn>,
    pub last_message: Option<String>,
    pub last_active: String,
    pub stop_when_idle: Option<u64>,
    pub stopping: bool,
    pub uptime: u64,
}

/// `session fork --json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Forked {
    pub session: String,
    pub forked_from: String,
}

/// `session close --json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Closed {
    pub session: String,
}

/// `session delete`'s flags.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct DeleteFlags {
    /// `--purge`: also delete brnr's transcript of it.
    pub purge: bool,
    /// `--profile`: this profile's agent.
    pub profile: Option<String>,
    /// `-- <agent> [args...]`: this agent.
    pub agent: Option<Vec<String>>,
}

/// `session delete --json`: whether the agent deleted it, and what brnr
/// did with its transcript.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Deleted {
    pub session: String,
    pub deleted: bool,
    pub error: Option<Error>,
    pub recorded_in: Vec<String>,
    pub purged: Vec<String>,
    pub failed: Vec<String>,
}

/// `session list`'s flags.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct ListFlags {
    /// `--include`: `active`, `inactive`, or both (the default, as empty).
    pub include: Vec<String>,
    /// `--profile`: also ask this profile's agent.
    pub profile: Option<String>,
    /// `--cwd`: only this directory's sessions.
    pub cwd: Option<String>,
    /// `-- <agent> [args...]`: also ask this agent.
    pub agent: Option<Vec<String>>,
}

/// `session list --json`: most recently active first.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Sessions {
    pub sessions: Vec<Row>,
}

/// A session, as `session list` has it: [`Status`] with fewer fields.
/// `state` is as `status` has it, or `unreachable`, or `inactive`; `source`
/// is `brnr`, `agent` or `both`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Row {
    pub session: String,
    pub title: Option<String>,
    pub state: String,
    pub pid: Option<u32>,
    pub agent: Agent,
    pub source: String,
    pub last_active: Option<String>,
    pub forked_from: Option<String>,
    pub cwd: Option<String>,
}

/// `brnr session new`: a process for a headless session, in the background,
/// and the session it opens; with `pid`, a session in that process. The
/// process is this program, started again as `<program> host` (ADR 8.5).
pub fn new(flags: &StartFlags) -> Result<Exit<Started>, Error> {
    start(Words::new("session", Some("new")), flags)
}

/// `brnr session resume <session>`: as [`new`], carrying on a session.
pub fn resume(session: &str, flags: &StartFlags) -> Result<Exit<Started>, Error> {
    let mut w = Words::new("session", Some("resume"));
    w.arg(session);
    start(w, flags)
}

fn start(mut w: Words, f: &StartFlags) -> Result<Exit<Started>, Error> {
    w.value("--pid", f.pid);
    w.value("--profile", f.profile.as_ref());
    w.value("--auth", f.auth.as_ref());
    w.flag("--strict", f.strict);
    w.value("--stop-when-idle", f.stop_when_idle);
    w.value("--response-timeout", f.response_timeout);
    w.value("--cwd", f.cwd.as_ref());
    w.value("--mode", f.mode.as_ref());
    w.value("--model", f.model.as_ref());
    w.value("--thought-level", f.thought_level.as_ref());
    w.pairs("--option", &f.option);
    w.prompt(f.prompt.as_deref());
    w.values("--file", &f.file);
    w.values("--image", &f.image);
    w.flag("--wait", f.wait);
    w.value("--timeout", f.timeout);
    w.flag("--take-over", f.take_over);
    w.rest(f.agent.as_deref());
    match f.wait {
        true => Ok(w.exit()?.map(Started::Turn)),
        false => Ok(w.exit()?.map(Started::Opened)),
    }
}

/// `brnr session status <session>`.
pub fn status(session: &str) -> Result<Status, Error> {
    let mut w = Words::new("session", Some("status"));
    w.arg(session);
    w.json()
}

/// `brnr session fork <session>`.
pub fn fork(session: &str) -> Result<Forked, Error> {
    let mut w = Words::new("session", Some("fork"));
    w.arg(session);
    w.json()
}

/// `brnr session close <session>`.
pub fn close(session: &str) -> Result<Closed, Error> {
    let mut w = Words::new("session", Some("close"));
    w.arg(session);
    w.json()
}

/// `brnr session delete <session>`: [`Exit::Failure`] if the agent didn't
/// delete it, or a file couldn't be deleted or recorded in.
pub fn delete(session: &str, flags: &DeleteFlags) -> Result<Exit<Deleted>, Error> {
    let mut w = Words::new("session", Some("delete"));
    w.arg(session);
    w.flag("--purge", flags.purge);
    w.value("--profile", flags.profile.as_ref());
    w.rest(flags.agent.as_deref());
    w.exit()
}

/// `brnr session list`.
pub fn list(flags: &ListFlags) -> Result<Sessions, Error> {
    let mut w = Words::new("session", Some("list"));
    let include = flags.include.join(",");
    w.value("--include", (!include.is_empty()).then_some(include));
    w.value("--profile", flags.profile.as_ref());
    w.value("--cwd", flags.cwd.as_ref());
    w.rest(flags.agent.as_deref());
    w.json()
}
