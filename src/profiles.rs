//! Profiles from `config.toml` (see paths.rs for where it lives). A profile
//! has three execution parts (ADR 6.1 in docs/adr): at the top what every process of
//! the profile uses, under `headless` what only `brnr session new` and
//! `resume` use, and under `editor` what only `brnr acp` uses.
//!
//! ```toml
//! [profiles.work]                     # every process of the profile
//! agent = ["brnr-claude-adapter"]     # used when no agent is given after --
//! log = "all"                         # transcripts under ~/.brnr; "events", or "off"
//! strict = false                      # stable ACP only (ADR 2.1)
//!
//! [[profiles.work.bridges]]
//! command = "~/bin/slack-bridge"      # a program and its arguments, as an MCP server's
//! args = ["--channel", "#agents"]
//! events = ["permission_request", "turn_ended"]   # omit for every event but acp
//!
//! [profiles.work.headless]            # brnr session new and resume
//! cwd = "~/work/project"
//! mode = "plan"                       # the agent's mode, model, thought
//! model = "opus"                      #   level and config options by id,
//! thought_level = "high"              #   set before the first prompt
//! options = { effort = "high" }
//! response_timeout = 600              # seconds until an unanswered approval is rejected,
//!                                     #   or an elicitation cancelled (ADR 4.5)
//! stop_when_idle = 600                # close a session idle this many seconds
//! auth = "api-key"                    # the agent's login method, run first (ADR 4.7)
//!
//! [[profiles.work.headless.mcp_servers]]
//! name = "github"
//! command = "github-mcp-server"       # stdio; or url = "https://…" (type = "http" | "sse")
//! args = ["stdio"]
//! env = { GITHUB_TOKEN = "…" }
//!
//! [profiles.work.editor]              # brnr acp
//! experimental = ["send", "permission"] # actions on the editor's session (ADR 7.1)
//! client_capabilities = ["fs"]        # the editor's, passed to the agent: "fs", "terminal" (ADR 2.7)
//! echo_as = "tool_call"               # or "user_message_chunk" (ADR 7.2)
//! features = ["shared_sessions"]      # process management (ADR 3.10)
//! ```
//!
//! Beside the profiles, at the top of the file, what every process uses
//! whatever its profile (ADR 9.5):
//!
//! ```toml
//! retention = 2592000                 # seconds: prune what was last active longer ago
//! ```
//!
//! Without `--profile`, the `default` profile applies if there is one. A key
//! in the wrong part, an unknown key or name, and the flat layout of before
//! fail to load, saying which and where (P7, P9). [`KEYS`] is every key, the
//! one table that the layout check, `brnr profile show` and `brnr profile
//! keys` read (ADR 6.5). A profile holds no routing: `--remote` is the one
//! transport (ADR 12.2).
//!
//! The file names the commands brnr runs and may hold secrets, so it is read
//! only if it is the user's own and nobody else can read or write it, nor
//! write its folder (ADR 9.4): every command reads it through `load_all`.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::CString;
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Read};
use std::os::fd::AsFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::error::{Code, Error};
use crate::{paths, sys};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigFile {
    retention: Option<u64>,
    #[serde(default)]
    profiles: BTreeMap<String, Profile>,
}

/// The config file, loaded.
pub(crate) struct Config {
    /// How long, in seconds, transcripts and host logs are kept after
    /// their last activity: every process prunes what is older as it
    /// starts (ADR 9.5). None keeps them.
    pub(crate) retention: Option<u64>,
    pub(crate) profiles: BTreeMap<String, Profile>,
    /// Each profile's table as the file has it, for `brnr profile show`
    /// (ADR 6.5).
    pub(crate) tables: BTreeMap<String, toml::Table>,
}

#[derive(Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Profile {
    pub(crate) agent: Option<Vec<String>>,
    #[serde(default)]
    pub(crate) log: Log,
    #[serde(default)]
    pub(crate) strict: bool,
    #[serde(default)]
    pub(crate) bridges: Vec<Bridge>,
    #[serde(default)]
    pub(crate) headless: Headless,
    #[serde(default)]
    pub(crate) editor: Editor,
}

/// What only `brnr session new` and `resume` use (ADRs 3.8, 4.5, 6.2, 4.7 and 6.3,
/// 63).
#[derive(Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Headless {
    pub(crate) cwd: Option<String>,
    pub(crate) mode: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) thought_level: Option<String>,
    #[serde(default)]
    pub(crate) options: BTreeMap<String, String>,
    pub(crate) response_timeout: Option<u64>,
    pub(crate) stop_when_idle: Option<u64>,
    pub(crate) auth: Option<String>,
    #[serde(default)]
    pub(crate) mcp_servers: Vec<McpServer>,
}

/// What only `brnr acp` uses (ADRs 7.1, 3.10, 2.7 and 7.2).
#[derive(Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Editor {
    #[serde(default)]
    pub(crate) experimental: BTreeSet<Experimental>,
    #[serde(default)]
    pub(crate) client_capabilities: BTreeSet<ClientCapability>,
    #[serde(default)]
    pub(crate) echo_as: EchoAs,
    #[serde(default)]
    pub(crate) features: BTreeSet<Feature>,
}

/// What the host records (ADR 5.1): `"all"`, `"events"` (no raw ACP) or
/// `"off"` (nothing). Strings only (ADR 6.1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Log {
    #[default]
    All,
    Events,
    Off,
}

/// An action on an editor's session through the side channel, enabled by
/// name (ADR 7.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Experimental {
    Send,
    Context,
    Cancel,
    Permission,
    Elicitation,
    Config,
    Close,
}

impl Experimental {
    pub(crate) const ALL: [Experimental; 7] = [
        Experimental::Send,
        Experimental::Context,
        Experimental::Cancel,
        Experimental::Permission,
        Experimental::Elicitation,
        Experimental::Config,
        Experimental::Close,
    ];

    pub(crate) fn name(self) -> &'static str {
        match self {
            Experimental::Send => "send",
            Experimental::Context => "context",
            Experimental::Cancel => "cancel",
            Experimental::Permission => "permission",
            Experimental::Elicitation => "elicitation",
            Experimental::Config => "config",
            Experimental::Close => "close",
        }
    }
}

/// A client capability of the editor's that ACP v2 drops, and `brnr acp`
/// drops too unless the profile names it, or in strict mode (ADR 2.7).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ClientCapability {
    Fs,
    Terminal,
}

impl ClientCapability {
    pub(crate) const ALL: [ClientCapability; 2] =
        [ClientCapability::Fs, ClientCapability::Terminal];

    /// Its name, as the editor's `initialize` and the profile have it.
    pub(crate) fn name(self) -> &'static str {
        match self {
            ClientCapability::Fs => "fs",
            ClientCapability::Terminal => "terminal",
        }
    }
}

/// The `session/update` a message sent to an editor's session through the
/// side channel is shown to it as (ADR 7.2).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum EchoAs {
    /// A completed tool call, "Message via brnr": rendered by every editor.
    #[default]
    ToolCall,
    /// The message's own `user_message_chunk`s, for an editor that renders
    /// them out of turn.
    UserMessageChunk,
}

impl EchoAs {
    pub(crate) const ALL: [EchoAs; 2] = [EchoAs::ToolCall, EchoAs::UserMessageChunk];

    pub(crate) fn name(self) -> &'static str {
        match self {
            EchoAs::ToolCall => "tool_call",
            EchoAs::UserMessageChunk => "user_message_chunk",
        }
    }
}

/// A process-management behaviour, off unless named (ADR 3.10).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Feature {
    SharedSessions,
}

impl Feature {
    pub(crate) const ALL: [Feature; 1] = [Feature::SharedSessions];

    pub(crate) fn name(self) -> &'static str {
        match self {
            Feature::SharedSessions => "shared_sessions",
        }
    }
}

/// An MCP server for sessions the host opens: stdio (`command`) or remote
/// (`url`, with `type` http or sse).
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct McpServer {
    pub(crate) name: String,
    pub(crate) command: Option<String>,
    #[serde(default)]
    pub(crate) args: Vec<String>,
    #[serde(default)]
    pub(crate) env: BTreeMap<String, String>,
    pub(crate) url: Option<String>,
    #[serde(rename = "type")]
    pub(crate) kind: Option<String>,
    #[serde(default)]
    pub(crate) headers: BTreeMap<String, String>,
}

impl McpServer {
    /// As ACP's `McpServer`.
    pub(crate) fn to_acp(&self) -> Result<Value, String> {
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

/// A process the host starts and speaks JSON lines with (ADR 5.7): a
/// program and its arguments, as an MCP server's are (ADR 6.1).
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Bridge {
    pub(crate) command: String,
    #[serde(default)]
    pub(crate) args: Vec<String>,
    pub(crate) events: Option<Vec<String>>,
}

/// The named profile, or `default` (or nothing) when no name is given, and
/// the config's `retention` (ADR 9.5). A file that doesn't load is
/// `config_invalid`; a profile it doesn't have, `profile_not_found` (ADR
/// 8.7).
pub(crate) fn load(name: Option<&str>) -> Result<(Profile, Option<u64>), Error> {
    let Some(config) = config_for(name)? else { return Ok((Profile::default(), None)) };
    let profile = match name {
        Some(name) => config.profiles.get(name).cloned().ok_or_else(|| not_found(name))?,
        None => config.profiles.get("default").cloned().unwrap_or_default(),
    };
    Ok((profile, config.retention))
}

/// The profile [`load`] loads, as the file has it (ADR 6.5): its name
/// (`default` when none is given), whether the file has it, and its table,
/// empty if not. It fails where `load` does.
pub(crate) fn load_table(name: Option<&str>) -> Result<(String, bool, toml::Table), Error> {
    let shown = name.unwrap_or("default").to_owned();
    let Some(config) = config_for(name)? else { return Ok((shown, false, toml::Table::new())) };
    match config.tables.get(&shown) {
        Some(table) => Ok((shown, true, table.clone())),
        None if name.is_none() => Ok((shown, false, toml::Table::new())),
        None => Err(not_found(&shown)),
    }
}

/// The config, for a command that names `name` (or none): `None` if there
/// is no file and none is named.
fn config_for(name: Option<&str>) -> Result<Option<Config>, Error> {
    let path = paths::config_file();
    let invalid = |e: String| Error::new(Code::config_invalid, format!("{}: {e}", path.display()));
    match load_all() {
        Ok(Some(config)) => Ok(Some(config)),
        Ok(None) if name.is_none() => Ok(None),
        Ok(None) => Err(invalid("no such file".into())),
        Err(problems) => Err(invalid(problems.join("; "))),
    }
}

fn not_found(name: &str) -> Error {
    let error = format!("no profile {name:?} in {}", paths::config_file().display());
    Error::new(Code::profile_not_found, error)
}

/// The config, or `None` if there is no config file. Otherwise what's
/// wrong with it, each problem saying where.
pub(crate) fn load_all() -> Result<Option<Config>, Vec<String>> {
    let path = paths::config_file();
    let Some(text) = read_private(&path)? else { return Ok(None) };
    parse_file(&text).map(Some)
}

fn parse_file(text: &str) -> Result<Config, Vec<String>> {
    let at_line = |e: toml::de::Error| match e.span() {
        Some(span) => {
            format!("line {}: {}", text[..span.start].matches('\n').count() + 1, e.message())
        }
        None => e.message().to_owned(),
    };
    let table: toml::Table = toml::from_str(text).map_err(|e| vec![at_line(e)])?;
    let problems = check(&table);
    if !problems.is_empty() {
        return Err(problems);
    }
    let file: ConfigFile = toml::from_str(text).map_err(|e| vec![at_line(e)])?;
    let tables = match table.get("profiles").and_then(toml::Value::as_table) {
        Some(profiles) => profiles
            .iter()
            .filter_map(|(name, t)| Some((name.clone(), t.as_table()?.clone())))
            .collect(),
        None => BTreeMap::new(),
    };
    Ok(Config { retention: file.retention, profiles: file.profiles, tables })
}

// ---- who can reach it ---------------------------------------------------

/// The text of the config file at `path`, or `None` if there is none, read
/// only if it is the user's own and nobody else can read or write it, and
/// its folder is the user's own and nobody else can write it (ADR 9.4): it
/// names the commands brnr runs, and may hold MCP servers' secrets. A
/// symlink is followed, and what is checked is the file it resolves to and
/// that file's folder, through the descriptors the file is read from. brnr
/// changes neither (P4): each problem says what is wrong, and how to fix it.
fn read_private(path: &Path) -> Result<Option<String>, Vec<String>> {
    let real = match fs::canonicalize(path) {
        Ok(real) => real,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(vec![err.to_string()]),
    };
    let (Some(dir), Some(name)) = (real.parent(), real.file_name()) else {
        return Err(vec![format!("{} is not a file", real.display())]);
    };
    let flags = libc::O_RDONLY | libc::O_DIRECTORY;
    let folder = OpenOptions::new().read(true).custom_flags(flags).open(dir).map_err(at(dir))?;
    let name = CString::new(name.as_bytes()).map_err(|e| at(&real)(e.into()))?;
    // Not blocked by a FIFO: a regular file ignores O_NONBLOCK.
    let flags = libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK;
    let mut file = match sys::openat(folder.as_fd(), &name, flags, 0) {
        Ok(file) => file,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(at(&real)(err)),
    };
    let linked = fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink());
    let it = if linked { format!("{}, which it links to,", real.display()) } else { "it".into() };
    let mut problems = Vec::new();
    let shown = if linked { real.as_path() } else { path };
    check_private(&file, &it, shown, false, &mut problems).map_err(at(shown))?;
    let its_folder = format!("its folder {}", dir.display());
    check_private(&folder, &its_folder, dir, true, &mut problems).map_err(at(dir))?;
    if !problems.is_empty() {
        return Err(problems);
    }
    let mut text = String::new();
    file.read_to_string(&mut text).map_err(at(&real))?;
    Ok(Some(text))
}

/// An error reading at `path`, saying where.
fn at(path: &Path) -> impl Fn(std::io::Error) -> Vec<String> + '_ {
    move |err| vec![format!("{}: {err}", path.display())]
}

/// What is wrong with `opened`, the config file or its folder (`dir`) at
/// `path`, called `what`: another user's, or others can write it, or (the
/// file) read it. Each problem with the mode or owner it has, and the
/// command that fixes it.
fn check_private(
    opened: &File,
    what: &str,
    path: &Path,
    dir: bool,
    problems: &mut Vec<String>,
) -> std::io::Result<()> {
    let meta = opened.metadata()?;
    let (me, mode) = (sys::uid(), meta.mode() & 0o777);
    let shell = paths::shell_word(&path.to_string_lossy());
    let refused = "so brnr won't read the config";
    if !dir && !meta.is_file() {
        problems.push(format!("{what} is not a regular file, {refused}"));
    }
    if meta.uid() != me {
        let fix = format!("sudo chown {me} {shell}");
        problems.push(format!(
            "{what} is owned by uid {}, not you (uid {me}), {refused}: {fix}",
            meta.uid()
        ));
    }
    let open = mode & if dir { 0o022 } else { 0o066 };
    if open != 0 {
        let who = match (open & 0o070 != 0, open & 0o007 != 0) {
            (true, true) => "the group and others",
            (true, false) => "the group",
            _ => "others",
        };
        let (read, written) = (open & 0o044 != 0, open & 0o022 != 0);
        let can = match (read, written) {
            (true, true) => "read and written",
            (true, false) => "read",
            _ => "written",
        };
        let secrets = if read { ", and may hold secrets" } else { "" };
        let fix = if dir { "go-w" } else { "go-rw" };
        problems.push(format!(
            "{what} can be {can} by {who} (mode {mode:o}){secrets}, {refused}: chmod {fix} {shell}"
        ));
    }
    // macOS: an ACL can let others in whatever the mode says.
    if sys::has_acl(opened)? {
        let can = if dir { "change what is in it" } else { "read or write it" };
        problems.push(format!(
            "{what} has an access control list (ls -le), which can let others {can} \
             whatever its mode, {refused}: chmod -N {shell}"
        ));
    }
    Ok(())
}

// ---- the layout --------------------------------------------------------

/// The parts of a profile, each a table of its own, by who uses it
/// (ADR 6.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Part {
    Shared,
    Headless,
    Editor,
}

impl Part {
    pub(crate) const ALL: [Part; 3] = [Part::Shared, Part::Headless, Part::Editor];

    fn of(key: &str) -> Option<Part> {
        KEYS.iter().find(|k| k.name == key).map(|k| k.part)
    }

    /// Its name, as `profile keys` has it.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Part::Shared => "shared",
            Part::Headless => "headless",
            Part::Editor => "editor",
        }
    }

    /// Its table's key in the profile, none for the profile's own.
    pub(crate) fn key(self) -> Option<&'static str> {
        match self {
            Part::Shared => None,
            Part::Headless => Some("headless"),
            Part::Editor => Some("editor"),
        }
    }

    /// Its table's name, for the profile at `at` (`profiles.<name>`).
    pub(crate) fn table(self, at: &str) -> String {
        match self.key() {
            None => format!("[{at}]"),
            Some(key) => format!("[{at}.{key}]"),
        }
    }

    /// Who uses it.
    pub(crate) fn users(self) -> &'static str {
        match self {
            Part::Shared => "every process of the profile",
            Part::Headless => "brnr session new and session resume",
            Part::Editor => "brnr acp",
        }
    }

    fn used_by(self) -> &'static str {
        match self {
            Part::Shared => "for every process of the profile",
            Part::Headless => "for brnr session new and resume only",
            Part::Editor => "for brnr acp only",
        }
    }

    /// Its keys, in the order the file's layout has them.
    pub(crate) fn keys(self) -> impl Iterator<Item = &'static Key> {
        KEYS.iter().filter(move |k| k.part == self)
    }
}

/// A key a profile can have: the one table that the layout check, `profile
/// show` and `profile keys` read, so they can't disagree (ADR 6.5).
pub(crate) struct Key {
    pub(crate) part: Part,
    pub(crate) name: &'static str,
    /// Its TOML type.
    pub(crate) kind: &'static str,
    /// Its value when the file doesn't set it, as TOML; `None`: [`Key::rule`]
    /// applies instead.
    pub(crate) default: Option<&'static str>,
    /// What applies when the file doesn't set it and it has no default.
    pub(crate) rule: Option<&'static str>,
    /// The ADR section that decides what it does.
    pub(crate) adr: &'static str,
}

const fn key(part: Part, name: &'static str, kind: &'static str, adr: &'static str) -> Key {
    Key { part, name, kind, default: None, rule: None, adr }
}

impl Key {
    const fn default(self, value: &'static str) -> Key {
        Key { default: Some(value), ..self }
    }
    const fn rule(self, rule: &'static str) -> Key {
        Key { rule: Some(rule), ..self }
    }

    /// The names it accepts, for a key that takes names: its values, or the
    /// items of its list.
    pub(crate) fn names(&self) -> Vec<&'static str> {
        match self.name {
            "log" => vec!["all", "events", "off"],
            "experimental" => Experimental::ALL.iter().map(|e| e.name()).collect(),
            "client_capabilities" => ClientCapability::ALL.iter().map(|c| c.name()).collect(),
            "echo_as" => EchoAs::ALL.iter().map(|e| e.name()).collect(),
            "features" => Feature::ALL.iter().map(|f| f.name()).collect(),
            _ => Vec::new(),
        }
    }

    /// Its default, as a value.
    pub(crate) fn default_value(&self) -> Option<toml::Value> {
        let table: toml::Table = toml::from_str(&format!("v = {}", self.default?)).ok()?;
        table.get("v").cloned()
    }
}

/// Every key, by part, as the file's layout has them.
pub(crate) const KEYS: &[Key] = &[
    key(Part::Shared, "agent", "list of strings", "6.1").rule("the agent given after --"),
    key(Part::Shared, "log", "string", "5.1").default(r#""all""#),
    key(Part::Shared, "strict", "boolean", "2.1").default("false"),
    key(Part::Shared, "bridges", "array of tables: command, args, events", "5.7").default("[]"),
    key(Part::Headless, "cwd", "string", "6.1").rule("--cwd, else where brnr session new runs"),
    key(Part::Headless, "mode", "string", "6.2").rule("--mode, else the agent's"),
    key(Part::Headless, "model", "string", "6.2").rule("--model, else the agent's"),
    key(Part::Headless, "thought_level", "string", "6.2").rule("--thought-level, else the agent's"),
    key(Part::Headless, "options", "table of strings", "6.2").default("{}"),
    key(Part::Headless, "response_timeout", "integer", "4.5")
        .rule("--response-timeout, else a request waits as long as the process runs"),
    key(Part::Headless, "stop_when_idle", "integer", "3.8")
        .rule("--stop-when-idle, else an idle session isn't closed"),
    key(Part::Headless, "auth", "string", "4.7").rule("--auth, else no login method is run"),
    key(
        Part::Headless,
        "mcp_servers",
        "array of tables: name, command, args, env, url, type, headers",
        "6.3",
    )
    .default("[]"),
    key(Part::Editor, "experimental", "list of strings", "7.1").default("[]"),
    key(Part::Editor, "client_capabilities", "list of strings", "2.7").default("[]"),
    key(Part::Editor, "echo_as", "string", "7.2").default(r#""tool_call""#),
    key(Part::Editor, "features", "list of strings", "3.10").default("[]"),
];

/// What the typed parse can't say well: keys in the wrong part (the flat
/// layout of before among them), unknown keys and names, `log`'s values, a
/// bridge's `command` given as a list, and a `retention` that isn't a
/// number of seconds, or is 0. Other types are left to it.
fn check(file: &toml::Table) -> Vec<String> {
    let mut problems = Vec::new();
    for (key, value) in file {
        if key == "retention" {
            problems.extend(check_retention(value));
            continue;
        }
        if key != "profiles" {
            problems.push(format!(
                "unknown key {key} (the config has retention and [profiles.<name>])"
            ));
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

/// What is wrong with `retention`'s value: it is a whole number of seconds
/// (ADR 6.1), and not 0, which would delete every transcript as soon as its
/// session closed, and which another tool may read as "no limit" (P4).
fn check_retention(value: &toml::Value) -> Option<String> {
    match value.as_integer() {
        Some(0) => Some(
            "retention = 0 would delete every transcript once its session closed: leave \
             retention out to keep them, or set log = \"off\" in a profile to write none"
                .into(),
        ),
        Some(n) if n > 0 => None,
        _ => Some(format!("retention is a whole number of seconds, not {value}")),
    }
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
            let mut keys: Vec<&str> = part.keys().map(|k| k.name).collect();
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
        let names = KEYS.iter().find(|k| k.name == key).map(Key::names).unwrap_or_default();
        match key.as_str() {
            "log" | "echo_as" if !names.iter().any(|n| value.as_str() == Some(n)) => {
                let names: Vec<String> = names.iter().map(|n| format!("{n:?}")).collect();
                let names = match names.split_last() {
                    Some((last, [])) => last.clone(),
                    Some((last, rest)) => format!("{} or {last}", rest.join(", ")),
                    None => String::new(),
                };
                problems.push(format!("{here}: {key} is {names}, not {value}"));
            }
            "bridges" => {
                let bridges = value.as_array().into_iter().flatten();
                let listed = |b: &toml::Value| b.get("command").is_some_and(toml::Value::is_array);
                for (n, _) in bridges.enumerate().filter(|(_, b)| listed(b)) {
                    problems.push(format!(
                        "{here}.bridges[{n}]: command is the program, a string; its arguments go \
                         in args"
                    ));
                }
            }
            "experimental" => {
                let at = format!("{here}.experimental");
                check_names(&at, ("action", "actions"), &names, value, problems);
            }
            "client_capabilities" => {
                let at = format!("{here}.client_capabilities");
                check_names(&at, ("capability", "capabilities"), &names, value, problems);
            }
            "features" => {
                let at = format!("{here}.features");
                check_names(&at, ("feature", "features"), &names, value, problems);
            }
            _ => {}
        }
    }
}

/// A list of names, each one of `known`. A value that isn't a list of
/// strings is left to the typed parse.
fn check_names(
    at: &str,
    (what, whats): (&str, &str),
    known: &[&str],
    value: &toml::Value,
    problems: &mut Vec<String>,
) {
    for name in value.as_array().into_iter().flatten().filter_map(toml::Value::as_str) {
        if !known.contains(&name) {
            problems.push(format!("{at}: unknown {what} {name:?} ({whats}: {})", known.join(", ")));
        }
    }
}
