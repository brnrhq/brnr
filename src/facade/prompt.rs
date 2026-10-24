//! `brnr prompt`: what is sent to a session's agent, and the turn's
//! cancellation (ADR 4.2, 4.3).

use serde::{Deserialize, Serialize};

use super::{Dropped, Error, Exit, Reply, Words};

/// `prompt send`'s modes, which don't combine (ADR 4.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Mode {
    /// `--steer`: into the running turn.
    Steer,
    /// `--interrupt`: cancel the running turn, and go next.
    Interrupt,
    /// `--context`: for the next prompt; `replace` replaces the last entry.
    Context { replace: bool },
}

/// `prompt send`'s flags. One of `prompt`, `file` and `image` is needed.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct SendFlags {
    /// `--steer`, `--interrupt` or `--context [--replace]`; a prompt of its
    /// own without.
    pub mode: Option<Mode>,
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
}

/// What `prompt send --json` prints: the message accepted, or with `wait`
/// its turn.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
#[non_exhaustive]
// Returned once per call, not kept in bulk: a box would only cost the caller.
#[allow(clippy::large_enum_variant)]
pub enum Sent {
    Turn(Reply),
    Accepted(Accepted),
}

/// A message accepted: `status` is `delivered`, `held`, `steered` or
/// `interrupting`; `message` is its id, `None` for context, which has none.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Accepted {
    pub status: String,
    pub session: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// `prompt cancel`'s flags.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct CancelFlags {
    /// `--keep-held`: keep the held messages.
    pub keep_held: bool,
}

/// `prompt cancel --json`: `status` is `cancelling` or `idle`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Cancelled {
    pub status: String,
    pub session: String,
    pub dropped: Vec<Dropped>,
}

/// `prompt commands --json`: the slash commands the agent advertised.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Commands {
    pub session: String,
    pub commands: Vec<Command>,
}

/// A slash command: its name without the `/`, and its input's hint.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Command {
    pub command: String,
    pub hint: Option<String>,
    pub description: String,
}

/// `brnr prompt send <session>`.
pub fn send(session: &str, flags: &SendFlags) -> Result<Exit<Sent>, Error> {
    let mut w = Words::new("prompt", Some("send"));
    w.arg(session);
    match flags.mode {
        Some(Mode::Steer) => w.flag("--steer", true),
        Some(Mode::Interrupt) => w.flag("--interrupt", true),
        Some(Mode::Context { replace }) => {
            w.flag("--context", true);
            w.flag("--replace", replace);
        }
        None => {}
    }
    w.prompt(flags.prompt.as_deref());
    w.values("--file", &flags.file);
    w.values("--image", &flags.image);
    w.flag("--wait", flags.wait);
    w.value("--timeout", flags.timeout);
    match flags.wait {
        true => Ok(w.exit()?.map(Sent::Turn)),
        false => Ok(w.exit()?.map(Sent::Accepted)),
    }
}

/// `brnr prompt cancel <session>`.
pub fn cancel(session: &str, flags: &CancelFlags) -> Result<Cancelled, Error> {
    let mut w = Words::new("prompt", Some("cancel"));
    w.arg(session);
    w.flag("--keep-held", flags.keep_held);
    w.json()
}

/// `brnr prompt commands <session>`.
pub fn commands(session: &str) -> Result<Commands, Error> {
    let mut w = Words::new("prompt", Some("commands"));
    w.arg(session);
    w.json()
}
