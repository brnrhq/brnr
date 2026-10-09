//! Profiles from `config.toml` (see paths.rs for where it lives). A profile
//! has three parts (ADR 33 in docs/adr): at the top what every process of
//! the profile uses, under `headless` what only `brnr session new` and
//! `resume` use, and under `editor` what only `brnr acp` uses.
//!
//! ```toml
//! [profiles.work]                     # every process of the profile
//! agent = ["brnr-claude-adapter"]     # used when no agent is given after --
//! log = "all"                         # transcripts under ~/.brnr; "events", or false
//! strict = false                      # stable ACP only (ADR 41)
//!
//! [[profiles.work.bridges]]
//! command = ["~/bin/slack-bridge", "--channel", "#agents"]
//! events = ["permission_request", "turn_ended"]   # omit for every event but acp
//!
//! [profiles.work.headless]            # brnr session new and resume
//! cwd = "~/work/project"
//! mode = "plan"                       # the agent's mode, model, thought
//! model = "opus"                      #   level and config options by id,
//! thought_level = "high"              #   set before the first prompt
//! options = { effort = "high" }
//! permission_timeout = 600            # seconds until an unanswered request is rejected
//! stop_when_idle = 600                # close a session idle this many seconds
//! auth = "api-key"                    # the agent's login method, run first (ADR 30)
//!
//! [[profiles.work.headless.mcp_servers]]
//! name = "github"
//! command = "github-mcp-server"       # stdio; or url = "https://…" (type = "http" | "sse")
//! args = ["stdio"]
//! env = { GITHUB_TOKEN = "…" }
//!
//! [profiles.work.editor]              # brnr acp
//! experimental = ["send", "permission"] # actions on the editor's session (ADR 4)
//! features = ["shared_sessions"]      # process management (ADR 42)
//! ```
//!
//! Without `--profile`, the `default` profile applies if there is one. A key
//! in the wrong part, an unknown key or name, and the flat layout of before
//! fail to load, saying which and where (P7, P9).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::io::ErrorKind;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
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
    #[serde(default)]
    pub log: Log,
    #[serde(default)]
    pub strict: bool,
    #[serde(default)]
    pub bridges: Vec<Bridge>,
    #[serde(default)]
    pub headless: Headless,
    #[serde(default)]
    pub editor: Editor,
}

/// What only `brnr session new` and `resume` use (ADR 12, 27, 28, 30, 31,
/// 63).
#[derive(Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Headless {
    pub cwd: Option<String>,
    pub mode: Option<String>,
    pub model: Option<String>,
    pub thought_level: Option<String>,
    #[serde(default)]
    pub options: BTreeMap<String, String>,
    pub permission_timeout: Option<u64>,
    pub stop_when_idle: Option<u64>,
    pub auth: Option<String>,
    #[serde(default)]
    pub mcp_servers: Vec<McpServer>,
}

/// What only `brnr acp` uses (ADR 4, 42).
#[derive(Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Editor {
    #[serde(default)]
    pub experimental: BTreeSet<Experimental>,
    #[serde(default)]
    pub features: BTreeSet<Feature>,
}

/// What the host records (ADR 22): `"all"`, `"events"` (no raw ACP) or
/// `false` (nothing).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Log {
    #[default]
    All,
    Events,
    Off,
}

impl Serialize for Log {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Log::All => s.serialize_str("all"),
            Log::Events => s.serialize_str("events"),
            Log::Off => s.serialize_bool(false),
        }
    }
}

impl<'de> Deserialize<'de> for Log {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Log, D::Error> {
        struct Visitor;
        impl serde::de::Visitor<'_> for Visitor {
            type Value = Log;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str(r#""all", "events" or false"#)
            }
            fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<Log, E> {
                if v {
                    Err(E::invalid_value(serde::de::Unexpected::Bool(v), &self))
                } else {
                    Ok(Log::Off)
                }
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Log, E> {
                match v {
                    "all" => Ok(Log::All),
                    "events" => Ok(Log::Events),
                    _ => Err(E::invalid_value(serde::de::Unexpected::Str(v), &self)),
                }
            }
        }
        d.deserialize_any(Visitor)
    }
}

/// An action on an editor's session through the side channel, enabled by
/// name (ADR 4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Experimental {
    Send,
    Context,
    Cancel,
    Permission,
    Config,
    Close,
}

impl Experimental {
    pub const ALL: [Experimental; 6] = [
        Experimental::Send,
        Experimental::Context,
        Experimental::Cancel,
        Experimental::Permission,
        Experimental::Config,
        Experimental::Close,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Experimental::Send => "send",
            Experimental::Context => "context",
            Experimental::Cancel => "cancel",
            Experimental::Permission => "permission",
            Experimental::Config => "config",
            Experimental::Close => "close",
        }
    }
}

/// A process-management behaviour, off unless named (ADR 42).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Feature {
    SharedSessions,
}

impl Feature {
    pub const ALL: [Feature; 1] = [Feature::SharedSessions];

    pub fn name(self) -> &'static str {
        match self {
            Feature::SharedSessions => "shared_sessions",
        }
    }
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

#[derive(Clone, Deserialize, Serialize)]
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
        Err(problems) => return Err(format!("{}: {}", path.display(), problems.join("; "))),
    };
    match name {
        Some(name) => profiles
            .get(name)
            .cloned()
            .ok_or_else(|| format!("no profile {name:?} in {}", path.display())),
        None => Ok(profiles.get("default").cloned().unwrap_or_default()),
    }
}

/// Every profile, or `None` if there is no config file. Otherwise what's
/// wrong with it, each problem saying where.
pub fn load_all() -> Result<Option<BTreeMap<String, Profile>>, Vec<String>> {
    let path = paths::config_file();
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(vec![err.to_string()]),
    };
    let at_line = |e: toml::de::Error| match e.span() {
        Some(span) => {
            format!("line {}: {}", text[..span.start].matches('\n').count() + 1, e.message())
        }
        None => e.message().to_owned(),
    };
    let table: toml::Table = toml::from_str(&text).map_err(|e| vec![at_line(e)])?;
    let problems = check(&table);
    if !problems.is_empty() {
        return Err(problems);
    }
    let file: ConfigFile = toml::from_str(&text).map_err(|e| vec![at_line(e)])?;
    Ok(Some(file.profiles))
}

// ---- the layout --------------------------------------------------------

/// The parts of a profile, and what goes in each.
#[derive(Clone, Copy, PartialEq)]
enum Part {
    Shared,
    Headless,
    Editor,
}

const SHARED_KEYS: &[&str] = &["agent", "log", "strict", "bridges"];
const HEADLESS_KEYS: &[&str] = &[
    "cwd",
    "mode",
    "model",
    "thought_level",
    "options",
    "permission_timeout",
    "stop_when_idle",
    "auth",
    "mcp_servers",
];
const EDITOR_KEYS: &[&str] = &["experimental", "features"];

impl Part {
    fn of(key: &str) -> Option<Part> {
        [(Part::Shared, SHARED_KEYS), (Part::Headless, HEADLESS_KEYS), (Part::Editor, EDITOR_KEYS)]
            .into_iter()
            .find(|(_, keys)| keys.contains(&key))
            .map(|(part, _)| part)
    }

    /// Its table's name, for the profile at `at` (`profiles.<name>`).
    fn table(self, at: &str) -> String {
        match self {
            Part::Shared => format!("[{at}]"),
            Part::Headless => format!("[{at}.headless]"),
            Part::Editor => format!("[{at}.editor]"),
        }
    }

    fn used_by(self) -> &'static str {
        match self {
            Part::Shared => "for every process of the profile",
            Part::Headless => "for brnr session new and resume only",
            Part::Editor => "for brnr acp only",
        }
    }
}

/// What the typed parse can't say well: keys in the wrong part (the flat
/// layout of before among them), unknown keys and names, and `log`'s values.
/// Types are left to it.
fn check(file: &toml::Table) -> Vec<String> {
    let mut problems = Vec::new();
    for (key, value) in file {
        if key != "profiles" {
            problems.push(format!("unknown key {key} (the config has only [profiles.<name>])"));
            continue;
        }
        let Some(profiles) = value.as_table() else {
            problems.push("profiles is not a table".into());
            continue;
        };
        for (name, profile) in profiles {
            let at = format!("profiles.{name}");
            match profile.as_table() {
                Some(profile) => check_part(&at, Part::Shared, profile, &mut problems),
                None => problems.push(format!("{at} is not a table")),
            }
        }
    }
    problems
}

fn check_part(at: &str, part: Part, table: &toml::Table, problems: &mut Vec<String>) {
    let here = match part {
        Part::Shared => at.to_owned(),
        Part::Headless => format!("{at}.headless"),
        Part::Editor => format!("{at}.editor"),
    };
    for (key, value) in table {
        let sub = match key.as_str() {
            "headless" => Some(Part::Headless),
            "editor" => Some(Part::Editor),
            _ => None,
        };
        if let Some(sub) = sub.filter(|_| part == Part::Shared) {
            match value.as_table() {
                Some(table) => check_part(at, sub, table, problems),
                None => problems.push(format!("{here}.{key} is not a table")),
            }
            continue;
        }
        let Some(belongs) = Part::of(key) else {
            let mut keys = match part {
                Part::Shared => SHARED_KEYS.to_vec(),
                Part::Headless => HEADLESS_KEYS.to_vec(),
                Part::Editor => EDITOR_KEYS.to_vec(),
            };
            if part == Part::Shared {
                keys.extend(["headless", "editor"]);
            }
            problems.push(format!("{here}: unknown key {key} (keys: {})", keys.join(", ")));
            continue;
        };
        if belongs != part {
            problems.push(format!(
                "{here}: {key} is {}; it goes under {}",
                belongs.used_by(),
                belongs.table(at)
            ));
            continue;
        }
        match key.as_str() {
            "log"
                if !matches!(value, toml::Value::Boolean(false))
                    && !matches!(value.as_str(), Some("all" | "events")) =>
            {
                problems.push(format!(r#"{here}: log is "all", "events" or false, not {value}"#));
            }
            "experimental" => {
                let names: Vec<&str> = Experimental::ALL.iter().map(|e| e.name()).collect();
                check_names(&format!("{here}.experimental"), "action", &names, value, problems);
            }
            "features" => {
                let names: Vec<&str> = Feature::ALL.iter().map(|f| f.name()).collect();
                check_names(&format!("{here}.features"), "feature", &names, value, problems);
            }
            _ => {}
        }
    }
}

/// A list of names, each one of `known`. A value that isn't a list of
/// strings is left to the typed parse.
fn check_names(
    at: &str,
    what: &str,
    known: &[&str],
    value: &toml::Value,
    problems: &mut Vec<String>,
) {
    for name in value.as_array().into_iter().flatten().filter_map(toml::Value::as_str) {
        if !known.contains(&name) {
            problems.push(format!("{at}: unknown {what} {name:?} ({what}s: {})", known.join(", ")));
        }
    }
}
