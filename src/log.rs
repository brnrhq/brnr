//! Transcripts (ADR 22 in docs/adr): two JSONL files per ACP session, its
//! events and its raw ACP, plus one per host for what belongs to no session
//! (see paths.rs for where they go). `log = "events"` leaves out the raw
//! ACP; `log = false` is no logger at all. Records are written by a
//! background thread so logging can never slow down or break forwarding;
//! [`Sink::when_written`] says when what was recorded is written, for
//! `brnr event log` (ADR 48).
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
//! naming both of each session's files. Closing a session closes its files
//! once what was recorded for it before is written, so a process holds
//! files only for the sessions it has open; one opened again appends to
//! them, starting with another `session-opened`.
//!
//! What a record goes into, from the state directory down, is the user's own
//! and private before anything is written to it (P13, ADR 59): a file or
//! directory others can reach is made private, and noted in the host log as
//! `made-private`; a symlink, or one that isn't the user's, is refused.
//!
//! What is queued for the logger is bounded (ADR 6 in docs/adr), in bytes,
//! counted from when a record is queued until the logger has written it.
//! Past `LOG_BYTES` (a disk that is slow, or has stopped) records are
//! skipped rather than queued, and counted; the host never waits for the
//! logger (P1, P6). Never skipped, however full the queue: a session's
//! opening, which says where its records go, and `exited` and `panic`,
//! which say how the process ended, so that `brnr doctor` can tell an exit
//! from a death (ADR 11). Once the queue is down to half `LOG_BYTES`, the
//! gap is over, and a note says so where the records would have been (P3):
//! in the host log, and in the events file of each session that lost
//! records,
//!
//! ```text
//! {"event":"records-skipped","count":120,"acp":100,"since":"…","until":"…"}
//! ```
//!
//! how many records, how many of them raw ACP, and when the first and the
//! last of them were made. The host log's note counts every record of the
//! gap, a session's only its own. A write that fails (a full disk) is
//! counted the same way, and noted, with the `error`, in the file that
//! lost it once that file takes a record again (the raw file's, in its
//! events file). What is queued is never dropped. For the tests,
//! `BRNR_TEST_LOG_STALL` names a file: while it exists, the logger writes
//! nothing, as if the disk had stopped.

use std::borrow::Cow;
use std::collections::HashMap;
use std::env;
use std::ffi::CString;
use std::fmt::Display;
use std::fs::{DirBuilder, File, OpenOptions, Permissions};
use std::io::{self, ErrorKind, Write};
use std::os::fd::AsFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, FileExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value, json};

use crate::{paths, sys};

/// What may be queued for the logger before records are skipped.
const LOG_BYTES: usize = 64 << 20;

/// Records skipped, the gap lasts until the queue is down to this: a disk
/// that only just can't keep up doesn't make every other record a note.
const ROOM_BYTES: usize = LOG_BYTES / 2;

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
    Close { session: String },
    Msg { session: Option<String>, ts: SystemTime, dir: Dir, bytes: Vec<u8> },
    Note { session: Option<String>, ts: SystemTime, event: String },
    // The queue has room again after `gap`.
    Skipped { ts: SystemTime, gap: Gap },
    // Called once what was queued before it is written.
    Written(Box<dyn FnOnce() + Send>),
    Finish,
}

impl Cmd {
    /// What it counts in the queue: what a record holds. Only records count.
    fn size(&self) -> usize {
        let (session, held) = match self {
            Cmd::Msg { session, bytes, .. } => (session, bytes.len()),
            Cmd::Note { session, event, .. } => (session, event.len()),
            _ => return 0,
        };
        size_of::<Cmd>() + held + session.as_ref().map_or(0, String::len)
    }
}

/// Where the host hands its records. A no-op when not logging.
#[derive(Clone)]
pub struct Sink(Option<(Sender<Cmd>, Arc<Queue>)>);

/// What the host and the logger share of the logger's queue.
#[derive(Default)]
struct Queue {
    /// Bytes of the records queued that the logger hasn't written yet.
    bytes: AtomicUsize,
    /// Records skipped since the queue was last full, until it has room.
    gap: Mutex<Option<Gap>>,
}

impl Queue {
    fn gap(&self) -> MutexGuard<'_, Option<Gap>> {
        self.gap.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The gap, if there is one, and when it ended: now.
    fn end_gap(&self) -> Option<(SystemTime, Gap)> {
        let mut gap = self.gap();
        gap.take().map(|gap| (SystemTime::now(), gap))
    }
}

/// Records skipped while the queue was full.
struct Gap {
    all: Lost,
    /// Each session's of them.
    sessions: HashMap<String, Lost>,
}

impl Gap {
    fn add(&mut self, session: Option<&str>, one: Lost) {
        self.all.add(&one);
        if let Some(session) = session {
            match self.sessions.get_mut(session) {
                Some(lost) => lost.add(&one),
                None => drop(self.sessions.insert(session.to_owned(), one)),
            }
        }
    }
}

/// Records that weren't written, for the `records-skipped` note.
struct Lost {
    count: u64,
    /// How many of them were raw ACP.
    acp: u64,
    /// When the first and the last of them were made.
    since: SystemTime,
    until: SystemTime,
    /// Why a file didn't take them, the first time; none if they were
    /// skipped.
    error: Option<String>,
}

impl Lost {
    fn none(ts: SystemTime) -> Lost {
        Lost { count: 0, acp: 0, since: ts, until: ts, error: None }
    }

    fn one(ts: SystemTime, raw: bool) -> Lost {
        Lost { count: 1, acp: raw.into(), ..Lost::none(ts) }
    }

    fn add(&mut self, other: &Lost) {
        self.count += other.count;
        self.acp += other.acp;
        self.since = self.since.min(other.since);
        self.until = self.until.max(other.until);
        if self.error.is_none() {
            self.error.clone_from(&other.error);
        }
    }

    fn note(&self) -> Value {
        let mut note = json!({
            "event": "records-skipped",
            "count": self.count,
            "acp": self.acp,
            "since": rfc3339(self.since),
            "until": rfc3339(self.until),
        });
        if let Some(error) = &self.error {
            note["error"] = json!(error);
        }
        note
    }
}

impl Sink {
    /// Starts `session`'s files (appending if they exist) under `cwd`'s
    /// project folder. Records for a session not opened go to the host log.
    pub fn open_session(&self, session: &str, cwd: &Path) {
        if let Some((tx, _)) = &self.0 {
            // Never skipped: it says where the session's records go.
            let _ = tx.send(Cmd::Open { session: session.to_owned(), cwd: cwd.to_owned() });
        }
    }

    /// Closes `session`'s files once what was recorded before is written.
    /// Records for it after go to the host log, until it is opened again.
    pub fn close_session(&self, session: &str) {
        if let Some((tx, _)) = &self.0 {
            // Never skipped: a process that serves session after session
            // would otherwise hold every file it ever opened.
            let _ = tx.send(Cmd::Close { session: session.to_owned() });
        }
    }

    /// Calls `then` once everything recorded before it is written, with the
    /// note of a gap that ends there (at once when not logging). Never
    /// skipped, and never waited for: `then` runs on the logger's thread.
    pub fn when_written(&self, then: impl FnOnce() + Send + 'static) {
        let Some((tx, _)) = &self.0 else { return then() };
        if let Err(mpsc::SendError(Cmd::Written(then))) = tx.send(Cmd::Written(Box::new(then))) {
            then();
        }
    }

    /// An ACP message: into the session's raw file (none with `log =
    /// "events"`), or the host log.
    pub fn msg(&self, session: Option<&str>, dir: Dir, bytes: &[u8]) {
        self.record(session, true, false, |ts| Cmd::Msg {
            session: session.map(str::to_owned),
            ts,
            dir,
            bytes: bytes.to_vec(),
        });
    }

    /// An event: into the session's events file, or the host log. It is
    /// written out here, on the caller's stack: it may nest as deeply as the
    /// ACP it came from (see json.rs).
    pub fn note(&self, session: Option<&str>, event: Value) {
        // How the process ended is never skipped: it is what tells an exit
        // from a death (`brnr doctor`, ADR 11).
        let keep = matches!(event["event"].as_str(), Some("exited" | "panic"));
        self.record(session, false, keep, |ts| Cmd::Note {
            session: session.map(str::to_owned),
            ts,
            event: event.to_string(),
        });
    }

    /// Queues the record `make` makes, or, with the queue full, counts it
    /// skipped without making it, unless it is one to `keep`. Never waits for
    /// the logger to write: it only takes the lock to take the gap.
    fn record(
        &self,
        session: Option<&str>,
        raw: bool,
        keep: bool,
        make: impl FnOnce(SystemTime) -> Cmd,
    ) {
        let Some((tx, queue)) = &self.0 else { return };
        let ts = {
            let mut gap = queue.gap();
            // Read under the lock: a gap the logger ends is before it.
            let ts = SystemTime::now();
            let room = if gap.is_some() { ROOM_BYTES } else { LOG_BYTES };
            if !keep && queue.bytes.load(Relaxed) > room {
                let new = || Gap { all: Lost::none(ts), sessions: HashMap::new() };
                gap.get_or_insert_with(new).add(session, Lost::one(ts, raw));
                return;
            }
            // Room again: the gap is noted where it was, before this record.
            if let Some(gap) = gap.take() {
                let _ = tx.send(Cmd::Skipped { ts, gap });
            }
            ts
        };
        let cmd = make(ts);
        queue.bytes.fetch_add(cmd.size(), Relaxed);
        let _ = tx.send(cmd);
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
        let (host, made) = open_append(&path)?;
        let (tx, rx) = mpsc::channel();
        let queue = Arc::new(Queue::default());
        let mut writer = Writer {
            head: Head { ids, proxy_pid },
            acp,
            host,
            host_path: path.clone(),
            sessions: HashMap::new(),
            queue: queue.clone(),
            stall: env::var_os("BRNR_TEST_LOG_STALL").map(PathBuf::from),
        };
        writer.made_private(SystemTime::now(), made);
        let thread = thread::spawn(move || writer.run(rx));
        Ok(Logger { sink: Sink(Some((tx, queue))), host_log: Some(path), thread: Some(thread) })
    }

    pub fn sink(&self) -> Sink {
        self.sink.clone()
    }

    pub fn host_log(&self) -> Option<&Path> {
        self.host_log.as_deref()
    }

    /// Writes everything recorded so far.
    pub fn finish(self) {
        if let (Some((tx, _)), Some(thread)) = (&self.sink.0, self.thread) {
            let _ = tx.send(Cmd::Finish);
            let _ = thread.join();
        }
    }
}

struct Writer {
    head: Head,
    /// Sessions get a raw ACP file.
    acp: bool,
    host: Out,
    host_path: PathBuf,
    sessions: HashMap<String, Files>,
    queue: Arc<Queue>,
    /// For the tests, from `BRNR_TEST_LOG_STALL`: while this file exists,
    /// nothing is written, as if the disk had stopped.
    stall: Option<PathBuf>,
}

/// A session's events, and its raw ACP unless `log = "events"`.
struct Files {
    events: Out,
    acp: Option<Out>,
}

impl Writer {
    fn run(mut self, rx: Receiver<Cmd>) {
        loop {
            let cmd = match rx.try_recv() {
                Ok(cmd) => cmd,
                Err(TryRecvError::Empty) => {
                    // Caught up: a gap is noted now, not with the next record.
                    if let Some((ts, gap)) = self.queue.end_gap() {
                        self.skipped(ts, gap);
                    }
                    match rx.recv() {
                        Ok(cmd) => cmd,
                        Err(_) => return,
                    }
                }
                Err(TryRecvError::Disconnected) => return,
            };
            if let Some(path) = &self.stall {
                while path.exists() {
                    thread::sleep(Duration::from_millis(10));
                }
            }
            let size = cmd.size();
            let more = self.handle(cmd);
            self.queue.bytes.fetch_sub(size, Relaxed);
            if !more {
                return;
            }
        }
    }

    /// Writes what `cmd` has; false once there is nothing more to come.
    fn handle(&mut self, cmd: Cmd) -> bool {
        match cmd {
            Cmd::Open { session, cwd } => self.open(session, &cwd),
            Cmd::Close { session } => drop(self.sessions.remove(&session)),
            Cmd::Msg { session, ts, dir, bytes } => {
                let files = session.as_deref().and_then(|s| self.sessions.get(s));
                // A session's, with no raw file to go in, is left out.
                if files.is_none_or(|f| f.acp.is_some()) {
                    let mut body = format!(r#""dir":"{}","#, dir.name()).into_bytes();
                    embed(&mut body, bytes.strip_suffix(b"\n").unwrap_or(&bytes));
                    self.write(ts, session.as_deref(), true, &body);
                }
            }
            Cmd::Note { session, ts, event } => {
                self.write(ts, session.as_deref(), false, &event_body(event));
            }
            Cmd::Skipped { ts, gap } => self.skipped(ts, gap),
            Cmd::Written(then) => {
                if let Some((ts, gap)) = self.queue.end_gap() {
                    self.skipped(ts, gap);
                }
                then();
            }
            Cmd::Finish => {
                if let Some((ts, gap)) = self.queue.end_gap() {
                    self.skipped(ts, gap);
                }
                return false;
            }
        }
        true
    }

    fn open(&mut self, session: String, cwd: &Path) {
        if self.sessions.contains_key(&session) {
            return;
        }
        let path = paths::session_log(cwd, &session);
        let now = SystemTime::now();
        let events = match self.open_file(now, &path) {
            Ok(out) => out,
            Err(err) => return self.failed(now, &session, &path, &err),
        };
        let (mut acp, mut acp_path) = (None, None);
        if self.acp {
            let raw = paths::acp_log(&path);
            match self.open_file(now, &raw) {
                Ok(out) => (acp, acp_path) = (Some(out), Some(raw.to_string_lossy().into_owned())),
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

    /// Opens `path` (see [`open_append`]), noting in the host log what it
    /// made private.
    fn open_file(&mut self, ts: SystemTime, path: &Path) -> io::Result<Out> {
        let (out, made) = open_append(path)?;
        self.made_private(ts, made);
        Ok(out)
    }

    /// Notes in the host log each path that was made private, with the mode
    /// it had: others could have read what was there before (ADR 59).
    fn made_private(&mut self, ts: SystemTime, made: Made) {
        for (path, mode) in made {
            let path = path.to_string_lossy();
            let event =
                json!({ "event": "made-private", "path": path, "mode": format!("{mode:o}") });
            self.note(ts, None, event);
        }
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
        self.write(ts, session, false, &event_body(event));
    }

    /// The records `gap` skipped, noted in each session's events file that
    /// lost some, and in the host log, at `ts`, when it ended.
    fn skipped(&mut self, ts: SystemTime, gap: Gap) {
        let (head, mut all) = (&self.head, gap.all);
        for (session, mut lost) in gap.sessions {
            // One not open had its records in the host log: counted there.
            let Some(files) = self.sessions.get_mut(&session) else { continue };
            if files.acp.is_none() {
                // Its raw ACP had nowhere to go, skipped or not.
                (all.count, all.acp) = (all.count - lost.acp, all.acp - lost.acp);
                (lost.count, lost.acp) = (lost.count - lost.acp, 0);
            }
            if lost.count > 0 {
                let note = head.record(ts, Some(&session), &event_body(lost.note()));
                files.events.write(head, Some(&session), ts, &note, lost);
            }
        }
        if all.count > 0 {
            let note = head.record(ts, None, &event_body(all.note()));
            self.host.write(head, None, ts, &note, all);
        }
    }

    /// Into the session's events or (`raw`) raw file, or the host log if it
    /// has none open. The host has no stderr to report a failed write on:
    /// it is noted in the file once the file takes records again (see
    /// [`Out::write`]); the raw file's, in the events file.
    fn write(&mut self, ts: SystemTime, session: Option<&str>, raw: bool, body: &[u8]) {
        let head = &self.head;
        let record = head.record(ts, session, body);
        let one = Lost::one(ts, raw);
        let Some(files) = session.and_then(|s| self.sessions.get_mut(s)) else {
            return self.host.write(head, None, ts, &record, one);
        };
        if !raw {
            return files.events.write(head, session, ts, &record, one);
        }
        let Some(acp) = &mut files.acp else { return };
        match acp.put(&record) {
            Err(err) => acp.lose(one, &err),
            Ok(()) => {
                if let Some(lost) = acp.failed.take() {
                    let note = head.record(ts, session, &event_body(lost.note()));
                    files.events.write(head, session, ts, &note, lost);
                }
            }
        }
    }
}

/// What every record starts with.
struct Head {
    ids: Ids,
    proxy_pid: Option<u32>,
}

impl Head {
    /// A record: the fields that join files together, then `body`.
    fn record(&self, ts: SystemTime, session: Option<&str>, body: &[u8]) -> Vec<u8> {
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
        let mut out = out.into_bytes();
        out.extend_from_slice(body);
        out
    }
}

/// The rest of a record that is an event.
fn event_body(event: impl Display) -> Vec<u8> {
    format!("\"event\":{event}}}\n").into_bytes()
}

/// A file records go into.
struct Out {
    file: File,
    /// What it didn't take since it last took a record.
    failed: Option<Lost>,
    /// A failed write left a line cut short.
    cut: bool,
}

impl Out {
    fn new(file: File) -> Out {
        Out { file, failed: None, cut: false }
    }

    /// Writes `record` into an events file or the host log, after a note of
    /// what the file didn't take before, if anything. What it doesn't take
    /// now is counted with that, as `what`: the record, or the records a
    /// note is about. Until the note is written, nothing comes after it.
    fn write(
        &mut self,
        head: &Head,
        session: Option<&str>,
        ts: SystemTime,
        record: &[u8],
        what: Lost,
    ) {
        if let Some(lost) = &self.failed {
            let note = head.record(ts, session, &event_body(lost.note()));
            if let Err(err) = self.put(&note) {
                return self.lose(what, &err);
            }
            self.failed = None;
        }
        if let Err(err) = self.put(record) {
            self.lose(what, &err);
        }
    }

    /// Writes `record`, on a line of its own.
    fn put(&mut self, record: &[u8]) -> io::Result<()> {
        let line: Cow<[u8]> =
            if self.cut { [b"\n", record].concat().into() } else { record.into() };
        let mut done = 0;
        while done < line.len() {
            let err = match self.file.write(&line[done..]) {
                Ok(0) => ErrorKind::WriteZero.into(),
                Ok(n) => {
                    done += n;
                    continue;
                }
                Err(err) if err.kind() == ErrorKind::Interrupted => continue,
                Err(err) => err,
            };
            self.cut |= done > 0;
            return Err(err);
        }
        self.cut = false;
        Ok(())
    }

    /// Counts `what` as not taken.
    fn lose(&mut self, what: Lost, err: &io::Error) {
        match &mut self.failed {
            Some(lost) => lost.add(&what),
            None => self.failed = Some(Lost { error: Some(err.to_string()), ..what }),
        }
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

/// A record of a host log or transcript with what [`redacted`] and
/// `Request::recorded` redact redacted (ADR 25 in docs/adr), as brnr
/// records it now: for one an older brnr wrote, shown again.
pub fn redact_record(record: &mut Value) {
    if let Some(bytes) = record.get("msg").and_then(Value::as_object).and_then(redacted)
        && let Ok(msg) = serde_json::from_slice(&bytes)
    {
        record["msg"] = msg;
    }
    if record["event"]["event"] == "started"
        && let Some(servers) = record.pointer_mut("/event/request/role/headless/mcp_servers")
    {
        redact_mcp_servers(servers);
    }
}

/// What [`open_private`] made private: each path, and the mode it had.
type Made = Vec<(PathBuf, u32)>;

/// Opens `path`, under the state directory, for appending, so a session's
/// file grows across hosts that serve it. Transcripts hold prompts and tool
/// output, so what it writes to is private to the user (ADR 59; see
/// [`open_private`]). A file that ends partway through a line (a process
/// that died mid-write) is left as it is, and the next record starts on a
/// line of its own (ADR 55).
fn open_append(path: &Path) -> io::Result<(Out, Made)> {
    let (file, made) = open_private(&paths::state_dir(), path)?;
    let mut last = *b"\n";
    let len = file.metadata()?.len();
    if len > 0 {
        file.read_exact_at(&mut last, len - 1)?;
    }
    Ok((Out { cut: last[0] != b'\n', ..Out::new(file) }, made))
}

/// Opens `path` under `root` for appending, private to the user before
/// anything is written (ADR 59): `root`, each directory below it and the
/// file are opened without following a symlink, each from the one before,
/// and created 0700 or 0600 if missing. Each is checked through what was
/// opened, so what is checked is what is written to: one that isn't a
/// directory (or a file with no other links) of the user's is refused, and
/// one that others could read, write or search is made private (`fchmod`).
fn open_private(root: &Path, path: &Path) -> io::Result<(File, Made)> {
    let mut made = Made::new();
    let flags = libc::O_DIRECTORY | libc::O_NOFOLLOW;
    let open_root = || OpenOptions::new().read(true).custom_flags(flags).open(root);
    let mut dir = match open_root() {
        Err(e) if e.kind() == ErrorKind::NotFound => {
            DirBuilder::new().recursive(true).mode(0o700).create(root).map_err(|e| at(root, e))?;
            open_root()
        }
        opened => opened,
    }
    .map_err(|e| at(root, e))?;
    keep_private(&dir, root, true, &mut made)?;
    let rel =
        path.strip_prefix(root).map_err(|_| refused(path, "is not in the state directory"))?;
    let (mut names, mut here) = (rel.components().peekable(), root.to_owned());
    while let Some(name) = names.next() {
        let Component::Normal(name) = name else {
            return Err(refused(path, "is not a plain path"));
        };
        here.push(name);
        let name = CString::new(name.as_bytes()).map_err(|e| at(&here, e.into()))?;
        if names.peek().is_none() {
            // Read too, for its last byte (ADR 55). Not blocked by a FIFO
            // put there; a regular file ignores O_NONBLOCK.
            let flags = libc::O_RDWR | libc::O_APPEND | libc::O_CREAT | libc::O_NONBLOCK;
            let file = sys::openat(dir.as_fd(), &name, flags | libc::O_NOFOLLOW, 0o600);
            let file = file.map_err(|e| at(&here, e))?;
            keep_private(&file, &here, false, &mut made)?;
            return Ok((file, made));
        }
        let flags = libc::O_RDONLY | flags;
        dir = match sys::openat(dir.as_fd(), &name, flags, 0) {
            Err(e) if e.kind() == ErrorKind::NotFound => {
                match sys::mkdirat(dir.as_fd(), &name, 0o700) {
                    Err(e) if e.kind() != ErrorKind::AlreadyExists => Err(e),
                    _ => sys::openat(dir.as_fd(), &name, flags, 0),
                }
            }
            opened => opened,
        }
        .map_err(|e| at(&here, e))?;
        keep_private(&dir, &here, true, &mut made)?;
    }
    Err(refused(path, "is not a file in the state directory"))
}

/// Refuses what `open_private` opened at `path` unless it is the user's own
/// directory (`dir`), or file with no other name, and makes it private if
/// others have any access to it, adding it to `made` with the mode it had.
fn keep_private(opened: &File, path: &Path, dir: bool, made: &mut Made) -> io::Result<()> {
    let meta = opened.metadata().map_err(|e| at(path, e))?;
    let mode = meta.mode() & 0o777;
    if dir && !meta.is_dir() {
        return Err(refused(path, "is not a directory"));
    } else if !dir && !meta.is_file() {
        return Err(refused(path, "is not a regular file"));
    } else if meta.uid() != sys::uid() {
        return Err(refused(path, &format!("is owned by uid {}", meta.uid())));
    } else if !dir && meta.nlink() != 1 {
        // Another name for it could be anywhere: appending there writes
        // the transcript into some other file.
        return Err(refused(path, &format!("has {} hard links", meta.nlink())));
    }
    if mode & 0o077 != 0 {
        opened.set_permissions(Permissions::from_mode(mode & 0o700)).map_err(|e| at(path, e))?;
        made.push((path.to_owned(), mode));
    }
    Ok(())
}

/// `err`, opening `path`, saying where, and what was there if that is why.
/// `O_NOFOLLOW` and `O_DIRECTORY` refused it; what it is is looked at only
/// for the message (a symlink is ELOOP on Linux, ENOTDIR on macOS for a
/// directory, and a FIFO opened without blocking ENXIO).
fn at(path: &Path, err: io::Error) -> io::Error {
    let errno = err.raw_os_error();
    let link = || std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink());
    match errno {
        Some(libc::ELOOP | libc::ENOTDIR | libc::ENXIO) if link() => refused(path, "is a symlink"),
        Some(libc::ENOTDIR) => refused(path, "is not a directory"),
        Some(libc::ENXIO) => refused(path, "is not a regular file"),
        _ => io::Error::new(err.kind(), format!("{}: {err}", path.display())),
    }
}

/// Why no transcript is written at `path`, and what to do.
fn refused(path: &Path, why: &str) -> io::Error {
    let msg = format!(
        "{}: {why}, so brnr won't write a transcript there (move it away, or set BRNR_HOME)",
        path.display()
    );
    io::Error::new(ErrorKind::PermissionDenied, msg)
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

    /// An empty file of this test's own.
    fn scratch(name: &str) -> PathBuf {
        let path = env::temp_dir().join(format!("brnr-log-{}-{name}", std::process::id()));
        File::create(&path).unwrap();
        path
    }

    fn appending(path: &Path) -> Out {
        Out::new(OpenOptions::new().append(true).open(path).unwrap())
    }

    /// Each line of `path`, as the record it is, if it is one.
    fn lines(path: &Path) -> Vec<Option<Value>> {
        let text = std::fs::read_to_string(path).unwrap();
        text.lines().map(|l| serde_json::from_str(l).ok()).collect()
    }

    /// The `records-skipped` notes in `path`: how many records, and of them
    /// raw ACP.
    fn skipped(path: &Path) -> Vec<(u64, u64)> {
        let notes = lines(path).into_iter().flatten().map(|r| r["event"].clone());
        let notes = notes.filter(|e| e["event"] == "records-skipped");
        notes.map(|e| (e["count"].as_u64().unwrap(), e["acp"].as_u64().unwrap())).collect()
    }

    fn head() -> Head {
        Head { ids: Ids { host_id: "h".into(), host_pid: 1, agent_pid: 2 }, proxy_pid: None }
    }

    #[test]
    fn adr_0011_how_the_process_ended_is_never_skipped() {
        let (tx, rx) = mpsc::channel();
        let queue = Arc::new(Queue::default());
        let sink = Sink(Some((tx, queue.clone())));
        queue.bytes.store(LOG_BYTES + 1, Relaxed);
        sink.note(Some("s"), json!({ "event": "a" }));
        sink.note(Some("s"), json!({ "event": "exited", "status": { "code": 0 } }));
        let shown: Vec<String> = (rx.try_iter())
            .map(|cmd| match cmd {
                Cmd::Note { event, .. } => event,
                Cmd::Skipped { gap, .. } => format!("skipped {}", gap.all.count),
                _ => "?".into(),
            })
            .collect();
        // The gap before it is noted first, so the file stays in order.
        assert_eq!(shown, ["skipped 1", r#"{"event":"exited","status":{"code":0}}"#]);
    }

    #[test]
    fn adr_0006_past_the_cap_records_are_skipped_and_noted_before_the_next() {
        let (tx, rx) = mpsc::channel();
        let queue = Arc::new(Queue::default());
        let sink = Sink(Some((tx, queue.clone())));
        sink.note(None, json!({ "event": "before" }));
        queue.bytes.store(LOG_BYTES + 1, Relaxed);
        sink.note(Some("s"), json!({ "event": "a" }));
        sink.msg(Some("s"), Dir::AgentToEditor, b"{}\n");
        sink.msg(None, Dir::AgentStderr, b"x");
        // Back under the cap, but not down to room yet: the gap goes on.
        queue.bytes.store(ROOM_BYTES + 1, Relaxed);
        sink.note(Some("s"), json!({ "event": "b" }));
        // A session's opening is never skipped.
        sink.open_session("t", Path::new("/"));
        queue.bytes.store(ROOM_BYTES, Relaxed);
        sink.note(Some("s"), json!({ "event": "after" }));

        let cmds: Vec<Cmd> = rx.try_iter().collect();
        let shown: Vec<String> = (cmds.iter())
            .map(|cmd| match cmd {
                Cmd::Note { event, .. } => event.clone(),
                Cmd::Open { session, .. } => format!("open {session}"),
                Cmd::Skipped { gap, .. } => {
                    let (all, s) = (&gap.all, &gap.sessions["s"]);
                    format!("skipped {} {}, s {} {}", all.count, all.acp, s.count, s.acp)
                }
                _ => "?".into(),
            })
            .collect();
        let want =
            [r#"{"event":"before"}"#, "open t", "skipped 4 2, s 3 1", r#"{"event":"after"}"#];
        assert_eq!(shown, want);
        // Counted from when it was queued: only what was.
        assert_eq!(queue.bytes.load(Relaxed), ROOM_BYTES + cmds[3].size());
    }

    #[test]
    fn adr_0006_a_gap_is_noted_in_the_host_log_and_each_session_that_lost_records() {
        let (host, s, t) = (scratch("gap-host"), scratch("gap-s"), scratch("gap-t"));
        let mut writer = Writer {
            head: head(),
            acp: false,
            host: appending(&host),
            host_path: host.clone(),
            sessions: HashMap::new(),
            queue: Arc::default(),
            stall: None,
        };
        // `log = "events"`: no raw files.
        writer.sessions.insert("s".into(), Files { events: appending(&s), acp: None });
        writer.sessions.insert("t".into(), Files { events: appending(&t), acp: None });
        let ts = SystemTime::now();
        let mut gap = Gap { all: Lost::none(ts), sessions: HashMap::new() };
        gap.add(Some("s"), Lost::one(ts, false));
        gap.add(Some("s"), Lost::one(ts, true));
        gap.add(Some("t"), Lost::one(ts, true));
        // Not open: its records go in the host log.
        gap.add(Some("u"), Lost::one(ts, false));
        gap.add(None, Lost::one(ts, true));
        writer.skipped(ts, gap);
        // Raw ACP for a session without a raw file had nowhere to go.
        assert_eq!(skipped(&s), [(1, 0)]);
        assert_eq!(skipped(&t), []);
        assert_eq!(skipped(&host), [(3, 1)]);
        for path in [host, s, t] {
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn adr_0022_a_closed_session_has_its_records_and_then_no_file() {
        let (host, s) = (scratch("close-host"), scratch("close-s"));
        let mut writer = Writer {
            head: head(),
            acp: false,
            host: appending(&host),
            host_path: host.clone(),
            sessions: HashMap::new(),
            queue: Arc::default(),
            stall: None,
        };
        writer.sessions.insert("s".into(), Files { events: appending(&s), acp: None });
        let note = |event: &str| Cmd::Note {
            session: Some("s".into()),
            ts: SystemTime::now(),
            event: json!({ "event": event }).to_string(),
        };
        for cmd in [note("last"), Cmd::Close { session: "s".into() }, note("after")] {
            assert!(writer.handle(cmd));
        }
        assert!(writer.sessions.is_empty());
        let events = |path: &Path| -> Vec<Value> {
            lines(path).into_iter().flatten().map(|r| r["event"]["event"].clone()).collect()
        };
        assert_eq!(events(&s), ["last"]);
        // Not open: into the host log, as for any session not opened.
        assert_eq!(events(&host), ["after"]);
        for path in [host, s] {
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn adr_0006_a_failed_write_is_noted_once_the_file_takes_records_again() {
        let path = scratch("failed");
        let head = head();
        // Open for reading only: no write goes in, as on a full disk.
        let mut out = Out::new(File::open(&path).unwrap());
        let t0 = SystemTime::now();
        for i in 0..3 {
            let ts = t0 + Duration::from_secs(i);
            let record = head.record(ts, None, &event_body(json!({ "event": i })));
            out.write(&head, None, ts, &record, Lost::one(ts, i == 2));
        }
        // It takes records again, after a write that left part of a line.
        std::fs::write(&path, r#"{"ts":"cut"#).unwrap();
        (out.file, out.cut) = (appending(&path).file, true);
        let ts = t0 + Duration::from_secs(5);
        let record = head.record(ts, None, &event_body(json!({ "event": "back" })));
        out.write(&head, None, ts, &record, Lost::one(ts, false));

        let lines = lines(&path);
        assert_eq!(lines.len(), 3, "{lines:?}");
        assert!(lines[0].is_none(), "the line cut short is ended");
        let note = &lines[1].as_ref().unwrap()["event"];
        assert_eq!(skipped(&path), [(3, 1)]);
        let (since, until) = (rfc3339(t0), rfc3339(t0 + Duration::from_secs(2)));
        assert_eq!((&note["since"], &note["until"]), (&json!(since), &json!(until)));
        assert!(note["error"].is_string(), "{note}");
        assert_eq!(lines[2].as_ref().unwrap()["event"]["event"], "back");
        let _ = std::fs::remove_file(path);
    }

    /// A directory of this test's own, empty, holding `home`: the state
    /// directory, not made yet.
    fn scratch_home(name: &str) -> (PathBuf, PathBuf) {
        let dir = env::temp_dir().join(format!("brnr-log-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir(&dir).unwrap();
        (dir.join("home"), dir)
    }

    fn mode(path: &Path) -> u32 {
        std::fs::symlink_metadata(path).unwrap().mode() & 0o777
    }

    fn chmod(path: &Path, mode: u32) {
        std::fs::set_permissions(path, Permissions::from_mode(mode)).unwrap();
    }

    /// `path`'s directories under `home`, `home` first, and `path`.
    fn chain(home: &Path, path: &Path) -> Vec<PathBuf> {
        let mut all: Vec<PathBuf> =
            path.ancestors().take_while(|p| p.starts_with(home)).map(Path::to_owned).collect();
        all.reverse();
        all
    }

    fn private_all(home: &Path, path: &Path) {
        for p in chain(home, path) {
            let want = if p == path { 0o600 } else { 0o700 };
            assert_eq!(mode(&p), want, "{}", p.display());
        }
    }

    #[test]
    fn adr_0059_new_and_private_paths_are_opened_as_they_are() {
        let (home, dir) = scratch_home("new");
        let path = home.join("projects/-w/s.jsonl");
        let (mut file, made) = open_private(&home, &path).unwrap();
        assert_eq!(made, []);
        private_all(&home, &path);
        file.write_all(b"one\n").unwrap();
        // Opened again: nothing to make private, and appended to.
        let (mut file, made) = open_private(&home, &path).unwrap();
        assert_eq!(made, []);
        file.write_all(b"two\n").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "one\ntwo\n");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn adr_0059_what_others_can_reach_is_made_private_before_a_write() {
        // Beneath private parents, and beneath parents anyone can search.
        for (name, parents) in [("open-file", 0o700), ("open-dirs", 0o755)] {
            let (home, dir) = scratch_home(name);
            let path = home.join("projects/-w/s.jsonl");
            open_private(&home, &path).unwrap().0.write_all(b"old\n").unwrap();
            let all = chain(&home, &path);
            for p in &all {
                chmod(p, if *p == path { 0o644 } else { parents });
            }
            let (mut file, made) = open_private(&home, &path).unwrap();
            // Private through the descriptor, before anything is written.
            private_all(&home, &path);
            let want: Made = (all.into_iter())
                .map(|p| if p == path { (p, 0o644) } else { (p, parents) })
                .filter(|(_, mode)| mode & 0o077 != 0)
                .collect();
            assert_eq!(made, want, "{name}");
            file.write_all(b"new\n").unwrap();
            assert_eq!(std::fs::read_to_string(&path).unwrap(), "old\nnew\n");
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn adr_0059_a_symlink_is_never_followed() {
        let (home, dir) = scratch_home("symlink");
        let path = home.join("projects/-w/s.jsonl");
        let victim = dir.join("victim");
        std::fs::create_dir(&victim).unwrap();
        chmod(&victim, 0o755);
        std::fs::write(victim.join("s.jsonl"), "keep me\n").unwrap();
        chmod(&victim.join("s.jsonl"), 0o644);
        // The file, its folder, and the state directory itself.
        let links = [
            (victim.join("s.jsonl"), path.clone()),
            (victim.clone(), home.join("projects/-w")),
            (victim.clone(), home.clone()),
        ];
        for (target, link) in links {
            let _ = std::fs::remove_dir_all(&home);
            std::fs::create_dir_all(link.parent().unwrap()).unwrap();
            std::os::unix::fs::symlink(&target, &link).unwrap();
            let err = open_private(&home, &path).unwrap_err().to_string();
            let want =
                format!("{}: is a symlink, so brnr won't write a transcript there", link.display());
            assert!(err.starts_with(&want), "{err}");
            assert_eq!(std::fs::read_to_string(victim.join("s.jsonl")).unwrap(), "keep me\n");
            assert_eq!((mode(&victim), mode(&victim.join("s.jsonl"))), (0o755, 0o644));
            assert_eq!(std::fs::read_dir(&victim).unwrap().count(), 1);
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn adr_0059_only_the_users_own_directories_and_files_are_written_to() {
        let (home, dir) = scratch_home("refused");
        let path = home.join("projects/-w/s.jsonl");
        let refused = |why: &str| {
            let err = open_private(&home, &path).unwrap_err().to_string();
            assert!(err.contains(why), "{err}");
        };
        // Another name for the file, which could be anywhere.
        let other = dir.join("other");
        std::fs::write(&other, "keep me\n").unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::hard_link(&other, &path).unwrap();
        refused(": has 2 hard links, so brnr won't");
        assert_eq!(std::fs::read_to_string(&other).unwrap(), "keep me\n");
        // A FIFO: refused, not waited on.
        std::fs::remove_file(&path).unwrap();
        let fifo = std::process::Command::new("mkfifo").arg(&path).status().unwrap();
        assert!(fifo.success());
        refused(": is not a regular file");
        // A file where a directory goes.
        std::fs::remove_dir_all(home.join("projects")).unwrap();
        std::fs::write(home.join("projects"), "").unwrap();
        refused(": is not a directory");
        // Another user's (root's): `/`, as the state directory.
        if sys::uid() != 0 {
            let err = open_private(Path::new("/"), Path::new("/x.jsonl")).unwrap_err();
            assert!(err.to_string().starts_with("/: is owned by uid 0"), "{err}");
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn adr_0025_secrets_are_redacted_keys_and_structure_stay() {
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
