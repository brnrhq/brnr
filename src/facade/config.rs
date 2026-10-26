//! `brnr config`: a session's settings, the agent's config options and its
//! v1 modes (ADR 6.2).

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{Error, Words};

/// `config get --json`: the options, then the v1 modes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Options {
    pub session: String,
    pub options: Vec<ConfigOption>,
}

/// A config option, or the v1 modes (`option` and `name` `None`). `value`
/// is a value id, a string, or a boolean option's `true` or `false`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ConfigOption {
    pub option: Option<String>,
    pub category: Option<String>,
    pub value: Value,
    pub choices: Vec<Choice>,
    pub name: Option<String>,
    pub description: Option<String>,
}

/// A value an option takes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Choice {
    pub value: String,
    pub name: String,
    pub description: Option<String>,
}

/// `config set --json`: each setting sent, in order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Set {
    pub session: String,
    pub set: Vec<Setting>,
}

/// A setting sent: `option` is `None` for a v1 mode.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Setting {
    pub option: Option<String>,
    pub category: Option<String>,
    pub value: String,
}

/// `config get`'s flags: only these options; all without any.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct GetFlags {
    /// `--mode`.
    pub mode: bool,
    /// `--model`.
    pub model: bool,
    /// `--thought-level`.
    pub thought_level: bool,
    /// `--option <o>`: by id.
    pub option: Vec<String>,
}

/// `config set`'s flags; one of them is needed.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct SetFlags {
    /// `--mode <m>`.
    pub mode: Option<String>,
    /// `--model <m>`.
    pub model: Option<String>,
    /// `--thought-level <l>`.
    pub thought_level: Option<String>,
    /// `--option <o>=<v>`: by id.
    pub option: Vec<(String, String)>,
}

/// `brnr config get <session>`.
pub fn get(session: &str, flags: &GetFlags) -> Result<Options, Error> {
    let mut w = Words::new("config", Some("get"));
    w.arg(session);
    w.flag("--mode", flags.mode);
    w.flag("--model", flags.model);
    w.flag("--thought-level", flags.thought_level);
    w.values("--option", &flags.option);
    w.json()
}

/// `brnr config set <session>`.
pub fn set(session: &str, flags: &SetFlags) -> Result<Set, Error> {
    let mut w = Words::new("config", Some("set"));
    w.arg(session);
    w.value("--mode", flags.mode.as_ref());
    w.value("--model", flags.model.as_ref());
    w.value("--thought-level", flags.thought_level.as_ref());
    w.pairs("--option", &flags.option);
    w.json()
}
