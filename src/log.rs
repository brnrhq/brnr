//! Transcripts (ADR 22 in docs/adr): two JSONL files per ACP session, its
//! events and its raw ACP, plus one per host for what belongs to no session
//! (see paths.rs for where they go). `log = "events"` leaves out the raw
//! ACP; `log = false` is no logger at all. Records are written by a
//! background thread so logging can never slow down or break forwarding.
//!
//! Every record starts with the fields that join files together:
//! `{"ts","host_id","host_pid","proxy_pid","agent_pid"[,"session_id"],…}`,
//! where `proxy_pid` is null for a headless process. ACP traffic adds
//! `"dir"` and `"msg"`: ACP frames messages as newline-delimited JSON, so
//! each line is embedded unchanged (but for the MCP servers' secrets in a
//! request that opens a session: see [`redacted`]), and a line that isn't
//! valid JSON is kept as a string in `"raw"` instead. Host events add
//! `"event":{…}`.
//!
//! A session's events file starts with a `session-opened` event naming the
//! host log and the raw file; the host log records a `session-opened` event
//! naming both of each session's files.

use std::collections::HashMap;
use std::fs::{DirBuilder, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value, json};

use crate::paths;

/// What a secret is recorded as.
const REDACTED: &str = "<redacted>";

/// Requests whose params carry MCP servers: `env` and `headers` with
/// tokens in them (ADR 25 in docs/adr).
const OPENING: &[&str] = &["session/new", "session/load", "session/resume", "session/fork"];

#[derive(Clone, Copy)]
pub enum Dir {
    EditorToAgent,
    AgentToEditor,
    /// Something the host sent the agent: an injected prompt, a cancel, or
    /// an answer it gave as the client.
    ControlToAgent,
    /// An injected message shown to the editor, as a completed tool call (see
    /// ADR 5 in docs/adr).
    ControlToEditor,
    /// An agent response meant for the host, which the editor never sees.
    AgentToControl,
    AgentStderr,
}

impl Dir {
    pub fn name(self) -> &'static str {
        match self {
            Dir::EditorToAgent => "editor->agent",
            Dir::AgentToEditor => "agent->editor",
            Dir::ControlToAgent => "control->agent",
            Dir::ControlToEditor => "control->editor",
            Dir::AgentToControl => "agent->control",
            Dir::AgentStderr => "agent-stderr",
        }
    }
}

pub struct Ids {
    pub host_id: String,
    pub host_pid: u32,
    pub agent_pid: u32,
}

enum Cmd {
    Open { session: String, cwd: PathBuf },
    Msg { session: Option<String>, ts: SystemTime, dir: Dir, bytes: Vec<u8> },
    Note { session: Option<String>, ts: SystemTime, event: String },
    Finish,
}

/// Where the host hands its records. A no-op when not logging.
#[derive(Clone)]
pub struct Sink(Option<Sender<Cmd>>);

impl Sink {
    /// Starts `session`'s files (appending if they exist) under `cwd`'s
    /// project folder. Records for a session not opened go to the host log.
    pub fn open_session(&self, session: &str, cwd: &Path) {
        self.send(Cmd::Open { session: session.to_owned(), cwd: cwd.to_owned() });
    }

    /// An ACP message: into the session's raw file (none with `log =
    /// "events"`), or the host log.
    pub fn msg(&self, session: Option<&str>, dir: Dir, bytes: &[u8]) {
        let session = session.map(str::to_owned);
        self.send(Cmd::Msg { session, ts: SystemTime::now(), dir, bytes: bytes.to_vec() });
    }

    /// An event: into the session's events file, or the host log. It is
    /// written out here, on the caller's stack: it may nest as deeply as the
    /// ACP it came from (see json.rs).
    pub fn note(&self, session: Option<&str>, event: Value) {
        if self.0.is_some() {
            let (session, event) = (session.map(str::to_owned), event.to_string());
            self.send(Cmd::Note { session, ts: SystemTime::now(), event });
        }
    }

    fn send(&self, cmd: Cmd) {
        if let Some(tx) = &self.0 {
            let _ = tx.send(cmd);
        }
    }
}

pub struct Logger {
    sink: Sink,
    host_log: Option<PathBuf>,
    thread: Option<JoinHandle<()>>,
}

impl Logger {
    pub fn disabled() -> Logger {
        Logger { sink: Sink(None), host_log: None, thread: None }
    }

    /// Creates the host log. Done before anything else, so a problem with
    /// the log directory fails early. Sessions get a raw file if `acp`.
    pub fn start(ids: Ids, proxy_pid: Option<u32>, acp: bool) -> io::Result<Logger> {
        let path = paths::host_log(&ids.host_id);
        let host = open_append(&path)?;
        let (tx, rx) = mpsc::channel();
        let host_path = path.clone();
        let writer = Writer { ids, proxy_pid, acp, host, host_path, sessions: HashMap::new() };
        let thread = thread::spawn(move || writer.run(rx));
        Ok(Logger { sink: Sink(Some(tx)), host_log: Some(path), thread: Some(thread) })
    }

    pub fn sink(&self) -> Sink {
        self.sink.clone()
    }

    pub fn host_log(&self) -> Option<&Path> {
        self.host_log.as_deref()
    }

    /// Writes everything recorded so far.
    pub fn finish(self) {
        if let (Some(tx), Some(thread)) = (&self.sink.0, self.thread) {
            let _ = tx.send(Cmd::Finish);
            let _ = thread.join();
        }
    }
}

struct Writer {
    ids: Ids,
    proxy_pid: Option<u32>,
    /// Sessions get a raw ACP file.
    acp: bool,
    host: File,
    host_path: PathBuf,
    sessions: HashMap<String, Files>,
}

/// A session's events, and its raw ACP unless `log = "events"`.
struct Files {
    events: File,
    acp: Option<File>,
}

impl Writer {
    fn run(mut self, rx: Receiver<Cmd>) {
        for cmd in rx {
            match cmd {
                Cmd::Open { session, cwd } => self.open(session, &cwd),
                Cmd::Msg { session, ts, dir, bytes } => {
                    let files = session.as_deref().and_then(|s| self.sessions.get(s));
                    if files.is_some_and(|f| f.acp.is_none()) {
                        continue; // A session's, with no raw file to go in.
                    }
                    let mut record = self.header(ts, session.as_deref());
                    record.extend_from_slice(format!(r#""dir":"{}","#, dir.name()).as_bytes());
                    embed(&mut record, bytes.strip_suffix(b"\n").unwrap_or(&bytes));
                    self.write(session.as_deref(), true, record);
                }
                Cmd::Note { session, ts, event } => {
                    let mut record = self.header(ts, session.as_deref());
                    record.extend_from_slice(format!("\"event\":{event}}}\n").as_bytes());
                    self.write(session.as_deref(), false, record);
                }
                Cmd::Finish => return,
            }
        }
    }

    fn open(&mut self, session: String, cwd: &Path) {
        if self.sessions.contains_key(&session) {
            return;
        }
        let path = paths::session_log(cwd, &session);
        let now = SystemTime::now();
        let events = match open_append(&path) {
            Ok(file) => file,
            Err(err) => return self.failed(now, &session, &path, &err),
        };
        let (mut acp, mut acp_path) = (None, None);
        if self.acp {
            let raw = paths::acp_log(&path);
            match open_append(&raw) {
                Ok(file) => {
                    (acp, acp_path) = (Some(file), Some(raw.to_string_lossy().into_owned()))
                }
                Err(err) => self.failed(now, &session, &raw, &err),
            }
        }
        self.sessions.insert(session.clone(), Files { events, acp });
        let event = json!({
            "event": "session-opened",
            "cwd": cwd.to_string_lossy(),
            "host_log": self.host_path.to_string_lossy(),
            "acp_log": acp_path,
        });
        self.note(now, Some(&session), event);
        let event = json!({
            "event": "session-opened",
            "session_id": session,
            "file": path.to_string_lossy(),
            "acp_file": acp_path,
        });
        self.note(now, None, event);
    }

    fn failed(&mut self, ts: SystemTime, session: &str, path: &Path, err: &io::Error) {
        let event = json!({
            "event": "session-log-failed",
            "session_id": session,
            "file": path.to_string_lossy(),
            "error": err.to_string(),
        });
        self.note(ts, None, event);
    }

    fn note(&mut self, ts: SystemTime, session: Option<&str>, event: Value) {
        let mut record = self.header(ts, session);
        record.extend_from_slice(format!("\"event\":{event}}}\n").as_bytes());
        self.write(session, false, record);
    }

    fn header(&self, ts: SystemTime, session: Option<&str>) -> Vec<u8> {
        let proxy = self.proxy_pid.map_or("null".to_owned(), |p| p.to_string());
        let mut out = format!(
            r#"{{"ts":"{}","host_id":"{}","host_pid":{},"proxy_pid":{},"agent_pid":{},"#,
            rfc3339(ts),
            self.ids.host_id,
            self.ids.host_pid,
            proxy,
            self.ids.agent_pid,
        );
        if let Some(session) = session {
            out.push_str(&format!(r#""session_id":{},"#, json!(session)));
        }
        out.into_bytes()
    }

    /// Into the session's events or (`acp`) raw file, or the host log if it
    /// has none open. A failed write is dropped: the host has no stderr to
    /// report it on.
    fn write(&mut self, session: Option<&str>, acp: bool, record: Vec<u8>) {
        let file = match session.and_then(|s| self.sessions.get_mut(s)) {
            Some(files) if acp => match &mut files.acp {
                Some(file) => file,
                None => return,
            },
            Some(files) => &mut files.events,
            None => &mut self.host,
        };
        let _ = file.write_all(&record);
    }
}

fn embed(out: &mut Vec<u8>, line: &[u8]) {
    if serde_json::from_slice::<Value>(line).is_ok() {
        out.extend_from_slice(b"\"msg\":");
        out.extend_from_slice(line);
    } else {
        let text = serde_json::to_string(&String::from_utf8_lossy(line)).unwrap();
        out.extend_from_slice(b"\"raw\":");
        out.extend_from_slice(text.as_bytes());
    }
    out.extend_from_slice(b"}\n");
}

/// What brnr records of an ACP message, if not the line as it came: in a
/// request that opens a session, the values of its MCP servers' `env` and
/// `headers` are `"<redacted>"` (ADR 25 in docs/adr). The keys and the
/// structure stay. Run it on a stack that takes the message.
pub fn redacted(msg: &Map<String, Value>) -> Option<Vec<u8>> {
    let method = msg.get("method")?.as_str()?;
    if !OPENING.contains(&method) {
        return None;
    }
    let mut servers = msg.get("params")?.get("mcpServers")?.clone();
    if !redact_mcp_servers(&mut servers) {
        return None;
    }
    let mut msg = msg.clone();
    msg["params"]["mcpServers"] = servers;
    serde_json::to_vec(&msg).ok()
}

/// Redacts a list of ACP `McpServer`s in place: the `value` of each of
/// their `env` and `headers` entries, or each value where they are a map.
/// Whether there was a value to redact.
pub fn redact_mcp_servers(servers: &mut Value) -> bool {
    let mut any = false;
    for server in servers.as_array_mut().into_iter().flatten() {
        for key in ["env", "headers"] {
            let values: Vec<&mut Value> = match server.get_mut(key) {
                Some(Value::Array(entries)) => {
                    entries.iter_mut().filter_map(|e| e.get_mut("value")).collect()
                }
                Some(Value::Object(map)) => map.values_mut().collect(),
                _ => continue,
            };
            for value in values {
                *value = json!(REDACTED);
                any = true;
            }
        }
    }
    any
}

/// Opens `path` for appending, creating its directory, so a session's file
/// grows across hosts that serve it. Transcripts hold prompts and tool
/// output, so what this creates is private to the user.
fn open_append(path: &Path) -> io::Result<File> {
    if let Some(dir) = path.parent() {
        DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    }
    OpenOptions::new().create(true).append(true).mode(0o600).open(path)
}

/// `20261001T171839` in UTC: the start of a host id.
pub fn compact_utc(ts: SystemTime) -> String {
    let full = rfc3339(ts);
    format!(
        "{}{}{}T{}{}{}",
        &full[0..4],
        &full[5..7],
        &full[8..10],
        &full[11..13],
        &full[14..16],
        &full[17..19]
    )
}

/// UTC timestamp with microseconds, e.g. `2026-09-30T14:50:01.123456Z`.
pub fn rfc3339(ts: SystemTime) -> String {
    let d = ts.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = d.as_secs() as i64;
    let (days, rem) = (secs.div_euclid(86400), secs.rem_euclid(86400));

    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + (month <= 2) as i64;

    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:06}Z",
        rem / 3600,
        rem / 60 % 60,
        rem % 60,
        d.subsec_micros()
    )
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn recorded(msg: Value) -> Option<Value> {
        let bytes = redacted(msg.as_object().unwrap())?;
        Some(serde_json::from_slice(&bytes).unwrap())
    }

    #[test]
    fn secrets_are_redacted_keys_and_structure_stay() {
        let env = json!([{ "name": "TOKEN", "value": "t" }]);
        let stdio = json!({ "name": "gh", "command": "gh", "args": [], "env": env });
        let headers = json!([{ "name": "Authorization", "value": "a" }]);
        let http = json!({ "type": "http", "name": "api", "url": "u", "headers": headers });
        let msg = |method: &str, servers: Value| {
            let params = json!({ "cwd": "/", "mcpServers": servers });
            json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params })
        };
        for method in OPENING {
            let shown = recorded(msg(method, json!([stdio, http]))).expect(method);
            let servers = &shown["params"]["mcpServers"];
            assert_eq!(servers[0]["env"], json!([{ "name": "TOKEN", "value": "<redacted>" }]));
            let header = json!({ "name": "Authorization", "value": "<redacted>" });
            assert_eq!(servers[1]["headers"][0], header);
            assert_eq!((&servers[0]["command"], &servers[1]["url"]), (&json!("gh"), &json!("u")));
            assert_eq!(shown["params"]["cwd"], "/");
        }
        // A map, as some send them.
        let mapped = json!([{ "name": "x", "command": "x", "env": { "TOKEN": "t" } }]);
        let shown = recorded(msg("session/new", mapped)).unwrap();
        assert_eq!(shown["params"]["mcpServers"][0]["env"], json!({ "TOKEN": "<redacted>" }));
        // Nothing to redact: the line as it came.
        assert_eq!(recorded(msg("session/new", json!([]))), None);
        assert_eq!(recorded(msg("session/prompt", json!([stdio]))), None);
    }
}
