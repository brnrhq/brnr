//! `brnr elicitation`: the agent asking for input, a form or a URL, held
//! until it is answered (ADR 4.5).

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::{Error, Target, Words};

/// `elicitation requests --json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Requests {
    pub requests: Vec<Row>,
}

/// An elicitation waiting, as `elicitation requests` lists it: [`Request`]
/// with fewer fields. `session` is `None` for one outside any session.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Row {
    pub session: Option<String>,
    pub pid: u32,
    pub request: String,
    pub owner: String,
    pub mode: Option<String>,
    pub scope: String,
    pub text: Option<String>,
}

/// `elicitation show --json`: the elicitation in full. `schema` is the
/// form's, as the agent sent it; `scope_request_id` a string, a number or
/// null, as ACP has it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Request {
    pub mode: Option<String>,
    pub text: Option<String>,
    pub scope: String,
    pub tool_call_id: Option<String>,
    pub scope_request_id: Value,
    pub schema: Option<Value>,
    pub url: Option<String>,
    pub elicitation_id: Option<String>,
    pub request: String,
    pub session: Option<String>,
    pub pid: u32,
    pub owner: String,
    pub answerable: bool,
    pub why_not: Option<String>,
    pub timeout_left: Option<u64>,
}

/// `elicitation accept --json`, `decline --json` and `cancel --json`:
/// `content` is an accept's form values, with ACP's value types.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Answered {
    pub session: Option<String>,
    pub request: String,
    pub action: String,
    pub content: Option<Value>,
}

/// `elicitation accept`'s flags: the form's fields as `--field` gives them,
/// or as one JSON object, as `-` reads it on stdin; not both.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct AcceptFlags {
    /// `--field <k>=<v>`: a field, each value a string read as the form's
    /// type asks; given again, a list's next value.
    pub field: Vec<(String, String)>,
    /// `-`: the fields, with their JSON types.
    pub fields: Option<Map<String, Value>>,
}

/// `brnr elicitation requests [<session>]`.
pub fn requests(session: Option<&str>) -> Result<Requests, Error> {
    let mut w = Words::new("elicitation", Some("requests"));
    w.args(session);
    w.json()
}

/// `brnr elicitation show (<session> | --pid <pid>) <request>`.
pub fn show(target: &Target, request: &str) -> Result<Request, Error> {
    let mut w = Words::new("elicitation", Some("show"));
    w.target(target);
    w.arg(request);
    w.json()
}

/// `brnr elicitation accept (<session> | --pid <pid>) <request>`.
pub fn accept(target: &Target, request: &str, flags: &AcceptFlags) -> Result<Answered, Error> {
    let mut w = Words::new("elicitation", Some("accept"));
    w.target(target);
    w.arg(request);
    w.pairs("--field", &flags.field);
    if let Some(fields) = &flags.fields {
        w.stdin("-", "", Value::Object(fields.clone()).to_string());
    }
    w.json()
}

/// `brnr elicitation decline (<session> | --pid <pid>) <request>`.
pub fn decline(target: &Target, request: &str) -> Result<Answered, Error> {
    answer("decline", target, request)
}

/// `brnr elicitation cancel (<session> | --pid <pid>) <request>`.
pub fn cancel(target: &Target, request: &str) -> Result<Answered, Error> {
    answer("cancel", target, request)
}

fn answer(verb: &str, target: &Target, request: &str) -> Result<Answered, Error> {
    let mut w = Words::new("elicitation", Some(verb));
    w.target(target);
    w.arg(request);
    w.json()
}
