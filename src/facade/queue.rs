//! `brnr queue`: what a session holds for later, the messages sent while a
//! turn runs and the context for the next prompt (ADR 4.2).

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{Dropped, Error, Words};

/// What `queue list`, `drop` and `clear` print: what is held after the
/// command, and what it dropped (`reason` `queue`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Queue {
    pub session: String,
    pub held: Vec<Held>,
    pub context: Vec<String>,
    pub dropped: Vec<Dropped>,
}

/// A held message, in the order they go: interrupts first.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Held {
    pub message: String,
    pub text: String,
    pub interrupt: bool,
    pub attachments: u64,
}

/// `queue show --json`: one held message, its attachments as ACP's content
/// blocks.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Message {
    pub session: String,
    pub message: String,
    pub text: String,
    pub interrupt: bool,
    pub attachments: u64,
    pub blocks: Vec<Value>,
}

/// `queue clear`'s flags; with neither, both are dropped.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct ClearFlags {
    /// `--messages`: drop the held messages.
    pub messages: bool,
    /// `--context`: drop the held context.
    pub context: bool,
}

/// `brnr queue list <session>`.
pub fn list(session: &str) -> Result<Queue, Error> {
    let mut w = Words::new("queue", Some("list"));
    w.arg(session);
    w.json()
}

/// `brnr queue show <session> <message>`.
pub fn show(session: &str, message: &str) -> Result<Message, Error> {
    let mut w = Words::new("queue", Some("show"));
    w.args([session, message]);
    w.json()
}

/// `brnr queue drop <session> <message>`.
pub fn drop(session: &str, message: &str) -> Result<Queue, Error> {
    let mut w = Words::new("queue", Some("drop"));
    w.args([session, message]);
    w.json()
}

/// `brnr queue clear <session> [--messages] [--context]`.
pub fn clear(session: &str, flags: &ClearFlags) -> Result<Queue, Error> {
    let mut w = Words::new("queue", Some("clear"));
    w.arg(session);
    w.flag("--messages", flags.messages);
    w.flag("--context", flags.context);
    w.json()
}
