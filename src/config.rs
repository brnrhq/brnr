//! Profiles from `config.toml` (see paths.rs for where it lives).
//!
//! ```toml
//! [profiles.slack]
//! agent = ["brnr-claude-adapter"]    # used when no agent is given after --
//! cwd = "~/work/project"             # sessions started headless (brnr start)
//! permission_timeout = 600           # seconds until an unanswered request is denied
//! mode = "plan"                      # sessions started headless: the agent's mode,
//! config = { model = "opus" }        # and config options, before the first prompt
//! stop_when_idle = 600               # close a headless session idle this many seconds
//! log = true                         # transcripts under ~/.brnr
//!
//! [[profiles.slack.bridges]]
//! command = ["~/bin/slack-bridge", "--channel", "#agents"]
//! events = ["permission_request", "turn_ended"]   # omit for every event
//!
//! [[profiles.slack.mcp_servers]]     # for sessions started headless
//! name = "github"
//! command = "github-mcp-server"      # stdio; or url = "https://…" (type = "http" | "sse")
//! args = ["stdio"]
//! env = { GITHUB_TOKEN = "…" }
//! ```
//!
//! Without `--profile`, the `default` profile applies if there is one.

use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::paths;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigFile {
    #[serde(default)]
    profiles: BTreeMap<String, Profile>,
}

#[derive(Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub agent: Option<Vec<String>>,
    pub cwd: Option<String>,
    pub permission_timeout: Option<u64>,
    pub mode: Option<String>,
    pub config: Option<BTreeMap<String, String>>,
    pub stop_when_idle: Option<u64>,
    pub log: Option<bool>,
    #[serde(default)]
    pub bridges: Vec<Bridge>,
    #[serde(default)]
    pub mcp_servers: Vec<McpServer>,
}

/// An MCP server for sessions the host opens: stdio (`command`) or remote
/// (`url`, with `type` http or sse).
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpServer {
    pub name: String,
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    pub url: Option<String>,
    #[serde(rename = "type")]
    pub kind: Option<String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}

impl McpServer {
    /// As ACP's `McpServer`.
    pub fn to_acp(&self) -> Result<Value, String> {
        let pairs = |map: &BTreeMap<String, String>| -> Vec<Value> {
            map.iter().map(|(name, value)| json!({ "name": name, "value": value })).collect()
        };
        let name = &self.name;
        match (&self.command, &self.url) {
            (Some(command), None) => {
                if self.kind.is_some() || !self.headers.is_empty() {
                    return Err(format!("MCP server {name}: type and headers are for url servers"));
                }
                Ok(json!({
                    "name": name,
                    "command": paths::expand(command),
                    "args": self.args,
                    "env": pairs(&self.env),
                }))
            }
            (None, Some(url)) => {
                let kind = self.kind.as_deref().unwrap_or("http");
                if !matches!(kind, "http" | "sse") {
                    return Err(format!("MCP server {name}: type must be http or sse, not {kind}"));
                }
                if !self.args.is_empty() || !self.env.is_empty() {
                    return Err(format!("MCP server {name}: args and env are for command servers"));
                }
                Ok(
                    json!({ "type": kind, "name": name, "url": url, "headers": pairs(&self.headers) }),
                )
            }
            _ => Err(format!("MCP server {name}: give either command or url")),
        }
    }
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bridge {
    pub command: Vec<String>,
    pub events: Option<Vec<String>>,
}

/// The named profile, or `default` (or nothing) when no name is given.
pub fn load(name: Option<&str>) -> Result<Profile, String> {
    let path = paths::config_file();
    let profiles = match load_all() {
        Ok(Some(profiles)) => profiles,
        Ok(None) if name.is_none() => return Ok(Profile::default()),
        Ok(None) => return Err(format!("{}: no such file", path.display())),
        Err(err) => return Err(err),
    };
    match name {
        Some(name) => profiles
            .get(name)
            .cloned()
            .ok_or_else(|| format!("no profile {name:?} in {}", path.display())),
        None => Ok(profiles.get("default").cloned().unwrap_or_default()),
    }
}

/// Every profile, or `None` if there is no config file.
pub fn load_all() -> Result<Option<BTreeMap<String, Profile>>, String> {
    let path = paths::config_file();
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(format!("{}: {err}", path.display())),
    };
    let file: ConfigFile =
        toml::from_str(&text).map_err(|e| format!("{}: {}", path.display(), e.message()))?;
    Ok(Some(file.profiles))
}
