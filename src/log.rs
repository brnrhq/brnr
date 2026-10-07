//! Transcripts: one JSONL file per ACP session plus one per host for what
//! belongs to no session (see paths.rs for where they go). Records are
//! written by a background thread so logging can never slow down or break
//! forwarding.
//!
//! Every record starts with the fields that join files together:
//! `{"ts","host_id","host_pid","proxy_pid","agent_pid"[,"session_id"],…}`,
//! where `proxy_pid` is null for a headless process. ACP traffic adds
//! `"dir"` and `"msg"`: ACP frames messages as newline-delimited JSON, so
//! each line is embedded unchanged, and a line that isn't valid JSON is kept
//! as a string in `"raw"` instead. Host events add `"event":{…}`.
//!
//! A session file starts with a `session-opened` event naming the host log;
//! the host log records a `session-opened` event naming each session file.

use std::collections::HashMap;
use std::fs::{DirBuilder, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::paths;

#[derive(Clone, Copy)]
pub enum Dir {
    EditorToAgent,
    AgentToEditor,
    /// Something the host sent the agent: an injected prompt, a cancel, or
    /// an answer it gave as the client.
    ControlToAgent,
    /// An injected message shown to the editor, as a completed tool call (see
    /// 48 in the decision log).
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
    Note { session: Option<String>, ts: SystemTime, event: Value },
    Finish,
}

/// Where the host hands its records. A no-op when not logging.
#[derive(Clone)]
pub struct Sink(Option<Sender<Cmd>>);

impl Sink {
    /// Starts `session`'s file (appending if it exists) under `cwd`'s
    /// project folder. Records for a session not opened go to the host log.
    pub fn open_session(&self, session: &str, cwd: &Path) {
        self.send(Cmd::Open { session: session.to_owned(), cwd: cwd.to_owned() });
    }

    pub fn msg(&self, session: Option<&str>, dir: Dir, bytes: &[u8]) {
        let session = session.map(str::to_owned);
        self.send(Cmd::Msg { session, ts: SystemTime::now(), dir, bytes: bytes.to_vec() });
    }

    pub fn note(&self, session: Option<&str>, event: Value) {
        let session = session.map(str::to_owned);
        self.send(Cmd::Note { session, ts: SystemTime::now(), event });
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
    /// the log directory fails early.
    pub fn start(ids: Ids, proxy_pid: Option<u32>) -> io::Result<Logger> {
        let path = paths::host_log(&ids.host_id);
        let host = open_append(&path)?;
        let (tx, rx) = mpsc::channel();
        let writer =
            Writer { ids, proxy_pid, host, host_path: path.clone(), sessions: HashMap::new() };
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
    host: File,
    host_path: PathBuf,
    sessions: HashMap<String, File>,
}

impl Writer {
    fn run(mut self, rx: Receiver<Cmd>) {
        for cmd in rx {
            match cmd {
                Cmd::Open { session, cwd } => self.open(session, &cwd),
                Cmd::Msg { session, ts, dir, bytes } => {
                    let mut record = self.header(ts, session.as_deref());
                    record.extend_from_slice(format!(r#""dir":"{}","#, dir.name()).as_bytes());
                    embed(&mut record, bytes.strip_suffix(b"\n").unwrap_or(&bytes));
                    self.write(session.as_deref(), record);
                }
                Cmd::Note { session, ts, event } => {
                    let mut record = self.header(ts, session.as_deref());
                    record.extend_from_slice(format!("\"event\":{event}}}\n").as_bytes());
                    self.write(session.as_deref(), record);
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
        match open_append(&path) {
            Ok(file) => {
                self.sessions.insert(session.clone(), file);
                let event = json!({
                    "event": "session-opened",
                    "cwd": cwd.to_string_lossy(),
                    "host_log": self.host_path.to_string_lossy(),
                });
                self.note(now, Some(&session), event);
                let event = json!({
                    "event": "session-opened",
                    "session_id": session,
                    "file": path.to_string_lossy(),
                });
                self.note(now, None, event);
            }
            Err(err) => {
                let event = json!({
                    "event": "session-log-failed",
                    "session_id": session,
                    "file": path.to_string_lossy(),
                    "error": err.to_string(),
                });
                self.note(now, None, event);
            }
        }
    }

    fn note(&mut self, ts: SystemTime, session: Option<&str>, event: Value) {
        let mut record = self.header(ts, session);
        record.extend_from_slice(format!("\"event\":{event}}}\n").as_bytes());
        self.write(session, record);
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

    /// Into the session's file, or the host log if it has none. A failed
    /// write is dropped: the host has no stderr to report it on.
    fn write(&mut self, session: Option<&str>, record: Vec<u8>) {
        let file = match session.and_then(|s| self.sessions.get_mut(s)) {
            Some(file) => file,
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
