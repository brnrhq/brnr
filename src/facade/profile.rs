//! `brnr profile`: the profiles in the config file, read and never written
//! (ADR 6.5).

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::{Error, Words};

/// `profile list --json`: the config file, whether it exists, and its
/// profiles.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Profiles {
    pub config: String,
    pub exists: bool,
    pub profiles: Vec<Summary>,
}

/// A profile, as doctor's line for it has it: `agent` `None` where the
/// profile has none.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Summary {
    pub name: String,
    pub agent: Option<Vec<String>>,
    pub bridges: u64,
    pub mcp_servers: u64,
    pub strict: bool,
    pub experimental: Vec<String>,
    pub client_capabilities: Vec<String>,
    pub echo_as: String,
    pub features: Vec<String>,
}

/// `profile show [<name>] --json`: the profile as a start would use it, in
/// the file's layout (`agent`, …, `headless`, `editor`), with each key the
/// file doesn't set at its default, or null, and named in `unset`. MCP
/// servers' `env` and `headers` values are `<redacted>`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Shown {
    pub config: String,
    pub name: String,
    pub found: bool,
    pub profile: Map<String, Value>,
    pub unset: Vec<String>,
}

/// `profile show --pid <pid> --json`: the request the process was started
/// with, secrets redacted, without the prompt and `foreground`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Started {
    pub pid: u32,
    pub start_request: Value,
}

/// `profile keys --json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Keys {
    pub keys: Vec<Key>,
}

/// A key a profile can have: its part (`shared`, `headless`, `editor`),
/// its TOML type, its default, or the rule that applies without it, the
/// names it takes, and its ADR.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Key {
    pub key: String,
    pub part: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub default: Option<Value>,
    pub rule: Option<String>,
    pub names: Vec<String>,
    pub adr: String,
}

/// `brnr profile list`.
pub fn list() -> Result<Profiles, Error> {
    Words::new("profile", Some("list")).json()
}

/// `brnr profile show [<name>]`: the `default` profile, or the empty one,
/// without a name.
pub fn show(name: Option<&str>) -> Result<Shown, Error> {
    let mut w = Words::new("profile", Some("show"));
    w.args(name);
    w.json()
}

/// `brnr profile show --pid <pid>`.
pub fn show_pid(pid: u32) -> Result<Started, Error> {
    let mut w = Words::new("profile", Some("show"));
    w.value("--pid", Some(pid));
    w.json()
}

/// `brnr profile keys`.
pub fn keys() -> Result<Keys, Error> {
    Words::new("profile", Some("keys")).json()
}
