//! `brnr permission`: the agent's permission requests, held as approvals
//! until they are answered (ADR 4.5).

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{Error, Words};

/// `permission requests --json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Requests {
    pub requests: Vec<Row>,
}

/// An approval waiting, as `permission requests` lists it: [`Request`]
/// with fewer fields. `options` are ACP's, as the agent sent them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Row {
    pub session: String,
    pub request: String,
    pub owner: String,
    pub kind: Option<String>,
    pub title: Option<String>,
    pub options: Vec<Value>,
}

/// `permission show --json`: the approval in full. `tool_call` and
/// `options` are ACP's, as the agent sent them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Request {
    pub request: String,
    pub session: String,
    pub owner: String,
    pub answerable: bool,
    pub why_not: Option<String>,
    pub title: Option<String>,
    pub kind: Option<String>,
    pub tool_call: Value,
    pub options: Vec<Value>,
    pub timeout_left: Option<u64>,
}

/// `permission allow --json` and `reject --json`: `outcome` is ACP's, as
/// sent to the agent.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Answered {
    pub session: String,
    pub request: String,
    pub outcome: Value,
}

/// `permission allow`'s and `reject`'s flags.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct AnswerFlags {
    /// `--always`: the `always` kind, in place of the `once` kind.
    pub always: bool,
    /// `--option-id`: the agent's option of this id instead.
    pub option_id: Option<String>,
}

/// `brnr permission requests [<session>]`: in every process that answers,
/// or in one session.
pub fn requests(session: Option<&str>) -> Result<Requests, Error> {
    let mut w = Words::new("permission", Some("requests"));
    w.args(session);
    w.json()
}

/// `brnr permission show <session> <request>`.
pub fn show(session: &str, request: &str) -> Result<Request, Error> {
    let mut w = Words::new("permission", Some("show"));
    w.args([session, request]);
    w.json()
}

/// `brnr permission allow <session> <request>`.
pub fn allow(session: &str, request: &str, flags: &AnswerFlags) -> Result<Answered, Error> {
    answer("allow", session, request, flags)
}

/// `brnr permission reject <session> <request>`.
pub fn reject(session: &str, request: &str, flags: &AnswerFlags) -> Result<Answered, Error> {
    answer("reject", session, request, flags)
}

fn answer(
    verb: &str,
    session: &str,
    request: &str,
    flags: &AnswerFlags,
) -> Result<Answered, Error> {
    let mut w = Words::new("permission", Some(verb));
    w.args([session, request]);
    w.flag("--always", flags.always);
    w.value("--option-id", flags.option_id.as_ref());
    w.json()
}
