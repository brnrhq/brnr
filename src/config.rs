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
//! A routing profile instead contains only a `remote` table naming an SSH
//! destination and optional brnr-specific overrides (ADR 12.2).
//! Without `--profile`, the `default` profile applies if there is one. A key
//! in the wrong part, an unknown key or name, and the flat layout of before
//! fail to load, saying which and where (P7, P9).
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
    #[serde(default)]
    profiles: BTreeMap<String, Profile>,
}

/// SSH destinations and remote paths (ADR 12.2). Authentication stays in SSH.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Remote {
    pub(crate) target: String,
    pub(crate) profile: Option<String>,
    #[serde(default = "remote_executable")]
    pub(crate) executable: String,
    pub(crate) cwd: Option<String>,
    pub(crate) runtime_dir: Option<String>,
    pub(crate) home: Option<String>,
    pub(crate) config: Option<String>,
}

fn remote_executable() -> String {
    "brnr".into()
}

impl Remote {
    /// A direct SSH destination uses brnr on PATH and remote defaults.
    pub(crate) fn new(target: &str) -> Self {
        Self {
            target: target.into(),
            profile: None,
            executable: remote_executable(),
            cwd: None,
            runtime_dir: None,
            home: None,
            config: None,
        }
    }
}

#[derive(Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Profile {
    pub(crate) remote: Option<Remote>,
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

/// The named profile, or `default` (or nothing) when no name is given.
/// A file that doesn't load is `config_invalid`; a profile it doesn't have,
/// `profile_not_found` (ADR 8.7).
pub(crate) fn load(name: Option<&str>) -> Result<Profile, Error> {
    let path = paths::config_file();
    let invalid = |e: String| Error::new(Code::config_invalid, format!("{}: {e}", path.display()));
    let profiles = match load_all() {
        Ok(Some(profiles)) => profiles,
        Ok(None) if name.is_none() => return Ok(Profile::default()),
        Ok(None) => return Err(invalid("no such file".into())),
        Err(problems) => return Err(invalid(problems.join("; "))),
    };
    match name {
        Some(name) => profiles.get(name).cloned().ok_or_else(|| {
            let error = format!("no profile {name:?} in {}", path.display());
            Error::new(Code::profile_not_found, error)
        }),
        None => Ok(profiles.get("default").cloned().unwrap_or_default()),
    }
}

/// Every profile, or `None` if there is no config file. Otherwise what's
/// wrong with it, each problem saying where.
pub(crate) fn load_all() -> Result<Option<BTreeMap<String, Profile>>, Vec<String>> {
    Ok(load_file()?.map(|file| file.profiles))
}

/// The complete config, including routing settings, with the same private-file checks.
fn load_file() -> Result<Option<ConfigFile>, Vec<String>> {
    let path = paths::config_file();
    let Some(text) = read_private(&path)? else { return Ok(None) };
    parse_file(&text).map(Some)
}

fn parse_file(text: &str) -> Result<ConfigFile, Vec<String>> {
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
    for (name, profile) in &file.profiles {
        if let Some(remote) = &profile.remote {
            crate::remote::validate(remote)
                .map_err(|e| vec![format!("profiles.{name}.remote: {e}")])?;
            if remote.profile.as_ref().is_some_and(|p| p.is_empty() || p.contains('\0')) {
                return Err(vec![format!(
                    "profiles.{name}.remote.profile is empty or contains NUL"
                )]);
            }
        }
    }
    Ok(file)
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
    "response_timeout",
    "stop_when_idle",
    "auth",
    "mcp_servers",
];
const EDITOR_KEYS: &[&str] = &["experimental", "client_capabilities", "echo_as", "features"];

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
/// layout of before among them), unknown keys and names, `log`'s values and
/// a bridge's `command` given as a list.
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
    if part == Part::Shared && table.contains_key("remote") && table.len() != 1 {
        problems.push(format!("{at}: a remote profile contains only [{}.remote]; agent settings belong on the remote host", at));
    }
    let here = match part {
        Part::Shared => at.to_owned(),
        Part::Headless => format!("{at}.headless"),
        Part::Editor => format!("{at}.editor"),
    };
    for (key, value) in table {
        if key == "remote" && part == Part::Shared {
            continue;
        }
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
            "log" if !matches!(value.as_str(), Some("all" | "events" | "off")) => {
                problems.push(format!(r#"{here}: log is "all", "events" or "off", not {value}"#));
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
                let names: Vec<&str> = Experimental::ALL.iter().map(|e| e.name()).collect();
                let at = format!("{here}.experimental");
                check_names(&at, ("action", "actions"), &names, value, problems);
            }
            "client_capabilities" => {
                let names: Vec<&str> = ClientCapability::ALL.iter().map(|c| c.name()).collect();
                let at = format!("{here}.client_capabilities");
                check_names(&at, ("capability", "capabilities"), &names, value, problems);
            }
            "echo_as" => {
                if !EchoAs::ALL.iter().any(|e| value.as_str() == Some(e.name())) {
                    let names: Vec<String> =
                        EchoAs::ALL.iter().map(|e| format!("{:?}", e.name())).collect();
                    problems
                        .push(format!("{here}: echo_as is {}, not {value}", names.join(" or ")));
                }
            }
            "features" => {
                let names: Vec<&str> = Feature::ALL.iter().map(|f| f.name()).collect();
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
