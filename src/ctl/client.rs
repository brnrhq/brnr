//! How the commands reach a running brnr process: over its control socket
//! (ADR 3.9), each answer and event read as brnr reads what it wrote
//! (ADR 2.5), a subscription caught up once the process cuts it off
//! (ADR 10.2).
//!
//! [`discover`] finds the processes, asking them all for their status at
//! once, and waiting about [`ANSWER_WAIT`] in all (ADR 10.4). A [`Host`] is
//! one of them: [`Host::call`] asks it something and waits for the answer,
//! [`Host::pending`] has what waits for an answer there, and [`Conn::open`]
//! subscribes to its events and asks it things on the same connection. A
//! request waits [`ANSWER_WAIT`] for its answer, or [`AGENT_WAIT`] for one
//! that waits on the agent, and a connection the process doesn't take, or
//! a write that makes no progress, for [`ANSWER_WAIT`] fails the same way:
//! "not answering". An answer is the process's `ok` and `error` taken off:
//! an error it says is the command's, also one it says to no request in
//! particular, before it closes the connection (a request too long to read,
//! too many connections: ADR 10.3, ADR 10.4). What the agent sent nests no
//! deeper than [`json::DEPTH`], and the host wraps it in a few levels of its
//! own: a line past [`json::OWN_DEPTH`] is none the host makes, and is
//! refused, and said (P3).
//!
//! A process may be another brnr than this one: an editor's lives as long as
//! the editor's session, past an upgrade. Its metadata says which brnr it is
//! and what it speaks (`brnr`, `protocol`); an error that may come of the
//! difference says so, and how to restart it ([`skewed`], ADR 8.8).

use std::collections::VecDeque;
use std::fs;
use std::io::{self, BufRead, BufReader, ErrorKind, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::error::{self, Code, Error};
use crate::host::PROTOCOL;
use crate::{json, lock, paths, render, sys};

use super::catch_up::{Follow, MISSED, TRIES};

/// How long a request waits for its answer.
const ANSWER_WAIT: Duration = Duration::from_secs(5);

/// How long one that waits on the agent does: it may take a while to switch
/// model, or to fork or close a session.
const AGENT_WAIT: Duration = Duration::from_secs(120);

/// A running brnr process: its metadata file and, if it answered, its live
/// status.
#[derive(Clone)]
pub(super) struct Host {
    pub(super) meta: Value,
    pub(super) status: Option<Value>,
}

impl Host {
    pub(super) fn info(&self) -> &Value {
        self.status.as_ref().unwrap_or(&self.meta)
    }

    /// Its pid, which names its socket and metadata.
    pub(super) fn id(&self) -> &str {
        self.meta["id"].as_str().unwrap_or("?")
    }

    pub(super) fn sessions(&self) -> &[Value] {
        self.status.as_ref().and_then(|s| s["sessions"].as_array()).map_or(&[], Vec::as_slice)
    }

    /// Session `id`, as its status has it, if it serves it.
    pub(super) fn session(&self, id: &str) -> Option<&Value> {
        session_in(self.status.as_ref()?, id)
    }

    /// Whether it has closed `session` and holds its lock still, until its
    /// records are written (ADR 5.5).
    pub(super) fn closing(&self, session: &str) -> bool {
        let closing = self.status.as_ref().and_then(|s| s["closing"].as_array());
        closing.is_some_and(|c| c.iter().any(|s| s == session))
    }

    /// The sessions whose locks it holds, as `locks` has them: what it
    /// serves, asked of nobody.
    pub(super) fn held(&self, locks: &[lock::Entry]) -> Vec<String> {
        let pid = self.id().parse().ok();
        let held = locks.iter().filter(|e| e.pid.is_some() && e.pid == pid);
        held.filter_map(|e| e.session.clone()).collect()
    }

    /// A request whose failure is the command's failure, and its answer.
    pub(super) fn call(&self, req: &Value) -> Result<Value, Error> {
        self.kept(req, ANSWER_WAIT)
    }

    /// [`Host::call`], for a request that waits on the agent.
    pub(super) fn call_agent(&self, req: &Value) -> Result<Value, Error> {
        self.kept(req, AGENT_WAIT)
    }

    /// The answer to `req`, waiting `wait` for it. `Err`: the process's
    /// error, or why there is no answer to use, which names the process: one
    /// it can't be asked, or that it doesn't give, is unreachable.
    fn kept(&self, req: &Value, wait: Duration) -> Result<Value, Error> {
        let named = |e: String| format!("process {}: {e}", self.id());
        let unreachable =
            |e: io::Error| Error::new(Code::process_unreachable, named(e.to_string()));
        let line = self.exchange(req, wait).map_err(unreachable)?;
        read(&line).map_err(|e| Error::from(named(e)))?.map_err(|e| skewed(e, &self.meta))
    }

    /// What waits for an answer in the process.
    pub(super) fn pending(&self) -> Result<Pending, Error> {
        self.kept(&json!({ "cmd": "pending" }), ANSWER_WAIT).map(Pending::from)
    }

    /// A request and the line that answers it, waiting up to `timeout` for
    /// it, and [`ANSWER_WAIT`] for the process to take the request.
    fn exchange(&self, req: &Value, timeout: Duration) -> io::Result<String> {
        let mut conn = self.connect()?;
        closed_ok(conn.set_read_timeout(Some(timeout)))?;
        closed_ok(conn.set_write_timeout(Some(ANSWER_WAIT)))?;
        let wrote = match writeln!(conn, "{req}") {
            Err(e) if waited(&e) => return Err(io::Error::other("not answering")),
            wrote => wrote,
        };
        let mut line = String::new();
        let read = BufReader::new(conn).read_line(&mut line);
        match (wrote, read) {
            // A write that failed may have been answered first: the process
            // says why before it closes the connection (ADR 10.3, 10.4).
            (_, Ok(n)) if n > 0 => Ok(line),
            (Err(e), _) => Err(e),
            (Ok(()), Err(e)) if waited(&e) => Err(io::Error::other("not answering")),
            (Ok(()), read) => read.map(|_| line),
        }
    }

    fn connect(&self) -> io::Result<UnixStream> {
        let socket =
            self.meta["socket"].as_str().ok_or(io::Error::other("no socket in metadata"))?;
        connect(Path::new(socket))
    }
}

/// A connection to a process's `socket`, waiting [`ANSWER_WAIT`] for the
/// process to take it: on Linux one whose backlog is full (a stopped
/// process's) makes a connection wait for as long as it stays full. One
/// not taken by then fails, "not answering" (`TimedOut`).
fn connect(socket: &Path) -> io::Result<UnixStream> {
    match sys::connect_within(socket, ANSWER_WAIT) {
        Err(e) if e.kind() == ErrorKind::TimedOut => {
            Err(io::Error::new(ErrorKind::TimedOut, "not answering"))
        }
        connected => connected,
    }
}

/// Whether anyone listens on `socket`, asked without waiting: a full backlog
/// (Linux) is someone listening, as a connection taken is. `Err`: why not.
pub(super) fn listening(socket: &Path) -> io::Result<()> {
    match sys::connect_within(socket, Duration::ZERO) {
        Err(e) if e.kind() == ErrorKind::TimedOut => Ok(()),
        connected => connected.map(drop),
    }
}

/// A timeout set, or the connection already closed by the process: macOS
/// refuses one then (EINVAL), where reading and writing don't wait anyway (a
/// process with too many connections closes a new one at once, ADR 10.4).
fn closed_ok(set: io::Result<()>) -> io::Result<()> {
    match set {
        Err(e) if e.raw_os_error() == Some(libc::EINVAL) => Ok(()),
        set => set,
    }
}

/// Whether a read or write failed for its timeout.
fn waited(e: &io::Error) -> bool {
    matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut)
}

/// Whether `msg` is an error the process says to no request in particular,
/// before it closes the connection: a request too long to read (ADR 10.3), or
/// too many connections (ADR 10.4). It answers the request waiting (P3).
fn unasked_error(msg: &Value) -> bool {
    msg.get("req_id").is_none() && msg["ok"] == false
}

/// The answer `line` without its envelope, read as brnr reads what it
/// wrote, to `json::OWN_DEPTH` (ADR 2.5). `Err`: why it can't be read;
/// `Ok(Err)`: the process's `error`.
fn read(line: &str) -> Result<Result<Value, Error>, String> {
    let line = line.as_bytes();
    json::within(line, json::OWN_DEPTH, "its answer")?;
    Ok(unwrapped(json::parse(line, json::OWN_DEPTH).ok_or("its answer isn't JSON")?))
}

/// An answer, `ok`, without `ok` and `req_id`; else the `error` it says,
/// with its code (ADR 8.7).
fn unwrapped(mut answer: Value) -> Result<Value, Error> {
    if answer["ok"].as_bool() != Some(true) {
        return Err(match &answer["error"] {
            Value::Null => Error::from("request failed"),
            error => Error::of(error),
        });
    }
    if let Some(map) = answer.as_object_mut() {
        map.shift_remove("ok");
        map.shift_remove("req_id");
    }
    Ok(answer)
}

/// How the process `meta` describes differs from this brnr, and what to do
/// about it, where it speaks another protocol, or predates `protocol`
/// (ADR 8.8); `None` where it speaks this brnr's.
pub(super) fn skew(meta: &Value) -> Option<String> {
    let theirs = meta["protocol"].as_u64();
    if theirs == Some(PROTOCOL) {
        return None;
    }
    let pid = meta["id"].as_str().unwrap_or("?");
    let ours = format!("brnr {}, protocol {PROTOCOL}", env!("CARGO_PKG_VERSION"));
    let what = match (theirs, meta["brnr"].as_str()) {
        (Some(p), Some(v)) => {
            let which = if p < PROTOCOL { "an older" } else { "a newer" };
            format!("{which} brnr (brnr {v}, protocol {p}) than this one ({ours})")
        }
        _ => format!("an older brnr than this one ({ours}), from before processes said theirs"),
    };
    // An editor's process is the editor's `brnr acp`'s, which runs the brnr
    // it was started as (ADR 3.1).
    let restart = if meta["proxy_pid"].is_number() {
        "restart the agent in its editor to run this brnr".to_owned()
    } else {
        format!(
            "stop it (brnr process stop {pid}) and resume its sessions (brnr session resume \
             <session>) to run them in this brnr"
        )
    };
    Some(format!("process {pid} runs {what}: {restart}"))
}

/// `err`, from the process `meta` describes, with what [`skew`] says where
/// it may come of the difference: `command_unknown` (a request, or a field
/// of one, the process doesn't have), and any error from a process that
/// predates `protocol`, whose errors are text that can't be told apart.
pub(super) fn skewed(err: Error, meta: &Value) -> Error {
    let may = err.code == Code::command_unknown || meta["protocol"].is_null();
    match skew(meta) {
        Some(note) if may => err.map(|m| format!("{m}; {note}")),
        _ => err,
    }
}

/// Session `id` in a process's `status` answer, if it serves it.
pub(super) fn session_in<'a>(status: &'a Value, id: &str) -> Option<&'a Value> {
    status["sessions"].as_array()?.iter().find(|s| s["session"] == id)
}

/// The counts a session has in a process's `status` answer (ADR 8.2).
pub(super) const COUNTS: [&str; 4] =
    ["held_count", "context_count", "pending_count", "elicitation_count"];

/// The count `key` in `report`, part of a `status` answer. A count that
/// isn't there (an older brnr's process, which names it otherwise) is no
/// count, not 0: an error says so (P4).
pub(super) fn count(report: &Value, key: &str) -> Result<u64, Error> {
    report[key].as_u64().ok_or_else(|| missing(key))
}

/// That a process's `status` answer has no `key`, which this brnr reads:
/// an older brnr's process, which names it otherwise, as
/// `command_unknown` is (ADR 8.7), and [`skewed`] says how to restart it
/// (ADR 8.8).
pub(super) fn missing(key: &str) -> Error {
    Error::new(Code::command_unknown, format!("the process's status has no {key}"))
}

/// What waits for an answer in a process, as its `pending` answer has it:
/// permission requests and elicitations.
pub(super) struct Pending {
    approvals: Vec<Value>,
    elicitations: Vec<Value>,
}

impl Pending {
    fn from(mut answer: Value) -> Pending {
        let mut list = |key: &str| match answer[key].take() {
            Value::Array(items) => items,
            _ => Vec::new(),
        };
        Pending { approvals: list("pending"), elicitations: list("elicitations") }
    }

    /// The permission requests of session `only`, or of every session.
    pub(super) fn approvals<'a>(
        &'a self,
        only: Option<&'a str>,
    ) -> impl Iterator<Item = &'a Value> {
        self.approvals.iter().filter(move |p| only.is_none_or(|id| p["session"] == id))
    }

    /// Permission request `request` of `session`.
    pub(super) fn approval(&self, session: &str, request: &str) -> Option<&Value> {
        self.approvals.iter().find(|p| p["session"] == session && p["request"] == request)
    }

    /// The elicitations of session `only`, or every one, outside any
    /// session too.
    pub(super) fn elicitations<'a>(
        &'a self,
        only: Option<&'a str>,
    ) -> impl Iterator<Item = &'a Value> {
        self.elicitations.iter().filter(move |e| only.is_none_or(|id| e["session"] == id))
    }

    /// Elicitation `request` of `session`, or, with none, outside any
    /// session.
    pub(super) fn elicitation(&self, session: Option<&str>, request: &str) -> Option<&Value> {
        let mut all = self.elicitations.iter();
        all.find(|e| e["request"] == request && e["session"].as_str() == session)
    }
}

// ---- finding the processes -----------------------------------------------

/// How much longer than a probe's own wait [`discover`] waits for it: the
/// probe's starts once it has connected, and telling a process gone takes a
/// moment more (see `gone`), so one that answers just in time still counts.
/// One that took its time to connect (a full backlog, on Linux) may not.
const PROBE_MARGIN: Duration = Duration::from_secs(1);

/// Every process with a metadata file, asking each for its live status.
/// Files left by a process that is gone are removed, even when its pid is
/// another process's now (see `gone`). The runtime directory must be
/// private, as the processes themselves require: anyone who could write to
/// it could list a process of their own, and be sent what brnr sends.
pub(super) fn discover() -> Result<Vec<Host>, String> {
    discover_in(&paths::runtime_dir())
}

/// [`discover`] in runtime directory `dir`. Each process is asked on a
/// thread of its own, so that k processes that don't answer cost one wait,
/// not k (ADR 10.4). One not back by then counts as not answering, and its
/// thread isn't waited for: it ends by itself, as its connection, write and
/// read each wait [`ANSWER_WAIT`] at most, but needn't hold the command.
fn discover_in(dir: &Path) -> Result<Vec<Host>, String> {
    match paths::check_private(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("{}: {e}", dir.display())),
    }
    let Ok(entries) = fs::read_dir(dir) else { return Ok(Vec::new()) };
    let found: Vec<(PathBuf, Host)> = (entries.flatten().map(|e| e.path()))
        .filter(|path| path.extension().is_some_and(|e| e == "json"))
        .filter_map(|path| Some((path.clone(), Host { meta: read_meta(&path)?, status: None })))
        .collect();
    let (tx, rx) = mpsc::channel();
    for (i, (path, host)) in found.iter().enumerate() {
        let (tx, path, host) = (tx.clone(), path.clone(), host.clone());
        thread::spawn(move || tx.send((i, probe(&path, &host))));
    }
    drop(tx);
    let mut probes: Vec<Option<Probe>> = found.iter().map(|_| None).collect();
    let until = Instant::now() + ANSWER_WAIT + PROBE_MARGIN;
    while let Ok((i, probe)) = rx.recv_timeout(until.saturating_duration_since(Instant::now())) {
        probes[i] = Some(probe);
    }
    let mut hosts = Vec::new();
    for ((path, mut host), probe) in found.into_iter().zip(probes) {
        match probe.unwrap_or_else(|| Probe::Silent("not answering".into())) {
            Probe::Answered(status) => host.status = Some(status),
            Probe::Silent(_) => {}
            Probe::Gone => {
                let _ = fs::remove_file(&path);
                let _ = fs::remove_file(path.with_extension("sock"));
                continue;
            }
        }
        hosts.push(host);
    }
    hosts.sort_by(|a, b| a.meta["started"].as_str().cmp(&b.meta["started"].as_str()));
    Ok(hosts)
}

/// What a process whose metadata is at `path` turned out to be, asked for
/// its status.
pub(super) enum Probe {
    /// Running and answering: its status.
    Answered(Value),
    /// Running but not answering (ADR 3.9): why.
    Silent(String),
    /// Gone, whatever process has its pid now.
    Gone,
}

/// Asks the process whose metadata is at `path` for its status, and says
/// what it is.
pub(super) fn probe(path: &Path, host: &Host) -> Probe {
    let pid = host.meta["host_pid"].as_i64().unwrap_or(0);
    let socket = path.with_extension("sock");
    match host.exchange(&json!({ "cmd": "status" }), ANSWER_WAIT) {
        Ok(line) => match read(&line) {
            Ok(Ok(status)) => Probe::Answered(status),
            Ok(Err(why)) => Probe::Silent(why.message),
            Err(why) => Probe::Silent(why),
        },
        Err(e) if gone(pid, &socket, path, &e) => Probe::Gone,
        // Running, but accepting nothing: its backlog is full (`made`).
        Err(e) if refused(&e) => Probe::Silent("not answering".into()),
        Err(e) => Probe::Silent(e.to_string()),
    }
}

/// How long a refused connection waits to be tried again: a process that
/// is starting binds its socket a moment before it listens on it.
const REFUSED_AGAIN: Duration = Duration::from_millis(20);

/// Whether the brnr process with `pid` that made `file` in the runtime
/// directory (its metadata, or its socket) is gone, a connection to its
/// `socket` having failed with `err`.
///
/// A brnr process binds its socket and listens on it before it writes
/// anything else there, and listens until it removes it as it ends. While
/// it lives, connecting succeeds, whether it answers or not (SIGSTOPped
/// too: the kernel queues the connection). A socket nobody listens on
/// refuses, as one that isn't there does: the process that made it is
/// gone, even when another process has its pid now (a reboot, or pids
/// wrapping around). A pid that has exited is gone, as for `gone_within`:
/// one that isn't the user's isn't brnr's, and a zombie nothing has reaped
/// yet listens no more, though `kill(pid, 0)` still reaches it.
pub(super) fn gone(pid: i64, socket: &Path, file: &Path, err: &io::Error) -> bool {
    if pid <= 0 || sys::exited(pid as libc::pid_t) {
        return true;
    }
    if !refused(err) {
        return false;
    }
    std::thread::sleep(REFUSED_AGAIN);
    match listening(socket) {
        Err(e) if refused(&e) => !made(pid, file),
        _ => false,
    }
}

/// A new subscription: its peer, and where it starts.
type Resubscribed = (String, u64);

/// How long a process that hangs up on a request unanswered has to go: one
/// that is stopping writes what it has left before its socket goes.
const STOPPING_WAIT: Duration = Duration::from_secs(10);

/// Whether the process whose socket is `socket` has gone, within `wait`:
/// it has exited, its pid naming the socket. Its socket isn't asked: a
/// process removes it before its last records are written (ADR 5.5), a
/// full backlog refuses on macOS and blocks a connection on Linux. Only a
/// socket no pid names goes by whether anyone listens. A pid that is
/// another process's now is waited for to the end.
fn gone_within(socket: &Path, wait: Duration) -> bool {
    let pid = socket.file_stem().and_then(|s| s.to_str()?.parse::<libc::pid_t>().ok());
    let until = Instant::now() + wait;
    loop {
        let gone = match pid {
            Some(pid) => sys::exited(pid),
            None => listening(socket).is_err_and(|e| refused(&e)),
        };
        if gone {
            return true;
        }
        if Instant::now() >= until {
            return false;
        }
        std::thread::sleep(REFUSED_AGAIN);
    }
}

/// Whether a connection failed because nobody listens: refused, or no
/// socket there.
fn refused(err: &io::Error) -> bool {
    matches!(err.kind(), ErrorKind::ConnectionRefused | ErrorKind::NotFound)
}

/// Whether the process with `pid` now may be the one that made `file`,
/// though its socket refuses: macOS also refuses a connection to a socket
/// whose backlog is full, as a stopped process's fills up with every
/// `brnr session list` that waited on it. One that started after `file` was
/// written is another process. What can't be told counts as "may be":
/// nothing is removed on a guess (P4).
#[cfg(target_vendor = "apple")]
fn made(pid: i64, file: &Path) -> bool {
    use std::time::UNIX_EPOCH;
    let Ok(written) = fs::metadata(file).and_then(|m| m.modified()) else { return true };
    // SAFETY: proc_bsdinfo is plain data, for which all zeros is a valid
    // value.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = size_of::<libc::proc_bsdinfo>() as libc::c_int;
    let buffer = (&raw mut info).cast();
    // SAFETY: buffer points at info, and size is its size, as proc_pidinfo
    // fills it.
    let got =
        unsafe { libc::proc_pidinfo(pid as libc::c_int, libc::PROC_PIDTBSDINFO, 0, buffer, size) };
    if got != size {
        return true;
    }
    let started = UNIX_EPOCH
        + Duration::from_secs(info.pbi_start_tvsec)
        + Duration::from_micros(info.pbi_start_tvusec);
    started <= written
}

/// Linux refuses a connection only when nobody listens: a full backlog
/// makes it wait.
#[cfg(not(target_vendor = "apple"))]
fn made(_pid: i64, _file: &Path) -> bool {
    false
}

/// A process's metadata file, if it is one: the socket it names must be the
/// one next to it in the runtime directory.
pub(super) fn read_meta(path: &Path) -> Option<Value> {
    let meta: Value = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    let socket = meta["socket"].as_str().map(Path::new);
    (socket == Some(&path.with_extension("sock"))).then_some(meta)
}

// ---- a connection that subscribes and calls ------------------------------

/// One control connection that is subscribed to events and also makes
/// requests: responses are matched by `req_id`, and events that arrive in
/// between are kept for `next_event`. Cut off by the process for falling
/// behind, it catches up from the transcript and goes on live on a new
/// connection (ADR 10.2, see catch_up.rs).
pub(super) struct Conn {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
    events: VecDeque<Value>,
    next_req: u64,
    /// The process's socket, to subscribe on again.
    socket: Option<PathBuf>,
    /// Subscribed: where it is, to catch up from.
    follow: Option<Follow>,
    /// What it missed, once it has given up catching up.
    missed: Option<Value>,
    /// How many times it has connected again.
    reconnects: u32,
    /// The process's metadata, to say how it differs from this brnr
    /// ([`skewed`]); none on the start channel, whose process is this brnr.
    meta: Option<Value>,
}

impl Conn {
    pub(super) fn open(host: &Host) -> Result<Conn, Error> {
        let unreachable =
            |e| Error::new(Code::process_unreachable, format!("process {}: {e}", host.id()));
        let stream = host.connect().map_err(unreachable)?;
        let mut conn = Conn::new(stream)?;
        conn.socket = host.meta["socket"].as_str().map(PathBuf::from);
        conn.meta = Some(host.meta.clone());
        Ok(conn)
    }

    /// One over `stream`: a control connection, or the start channel. A
    /// request the process takes nothing of for [`ANSWER_WAIT`] fails.
    pub(super) fn new(stream: UnixStream) -> Result<Conn, String> {
        super::stoppable(&stream);
        let writer = stream.try_clone().map_err(|e| e.to_string())?;
        closed_ok(writer.set_write_timeout(Some(ANSWER_WAIT))).map_err(|e| e.to_string())?;
        Ok(Conn {
            reader: BufReader::new(stream),
            writer,
            events: VecDeque::new(),
            next_req: 0,
            socket: None,
            follow: None,
            missed: None,
            reconnects: 0,
            meta: None,
        })
    }

    /// Its connection, to look at without reading (`brnr event notify`),
    /// and how many times it has connected again, to tell a new one.
    pub(super) fn socket(&self) -> (io::Result<UnixStream>, u32) {
        (self.writer.try_clone(), self.reconnects)
    }

    /// Subscribes to `events`, of `session` (the command's to filter; the
    /// process sends every session's) or of every session.
    pub(super) fn subscribe(
        &mut self,
        events: &[&str],
        session: Option<&str>,
    ) -> Result<(), Error> {
        let answer = self.call(json!({ "cmd": "subscribe", "events": events }))?;
        self.tell_unknown(&answer);
        if let Some(socket) = self.socket.clone() {
            self.follow = Follow::new(socket, &answer, events, session);
        }
        Ok(())
    }

    /// Says on stderr which of the events asked for the process doesn't have,
    /// as its `subscribe` answer names them: an older brnr's (ADR 8.8). The
    /// command goes on with the rest (P3).
    fn tell_unknown(&self, answer: &Value) {
        let unknown = answer["unknown_events"].as_array().filter(|u| !u.is_empty());
        let Some(unknown) = unknown else { return };
        let names: Vec<&str> = unknown.iter().filter_map(Value::as_str).collect();
        let pid = self.meta.as_ref().and_then(|m| m["id"].as_str()).unwrap_or("?");
        let mut said = format!("process {pid} doesn't send {}", names.join(", "));
        if let Some(note) = self.meta.as_ref().and_then(skew) {
            said = format!("{said}; {note}");
        }
        errln!("brnr: {said}");
    }

    /// Follows only `session`, which the subscription came before.
    pub(super) fn only(&mut self, session: &str) {
        if let Some(f) = &mut self.follow {
            f.session = Some(session.to_owned());
        }
    }

    /// The start channel, subscribed to `events` by the process `ready` names
    /// once the start commits: as a subscription, from where it says.
    pub(super) fn subscribed_at(&mut self, ready: &Value, events: &[&str], session: &str) {
        let socket = paths::runtime_dir().join(format!("{}.sock", ready["pid"]));
        self.follow = Follow::new(socket, ready, events, Some(session));
    }

    /// What it missed and couldn't catch up on, once it has given up (ADR 10.2).
    pub(super) fn missed(&self) -> Option<&Value> {
        self.missed.as_ref()
    }

    /// Whether the process has gone, for a command whose connection to it
    /// failed: it has exited within [`STOPPING_WAIT`], as catching up
    /// decides. Not if brnr doesn't know which process it is: it doesn't
    /// guess (P4).
    pub(super) fn process_gone(&self) -> bool {
        let socket = self.socket.as_ref().or(self.follow.as_ref().map(|f| &f.socket));
        socket.is_some_and(|socket| gone_within(socket, STOPPING_WAIT))
    }

    /// How a command ends that has got `Err(why)` from the connection: with
    /// [`MISSED`] once it has said what it missed, if it gave up catching
    /// up; else as `why`.
    pub(super) fn ended(&self, why: Error) -> Result<ExitCode, Error> {
        let Some(note) = &self.missed else { return Err(why) };
        super::missed(note);
        let text = render::event(note, &render::Options { session: false, time: false });
        errln!("brnr: {}", text.unwrap_or_default());
        Ok(ExitCode::from(MISSED))
    }

    /// A request whose failure is an error, and its answer, waiting
    /// [`ANSWER_WAIT`] for it: none waits on the agent (ADR 10.4).
    pub(super) fn call(&mut self, req: Value) -> Result<Value, Error> {
        let answer = self.call_until(req, Instant::now().checked_add(ANSWER_WAIT))?;
        answer.ok_or_else(|| "not answering".into())
    }

    /// What waits for an answer in the process.
    pub(super) fn pending(&mut self) -> Result<Pending, Error> {
        self.call(json!({ "cmd": "pending" })).map(Pending::from)
    }

    /// The events read and not yet taken, oldest first.
    pub(super) fn queued(&self) -> impl DoubleEndedIterator<Item = &Value> {
        self.events.iter()
    }

    /// A request whose failure is an error, answered by `deadline`;
    /// `Ok(None)` when it passes first. One that only asks (`status`,
    /// `pending`) is asked again once caught up, if the process cut the
    /// connection off before it answered, and waits as long again.
    pub(super) fn call_until(
        &mut self,
        req: Value,
        deadline: Option<Instant>,
    ) -> Result<Option<Value>, Error> {
        let answer = self.answer_until(req, deadline)?.map(unwrapped).transpose();
        answer.map_err(|e| self.skewed(e))
    }

    /// `err`, of the process, with what [`skew`] says where it may come of
    /// the difference ([`skewed`]); as it is on the start channel.
    pub(super) fn skewed(&self, err: Error) -> Error {
        match &self.meta {
            Some(meta) => skewed(err, meta),
            None => err,
        }
    }

    /// The answer to `req` as the process gave it, error or not; `Err` only
    /// if it can't be had (the connection failed), `Ok(None)` when `deadline`
    /// passes.
    fn answer_until(
        &mut self,
        mut req: Value,
        mut deadline: Option<Instant>,
    ) -> Result<Option<Value>, String> {
        let wait = deadline.map(|d| d.saturating_duration_since(Instant::now()));
        self.next_req += 1;
        let id = format!("r{}", self.next_req);
        req["req_id"] = json!(id);
        if let Err(e) = writeln!(self.writer, "{req}") {
            return self.unwritten(e).map(Some);
        }
        let again = matches!(req["cmd"].as_str(), Some("status" | "pending"));
        loop {
            let msg = match self.read(deadline) {
                Ok(Some(msg)) => msg,
                Ok(None) => return Ok(None),
                Err(why) if again && self.follow.is_some() => {
                    self.catch_up(why)?;
                    deadline = wait.and_then(|w| Instant::now().checked_add(w));
                    if let Err(e) = writeln!(self.writer, "{req}") {
                        return self.unwritten(e).map(Some);
                    }
                    continue;
                }
                Err(why) => return Err(why),
            };
            if msg["req_id"] == id.as_str() || unasked_error(&msg) {
                return Ok(Some(msg));
            }
            // Else an answer to another request.
            if msg.get("event").is_some() {
                self.events.push_back(msg);
            }
        }
    }

    /// Why a request couldn't be written (`e`): the process not taking it
    /// for [`ANSWER_WAIT`], or what it said before it closed the connection
    /// on it (a request too long to read, ADR 10.3), as its answer.
    fn unwritten(&mut self, e: io::Error) -> Result<Value, String> {
        if waited(&e) {
            return Err("not answering".into());
        }
        while let Ok(Some(msg)) = self.read(Instant::now().checked_add(ANSWER_WAIT)) {
            if unasked_error(&msg) {
                return Ok(msg);
            }
            if msg.get("event").is_some() {
                self.events.push_back(msg);
            }
        }
        Err(e.to_string())
    }

    /// The next event; `Ok(None)` when `deadline` passes, `Err` when the
    /// connection closes, but for a cut-off it has caught up on.
    pub(super) fn next_event(&mut self, deadline: Option<Instant>) -> Result<Option<Value>, Error> {
        loop {
            if let Some(event) = self.events.pop_front() {
                return Ok(Some(event));
            }
            match self.read(deadline) {
                Ok(None) => return Ok(None),
                Ok(Some(msg)) if msg.get("event").is_some() => return Ok(Some(msg)),
                Ok(Some(_)) => {} // A response nobody waits for.
                // Stopped by the facade's caller: nothing to catch up for.
                Err(why) if super::stopped() => return Err(why.into()),
                Err(why) => self.catch_up(why)?,
            }
        }
    }

    /// What is left to read before the connection ends, without catching
    /// up, why it ended, and whether the process cut it off: for `event
    /// notify`, which looks while its command runs (ADR 10.2).
    pub(super) fn rest(&mut self) -> (Vec<Value>, String, bool) {
        let mut rest: Vec<Value> = self.events.drain(..).collect();
        let why = loop {
            match self.read(None) {
                Ok(Some(msg)) if msg.get("event").is_some() => rest.push(msg),
                Ok(Some(_)) | Ok(None) => {}
                Err(why) => break why,
            }
        };
        let cut = match self.follow.as_ref() {
            Some(f) => {
                match connect(&f.socket).map_err(|e| e.to_string()).and_then(Conn::new) {
                    Ok(mut conn) => {
                        let asked = conn.was_cut_off(&f.peer);
                        drop(conn);
                        match asked {
                            Ok(cut) => cut.is_some(),
                            // Stopping (#275): its host log says, once it has
                            // gone.
                            Err(_) => {
                                gone_within(&f.socket, STOPPING_WAIT) && f.cut_off_before_it_went()
                            }
                        }
                    }
                    Err(_) => f.cut_off_before_it_went(),
                }
            }
            // Given up catching up, in a request: what it read meanwhile
            // is left, then what it missed.
            None => self.missed.is_some(),
        };
        (rest, why, cut)
    }

    /// Asks the process whether it cut `peer` off: if it did, the last
    /// position then (`last_queued_seq`), if it says; none if it didn't, or
    /// answers with an error. `Err` if it doesn't answer: it hung up, as one
    /// that is stopping does (#275).
    fn was_cut_off(&mut self, peer: &str) -> Result<Option<Option<u64>>, String> {
        let req = json!({ "cmd": "was_cut_off", "peer": peer });
        let asked = self.answer_until(req, Instant::now().checked_add(ANSWER_WAIT))?;
        let asked = asked.ok_or("not answering")?;
        let cut = asked["ok"] == true && asked["cut_off"] == true;
        Ok(cut.then(|| asked["last_queued_seq"].as_u64()))
    }

    /// The connection has ended (`why`) before the end the command waits
    /// for: if the process cut it off, what it missed, read from the
    /// transcript, is queued, followed by what comes live on a new
    /// subscription (ADR 10.2). `Err(why)` if it wasn't cut off; or, with
    /// `missed` saying what it missed, if it can't catch up. A new
    /// subscription that fails, or doesn't say where it starts, is one more
    /// cut-off: there is no position to catch up to, so it is tried again.
    fn catch_up(&mut self, why: String) -> Result<(), String> {
        let Some(mut f) = self.follow.take() else { return Err(why) };
        // Once the process has said it cut `f.peer` off: where it was then.
        let mut cut: Option<Option<u64>> = None;
        let (live, until) = loop {
            f.tries += 1;
            let fresh = connect(&f.socket).map_err(|e| e.to_string()).and_then(Conn::new);
            let mut conn = match fresh {
                // The same process: its errors there say how it differs
                // from this brnr, as here (ADR 8.8).
                Ok(conn) => Conn { meta: self.meta.clone(), ..conn },
                // Gone: its transcript is whole (ADR 5.5), and is read to its end.
                Err(_) if cut.is_some() || f.cut_off_before_it_went() => {
                    // Its socket goes before its last records are written.
                    if gone_within(&f.socket, STOPPING_WAIT) {
                        break (None, None);
                    }
                    return Err(why);
                }
                Err(_) => return Err(why),
            };
            if cut.is_none() {
                match conn.was_cut_off(&f.peer) {
                    Ok(Some(at)) => cut = Some(at),
                    Ok(None) => return Err(why),
                    // Hung up unanswered: a process that is stopping accepts
                    // until its socket goes (#275). Gone, its host log says
                    // whether it had cut `f.peer` off.
                    Err(_) => {
                        drop(conn);
                        if gone_within(&f.socket, STOPPING_WAIT) && f.cut_off_before_it_went() {
                            break (None, None);
                        }
                        return Err(why);
                    }
                }
            }
            match conn.resubscribe(&f) {
                Ok(Some((peer, until))) => {
                    f.peer = peer;
                    break (Some(conn), Some(until));
                }
                Ok(None) => break (None, None),
                Err((Some(at), e)) => return Err(self.give_up(&f, Some(at), e)),
                Err((None, e)) if f.tries > TRIES => {
                    // Live, as far as it knows: up to where the process
                    // said it cut the peer off.
                    let known = cut.flatten();
                    let n = f.tries;
                    let e = e.map(|m| {
                        let mut e = format!("cut off {n} times in a row while catching up ({m})");
                        if known.is_none() {
                            e.push_str("; the process says no position, so up to where is unknown");
                        }
                        e
                    });
                    return Err(self.give_up(&f, known, e));
                }
                Err((None, _)) => {}
            }
        };
        if f.tries > TRIES {
            let e = format!("cut off {} times in a row while catching up", f.tries);
            return Err(self.give_up(&f, until, e.into()));
        }
        let caught = match f.read(until) {
            Ok(caught) => caught,
            Err(e) => return Err(self.give_up(&f, until, e)),
        };
        f.last = until.or(caught.last().and_then(|e| e["seq"].as_u64())).unwrap_or(f.last);
        self.events.extend(caught);
        let Some(mut conn) = live else { return Ok(()) };
        if !conn.events.is_empty() {
            f.tries = 0;
        }
        // Live while it subscribed: read, so caught up on no more.
        if let Some(seq) = conn.events.iter().filter_map(|e| e["seq"].as_u64()).max() {
            f.last = f.last.max(seq);
        }
        self.events.append(&mut conn.events);
        std::mem::swap(&mut self.reader, &mut conn.reader);
        std::mem::swap(&mut self.writer, &mut conn.writer);
        self.reconnects += 1;
        self.follow = Some(f);
        Ok(())
    }

    /// Subscribes as `f` did, and waits for the process to have written what
    /// it recorded until then: the new subscription's peer, and where it
    /// starts, which the transcript has everything up to; none if the
    /// process went meanwhile, its transcript whole. `Err` says why it
    /// can't, and where the new subscription starts once it has one.
    fn resubscribe(&mut self, f: &Follow) -> Result<Option<Resubscribed>, (Option<u64>, Error)> {
        let events: Vec<&str> = f.events.iter().map(String::as_str).collect();
        let answer = self.call(json!({ "cmd": "subscribe", "events": events }));
        let answer = answer.map_err(|e| (None, e))?;
        let next = Follow::new(f.socket.clone(), &answer, &events, f.session.as_deref());
        let next = next.ok_or((None, "the process doesn't say where its events are".into()))?;
        let at = Some(next.last);
        match self.call(json!({ "cmd": "logged" })) {
            Ok(logged) if logged["lost"] == true => {
                Err((at, "the process has records it couldn't write yet (a full disk?)".into()))
            }
            Ok(_) => Ok(Some((next.peer, next.last))),
            // Hung up unanswered: a process that is stopping accepts until
            // its socket goes (#275), and gone, its transcript is whole. The
            // connection, of no more use, is let go of first.
            Err(e) => {
                let _ = self.writer.shutdown(std::net::Shutdown::Both);
                if gone_within(&f.socket, STOPPING_WAIT) { Ok(None) } else { Err((at, e)) }
            }
        }
    }

    /// Gives up catching up: `missed` says what it missed, and why.
    fn give_up(&mut self, f: &Follow, until: Option<u64>, why: error::Error) -> String {
        let note = f.missed(until, &why);
        let text = render::event(&note, &render::Options { session: false, time: false });
        self.missed = Some(note);
        text.unwrap_or(why.message)
    }

    /// The next line, an event or a response (or the start channel's
    /// report); an event moves on where a subscription is.
    pub(super) fn read(&mut self, deadline: Option<Instant>) -> Result<Option<Value>, String> {
        let timeout = deadline.map(|d| d.saturating_duration_since(Instant::now()));
        if timeout == Some(Duration::ZERO) {
            return Ok(None);
        }
        // Closed, it reads what is left, then EOF.
        closed_ok(self.reader.get_ref().set_read_timeout(timeout)).map_err(|e| e.to_string())?;
        let mut line = String::new();
        match self.reader.read_line(&mut line) {
            Ok(0) => Err("the process closed the connection".into()),
            // Cut short: the process cut the connection off mid-line.
            Ok(_) if !line.ends_with('\n') => Err("the process closed the connection".into()),
            Ok(_) => {
                // As brnr reads what it wrote (ADR 2.5): one too deep is
                // none the process makes, and is said; one that isn't JSON
                // is passed over.
                let line = line.as_bytes();
                json::within(line, json::OWN_DEPTH, "a line of the process's")?;
                let msg = json::parse(line, json::OWN_DEPTH).unwrap_or(Value::Null);
                if let (Some(f), Some(seq)) = (&mut self.follow, msg["seq"].as_u64())
                    && msg.get("event").is_some()
                {
                    (f.last, f.tries) = (seq, 0);
                }
                Ok(Some(msg))
            }
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    /// Where it catches up from, for a test to look at and move.
    #[cfg(test)]
    pub(super) fn follow(&mut self) -> Option<&mut Follow> {
        self.follow.as_mut()
    }

    /// The process's socket, for a test.
    #[cfg(test)]
    pub(super) fn socket_path(&self) -> Option<PathBuf> {
        self.socket.clone()
    }
}

/// A connection to a process, for a test, that the process cuts off,
/// unanswered, at its first `cmd` request, after one event more (at 11),
/// and that can't be caught up with: the process says it cut it off, and
/// that it has records it couldn't write (ADR 10.2).
#[cfg(test)]
pub(super) fn cut_off_at(cmd: &'static str) -> (Conn, std::thread::JoinHandle<()>) {
    use std::os::unix::net::UnixListener;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::{env, thread};
    static MADE: AtomicU32 = AtomicU32::new(0);
    let n = MADE.fetch_add(1, Ordering::Relaxed);
    let dir = env::temp_dir().join(format!("brnr-cut-{}-{n}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let socket = dir.join("h.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let process = thread::spawn(move || {
        for (k, stream) in (1..=2).zip(listener.incoming()) {
            let mut stream = stream.unwrap();
            let lines = BufReader::new(stream.try_clone().unwrap()).lines();
            for line in lines.map_while(Result::ok) {
                let req: Value = serde_json::from_str(&line).unwrap();
                let mut answer = match req["cmd"].as_str().unwrap() {
                    c if c == cmd => {
                        writeln!(stream, "{}", json!({ "event": "turn_ended", "seq": 11 }))
                            .unwrap();
                        break;
                    }
                    "was_cut_off" => json!({ "ok": true, "cut_off": true }),
                    "subscribe" => {
                        json!({ "ok": true, "peer": format!("socket#{k}"), "host_id": "h", "seq": 10 * k })
                    }
                    _ => json!({ "ok": true, "lost": true }),
                };
                answer["req_id"] = req["req_id"].clone();
                writeln!(stream, "{answer}").unwrap();
            }
        }
        let _ = fs::remove_dir_all(&dir);
    });
    let mut conn = Conn::new(UnixStream::connect(&socket).unwrap()).unwrap();
    conn.socket = Some(socket);
    (conn, process)
}

#[cfg(test)]
impl Conn {
    /// The same connection, as to the process `meta` describes: for a test.
    pub(super) fn of(mut self, meta: Value) -> Conn {
        self.meta = Some(meta);
        self
    }
}

/// A connection, for a test, to a process whose socket is `<pid>.sock`,
/// that answers each request with the next of `answers`, then closes it.
#[cfg(test)]
pub(super) fn answered(pid: u32, answers: Vec<Value>) -> (Conn, std::thread::JoinHandle<()>) {
    use std::os::unix::net::UnixListener;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::{env, thread};
    static MADE: AtomicU32 = AtomicU32::new(0);
    let n = MADE.fetch_add(1, Ordering::Relaxed);
    let dir = env::temp_dir().join(format!("brnr-answered-{}-{n}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let socket = dir.join(format!("{pid}.sock"));
    let listener = UnixListener::bind(&socket).unwrap();
    let process = thread::spawn(move || {
        let mut stream = listener.accept().unwrap().0;
        let mut lines = BufReader::new(stream.try_clone().unwrap()).lines();
        for mut answer in answers {
            let Some(Ok(line)) = lines.next() else { break };
            let req: Value = serde_json::from_str(&line).unwrap();
            answer["req_id"] = req["req_id"].clone();
            writeln!(stream, "{answer}").unwrap();
        }
        drop(stream);
        let _ = fs::remove_dir_all(&dir);
    });
    let mut conn = Conn::new(UnixStream::connect(&socket).unwrap()).unwrap();
    conn.socket = Some(socket);
    (conn, process)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::code_is;
    use std::env;
    use std::os::unix::net::UnixListener;
    use std::thread;

    /// A process, 7, that answers each connection's request with the next of
    /// `answers`, as it is, and what it was asked.
    fn answering(name: &str, answers: Vec<String>) -> (Host, thread::JoinHandle<Vec<Value>>) {
        let dir = env::temp_dir().join(format!("brnr-client-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let socket = dir.join("7.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let process = thread::spawn(move || {
            let mut asked = Vec::new();
            for (answer, stream) in answers.into_iter().zip(listener.incoming()) {
                let mut stream = stream.unwrap();
                let mut line = String::new();
                BufReader::new(&stream).read_line(&mut line).unwrap();
                asked.push(serde_json::from_str(&line).unwrap());
                writeln!(stream, "{answer}").unwrap();
            }
            let _ = fs::remove_dir_all(&dir);
            asked
        });
        let meta = json!({ "id": "7", "socket": socket, "protocol": PROTOCOL });
        (Host { meta, status: None }, process)
    }

    /// An error from a process that speaks another protocol than this
    /// brnr's says so, and how to restart it, where it may come of the
    /// difference: a request it doesn't have, or, from one that predates
    /// `protocol`, any error, as its errors are text (ADR 8.8). One that
    /// speaks this brnr's gets its errors as they are.
    #[test]
    fn adr_0008_an_error_from_another_brnrs_process_says_how_to_restart_it() {
        let unknown =
            r#"{"ok":false,"error":{"code":"command_unknown","message":"unknown command: fork"}}"#;
        let missing =
            r#"{"ok":false,"error":{"code":"session_not_found","message":"no session s"}}"#;
        let text = r#"{"ok":false,"error":"unknown command: fork"}"#;
        let answers = [unknown, unknown, missing, unknown, text];
        let (mut host, process) = answering("skew", answers.map(String::from).to_vec());
        let fork = json!({ "cmd": "fork", "session": "s" });
        let said = |host: &Host| host.call(&fork).unwrap_err();
        let error = said(&host);
        code_is(&error, Code::command_unknown);
        assert_eq!(error, Error::new(Code::command_unknown, "unknown command: fork"));

        let newer = PROTOCOL + 1;
        host.meta["protocol"] = json!(newer);
        host.meta["brnr"] = json!("9.0.0");
        let ours = format!("brnr {}, protocol {PROTOCOL}", env!("CARGO_PKG_VERSION"));
        let restart = "stop it (brnr process stop 7) and resume its sessions (brnr session \
                       resume <session>) to run them in this brnr";
        let note = format!(
            "process 7 runs a newer brnr (brnr 9.0.0, protocol {newer}) than this one ({ours}): \
             {restart}"
        );
        let error = said(&host);
        code_is(&error, Code::command_unknown);
        assert_eq!(error.message, format!("unknown command: fork; {note}"));
        // Not of the difference: as it is.
        let error = said(&host);
        code_is(&error, Code::session_not_found);
        assert_eq!(error, Error::new(Code::session_not_found, "no session s"));

        // An editor's, from before processes said their version.
        host.meta = json!({ "id": "7", "socket": host.meta["socket"], "proxy_pid": 5 });
        let note = format!(
            "process 7 runs an older brnr than this one ({ours}), from before processes said \
             theirs: restart the agent in its editor to run this brnr"
        );
        assert_eq!(said(&host).message, format!("unknown command: fork; {note}"));
        let error = said(&host);
        code_is(&error, Code::unclassified);
        assert_eq!(
            (error.code, error.message),
            (Code::unclassified, format!("unknown command: fork; {note}"))
        );
        process.join().unwrap();
    }

    /// What `call` gets is the answer without its envelope; an error the
    /// process says is the command's as it says it; and an answer that isn't
    /// JSON, or nested past what brnr reads of its own, is refused, naming
    /// the process.
    #[test]
    fn a_call_is_answered_or_says_why_not() {
        let n = json::OWN_DEPTH;
        let deep = format!(r#"{{"ok":true,"x":{}{}}}"#, "[".repeat(n), "]".repeat(n));
        let answers = [
            r#"{"ok":true,"req_id":"r","status":"stopping"}"#,
            r#"{"ok":false,"error":{"code":"session_not_found","message":"no session s"}}"#,
            r#"{"ok":true"#,
            &deep,
        ];
        let (host, process) = answering("call", answers.map(String::from).to_vec());
        assert_eq!(host.call(&json!({ "cmd": "stop" })), Ok(json!({ "status": "stopping" })));
        let none = Error::new(Code::session_not_found, "no session s");
        assert_eq!(host.call(&json!({ "cmd": "stop" })), Err(none));
        let isnt = Err("process 7: its answer isn't JSON".into());
        assert_eq!(host.call_agent(&json!({ "cmd": "fork" })), isnt);
        let deep = Err("process 7: its answer nests 257 deep, past the 256 brnr reads".into());
        assert_eq!(host.call(&json!({ "cmd": "queue" })), deep);
        let asked: Vec<Value> =
            process.join().unwrap().into_iter().map(|r| r["cmd"].clone()).collect();
        assert_eq!(asked, ["stop", "stop", "fork", "queue"]);
    }

    /// `pending` is read as deep as brnr writes it, what the agent sent at
    /// the depth limit and a few levels of the host's own (ADR 2.5), and
    /// finds a request by its session and handle: an elicitation outside any
    /// session by none. One level more is refused, and said.
    #[test]
    fn adr_0002_an_answer_is_read_to_the_limit_and_no_deeper() {
        for depth in [json::OWN_DEPTH - 4, json::OWN_DEPTH - 3] {
            let input = format!("{}{}", "[".repeat(depth), "]".repeat(depth));
            let answer = format!(
                r#"{{"ok":true,"pending":[{{"session":"s","request":"p1","tool_call":{{"rawInput":{input}}}}},{{"session":"t","request":"p1"}}],"elicitations":[{{"session":null,"request":"e1"}},{{"session":"s","request":"e2"}}]}}"#
            );
            let (host, process) = answering(&format!("pending-{depth}"), vec![answer]);
            let found = host.pending().map(|pending| {
                // Each as `<session>:<request>`, `-` for none.
                let named = |items: &mut dyn Iterator<Item = &Value>| -> Vec<String> {
                    let name = |v: &Value| v["session"].as_str().unwrap_or("-").to_owned();
                    items
                        .map(|v| format!("{}:{}", name(v), v["request"].as_str().unwrap()))
                        .collect()
                };
                (
                    pending.approval("s", "p1").map(|p| p["tool_call"]["rawInput"].is_array()),
                    named(&mut pending.approvals(None)),
                    named(&mut pending.approvals(Some("t"))),
                    named(&mut pending.elicitations(None)),
                    named(&mut pending.elicitations(Some("s"))),
                    [(None, "e1"), (Some("s"), "e1"), (Some("s"), "e2")]
                        .map(|(session, request)| pending.elicitation(session, request).is_some()),
                )
            });
            let want = (
                Some(true),
                vec!["s:p1".to_owned(), "t:p1".to_owned()],
                vec!["t:p1".to_owned()],
                vec!["-:e1".to_owned(), "s:e2".to_owned()],
                vec!["s:e2".to_owned()],
                [true, false, true],
            );
            match depth {
                d if d + 4 == json::OWN_DEPTH => assert_eq!(found, Ok(want)),
                _ => {
                    let past = "process 7: its answer nests 257 deep, past the 256 brnr reads";
                    assert_eq!(found.err().map(|e| e.message).as_deref(), Some(past));
                }
            }
            assert_eq!(process.join().unwrap(), [json!({ "cmd": "pending" })]);
        }
    }

    /// A `status` answer nested past serde_json's 128, as one with an
    /// agent's turn error at the depth limit is, is the process answering
    /// (#323), so `discover` lists it as it is.
    #[test]
    fn adr_0002_a_status_with_an_error_at_the_limit_is_an_answer() {
        let n = json::DEPTH - 1;
        let error =
            format!(r#"{{"code":-32603,"message":"m","data":{}{}}}"#, "[".repeat(n), "]".repeat(n));
        let status = format!(r#"{{"ok":true,"sessions":[{{"last_turn":{{"error":{error}}}}}]}}"#);
        assert!(json::depth(status.as_bytes()) > json::DEPTH);
        let (host, process) = answering("status-deep", vec![status]);
        match probe(Path::new("/nonexistent/7.json"), &host) {
            Probe::Answered(status) => {
                assert_eq!(status["sessions"][0]["last_turn"]["error"]["code"], -32603);
            }
            Probe::Silent(why) => panic!("silent: {why}"),
            Probe::Gone => panic!("gone"),
        }
        assert_eq!(process.join().unwrap(), [json!({ "cmd": "status" })]);
    }

    /// A process that cuts every new connection off again once it has
    /// answered `was_cut_off`, `subscribe` (the k-th at `10 k`) and `logged`,
    /// the first after one event more (at `10 k + 1`): a command gives up
    /// once it is cut off more than `TRIES` times in a row with nothing read
    /// live in between, and says what it missed.
    #[test]
    fn adr_0010_a_cut_off_while_catching_up_is_tried_again_then_given_up() {
        let dir = env::temp_dir().join(format!("brnr-tries-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let (socket, host_log) = (dir.join("h.sock"), dir.join("host.jsonl"));
        fs::write(&host_log, "").unwrap();
        let listener = UnixListener::bind(&socket).unwrap();
        let process = thread::spawn(move || {
            let mut served = 0;
            for (k, stream) in (1..).zip(listener.incoming()) {
                let mut stream = stream.unwrap();
                served = k;
                let mut lines = BufReader::new(stream.try_clone().unwrap()).lines();
                while let Some(Ok(line)) = lines.next() {
                    let req: Value = serde_json::from_str(&line).unwrap();
                    let answer = match req["cmd"].as_str().unwrap() {
                        "was_cut_off" => json!({ "ok": true, "cut_off": true }),
                        "subscribe" => {
                            json!({ "ok": true, "peer": format!("socket#{k}"), "host_id": "h", "seq": 10 * k })
                        }
                        _ => json!({ "ok": true, "lost": false }),
                    };
                    let mut answer = answer;
                    answer["req_id"] = req["req_id"].clone();
                    writeln!(stream, "{answer}").unwrap();
                    if req["cmd"] == "logged" {
                        if k == 1 {
                            let event = json!({ "event": "turn_ended", "seq": 10 * k + 1 });
                            writeln!(stream, "{event}").unwrap();
                        }
                        break;
                    }
                }
                if k == TRIES as u64 + 2 {
                    break;
                }
            }
            served
        });
        // Cut off before it read anything.
        let (ours, theirs) = UnixStream::pair().unwrap();
        drop(theirs);
        let mut conn = Conn::new(ours).unwrap();
        conn.follow = Some(Follow {
            socket,
            host_id: "h".into(),
            pid: json!(7),
            host_log,
            peer: "socket#0".into(),
            events: vec!["turn_ended".into()],
            session: None,
            last: 0,
            tries: 0,
        });
        let live = conn.next_event(None).unwrap().unwrap();
        assert_eq!(live["seq"], 11);
        assert!(conn.next_event(None).is_err());
        let note = conn.missed().unwrap();
        let error = &note["error"]["message"];
        assert_eq!(error, "cut off 4 times in a row while catching up", "{note}");
        assert_eq!((note["after_seq"].as_u64(), note["until_seq"].as_u64()), (Some(40), Some(50)));
        assert_eq!(process.join().unwrap(), 5, "connections after the first");
        let _ = fs::remove_dir_all(&dir);
    }

    /// An event that comes live while a command subscribes again (12, after
    /// `subscribe` at 11) is one it has read: cut off once more before the
    /// next line, it catches up after 12, so it gets each event once, in
    /// order (11 to 13, all in the transcript).
    #[test]
    fn adr_0010_an_event_live_while_subscribing_again_comes_once() {
        let dir = env::temp_dir().join(format!("brnr-once-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let (socket, host_log) = (dir.join("h.sock"), dir.join("host.jsonl"));
        let event = |seq: u64| json!({ "event": "turn_ended", "seq": seq });
        let lines: String = (11..=13)
            .map(|n| {
                format!(
                    "{}\n",
                    json!({ "format": 1, "ts": "t", "host_id": "h", "event": event(n) })
                )
            })
            .collect();
        fs::write(&host_log, lines).unwrap();
        let listener = UnixListener::bind(&socket).unwrap();
        // The k-th connection subscribes at `10 + k`; 12 comes live on the
        // first between `subscribe` and `logged`, which cuts it off, 13 on
        // the second after `logged`.
        let process = thread::spawn(move || {
            for (k, stream) in (1..=2).zip(listener.incoming()) {
                let mut stream = stream.unwrap();
                let lines = BufReader::new(stream.try_clone().unwrap()).lines();
                for line in lines.map_while(Result::ok) {
                    let req: Value = serde_json::from_str(&line).unwrap();
                    let mut answer = match req["cmd"].as_str().unwrap() {
                        "was_cut_off" => json!({ "ok": true, "cut_off": true }),
                        "subscribe" => {
                            json!({ "ok": true, "peer": format!("socket#{k}"), "host_id": "h", "seq": 10 + k })
                        }
                        _ => json!({ "ok": true, "lost": false }),
                    };
                    answer["req_id"] = req["req_id"].clone();
                    writeln!(stream, "{answer}").unwrap();
                    match (k, req["cmd"].as_str().unwrap()) {
                        (1, "subscribe") => writeln!(stream, "{}", event(12)).unwrap(),
                        (1, "logged") => break,
                        (_, "logged") => {
                            writeln!(stream, "{}", event(13)).unwrap();
                            break;
                        }
                        _ => {}
                    }
                }
            }
        });
        // Cut off before it read anything.
        let (ours, theirs) = UnixStream::pair().unwrap();
        drop(theirs);
        let mut conn = Conn::new(ours).unwrap();
        conn.follow = Some(Follow {
            socket,
            host_id: "h".into(),
            pid: json!(7),
            host_log,
            peer: "socket#0".into(),
            events: vec!["turn_ended".into()],
            session: None,
            last: 10,
            tries: 0,
        });
        let mut seqs = Vec::new();
        while let Ok(Some(e)) = conn.next_event(None) {
            seqs.push(e["seq"].as_u64().unwrap());
        }
        assert_eq!(seqs, [11, 12, 13]);
        assert_eq!(conn.reconnects, 2);
        process.join().unwrap();
        let _ = fs::remove_dir_all(&dir);
    }

    /// A process has gone once it has exited, its pid naming its socket,
    /// whether its socket is there or not: one that has removed it may still
    /// be writing its last records. A zombie nobody has reaped has exited,
    /// though `kill(pid, 0)` still reaches it.
    #[test]
    fn adr_0010_a_process_has_gone_once_it_has_exited() {
        let dir = env::temp_dir().join(format!("brnr-gone-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let socket = |pid: u32| dir.join(format!("{pid}.sock"));
        let wait = Duration::from_millis(100);
        assert!(!gone_within(&socket(std::process::id()), wait));
        let mut zombie = std::process::Command::new("true").spawn().unwrap();
        assert!(gone_within(&socket(zombie.id()), Duration::from_secs(5)));
        assert!(sys::kill(zombie.id() as libc::pid_t, 0), "reaped already");
        zombie.wait().unwrap();
        assert!(gone_within(&socket(zombie.id()), wait));
        let _ = fs::remove_dir_all(&dir);
    }

    /// A process that left its metadata is gone once it has exited, though
    /// nothing has reaped it: `kill(pid, 0)` still reaches a zombie, and on
    /// macOS it started before its file was written (#313). One that lives,
    /// its socket not refusing, isn't.
    #[test]
    fn adr_0003_a_zombie_process_is_gone() {
        let dir = env::temp_dir().join(format!("brnr-zombie-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let mut zombie = std::process::Command::new("true").spawn().unwrap();
        let pid = zombie.id();
        let (socket, file) = (dir.join(format!("{pid}.sock")), dir.join(format!("{pid}.json")));
        assert!(gone_within(&socket, Duration::from_secs(5)), "never a zombie");
        assert!(sys::kill(pid as libc::pid_t, 0), "reaped already");
        fs::write(&file, "{}").unwrap();
        assert!(gone(pid.into(), &socket, &file, &io::Error::from(ErrorKind::NotFound)));
        let timed_out = io::Error::from(ErrorKind::TimedOut);
        assert!(!gone(std::process::id().into(), &socket, &file, &timed_out));
        zombie.wait().unwrap();
        let _ = fs::remove_dir_all(&dir);
    }

    /// A process that is stopping accepts, and hangs up with no answer,
    /// until its socket goes (#275). A peer it had cut off waits for it to
    /// go, then reads what it missed from the transcript to its end, where
    /// it failed with "the process closed the connection".
    #[test]
    fn adr_0010_a_peer_cut_off_by_a_stopping_process_catches_up() {
        let dir = env::temp_dir().join(format!("brnr-stopping-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let (socket, host_log) = (dir.join("h.sock"), dir.join("host.jsonl"));
        let record = |event: Value| {
            format!("{}\n", json!({ "format": 1, "ts": "t", "host_id": "h", "event": event }))
        };
        let dropped = record(json!({ "event": "peer_dropped", "peer": "socket#0" }));
        let events: String =
            (11..=12).map(|n| record(json!({ "event": "turn_ended", "seq": n }))).collect();
        fs::write(&host_log, dropped + &events).unwrap();
        let listener = UnixListener::bind(&socket).unwrap();
        // Three connections hung up on, then it goes.
        let process = thread::spawn(move || {
            for stream in listener.incoming().take(3) {
                drop(stream.unwrap());
            }
        });
        let (ours, theirs) = UnixStream::pair().unwrap();
        drop(theirs);
        let mut conn = Conn::new(ours).unwrap();
        conn.follow = Some(Follow {
            socket,
            host_id: "h".into(),
            pid: json!(7),
            host_log,
            peer: "socket#0".into(),
            events: vec!["turn_ended".into()],
            session: None,
            last: 10,
            tries: 0,
        });
        let mut seqs = Vec::new();
        while let Ok(Some(e)) = conn.next_event(None) {
            seqs.push(e["seq"].as_u64().unwrap());
        }
        assert_eq!(seqs, [11, 12]);
        process.join().unwrap();
        let _ = fs::remove_dir_all(&dir);
    }

    /// A new subscription that fails, or doesn't say where it starts, is one
    /// more cut-off, tried again under the same limit. A process that goes
    /// meanwhile (its socket refuses) is caught up with to its end, so that
    /// `until` is null only for a process that has gone. One that stays
    /// gives up at the 4th, `until` where it said it cut the peer off
    /// (`last_queued_seq`, 30), or null with the error saying why if it said no
    /// position. An older brnr's errors there say how to restart it.
    #[test]
    fn adr_0010_a_new_subscription_that_fails_is_one_more_cut_off() {
        // A process that says it cut the peer off (`last_queued_seq`) and fails
        // each new subscription (no `seq`, or an error), then, after
        // `serves` connections, goes or (`stays`) only stops answering.
        let process = |name: &str, last_queued: Option<u64>, serves: usize, stays: bool| {
            let dir = env::temp_dir().join(format!("brnr-resub-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            let (socket, host_log) = (dir.join("h.sock"), dir.join("host.jsonl"));
            let exited = json!({ "event": "process_exited", "exit_status": 0, "seq": 12 });
            let record = json!({ "format": 1, "ts": "t", "host_id": "h", "event": exited });
            fs::write(&host_log, format!("{record}\n")).unwrap();
            let listener = UnixListener::bind(&socket).unwrap();
            let at = socket.clone();
            let thread = thread::spawn(move || {
                let mut asked = Vec::new();
                for (k, stream) in (1..=serves).zip(listener.incoming()) {
                    let mut stream = stream.unwrap();
                    let lines = BufReader::new(stream.try_clone().unwrap()).lines();
                    for line in lines.map_while(Result::ok) {
                        let req: Value = serde_json::from_str(&line).unwrap();
                        asked.push(format!("{k}:{}", req["cmd"].as_str().unwrap()));
                        let mut answer = match req["cmd"].as_str().unwrap() {
                            "was_cut_off" => {
                                json!({ "ok": true, "cut_off": true, "last_queued_seq": last_queued })
                            }
                            "subscribe" if k % 2 == 1 => {
                                json!({ "ok": true, "peer": format!("socket#{k}"), "host_id": "h" })
                            }
                            _ => json!({ "ok": false, "error": "connection gone" }),
                        };
                        answer["req_id"] = req["req_id"].clone();
                        // Gone before the command hears, so that it can't
                        // connect again.
                        if req["cmd"] == "subscribe" && k == serves && !stays {
                            fs::remove_file(&at).unwrap();
                        }
                        writeln!(stream, "{answer}").unwrap();
                        if req["cmd"] == "subscribe" {
                            break;
                        }
                    }
                }
                asked
            });
            // Cut off before it read anything.
            let (ours, theirs) = UnixStream::pair().unwrap();
            drop(theirs);
            let mut conn = Conn::new(ours).unwrap();
            conn.follow = Some(Follow {
                socket,
                host_id: "h".into(),
                pid: json!(7),
                host_log,
                peer: "socket#0".into(),
                events: vec!["process_exited".into()],
                session: None,
                last: 10,
                tries: 0,
            });
            (conn, thread, dir)
        };
        let tries = TRIES as usize + 1;
        let all: Vec<String> = (1..=tries).map(|k| format!("{k}:subscribe")).collect();
        let once = |asked: Vec<String>, served: usize| {
            let mut want = vec!["1:was_cut_off".to_owned()];
            want.extend(all.iter().take(served).cloned());
            assert_eq!(asked, want, "was_cut_off asked once, subscribe on each connection");
        };

        // Gone after two: caught up to its end from the transcript.
        let (mut conn, thread, dir) = process("gone", Some(30), 2, false);
        let caught = conn.next_event(None).unwrap().unwrap();
        assert_eq!((&caught["event"], &caught["seq"]), (&json!("process_exited"), &json!(12)));
        assert!(conn.missed().is_none());
        once(thread.join().unwrap(), 2);
        let _ = fs::remove_dir_all(&dir);

        // Still running: gives up at the 4th, up to where it was cut off.
        for (name, last_queued) in [("stays", Some(30)), ("nowhere", None)] {
            let (mut conn, thread, dir) = process(name, last_queued, tries, true);
            assert!(conn.next_event(None).is_err());
            let note = conn.missed().expect("no events_missed note").clone();
            assert_eq!(
                (note["after_seq"].as_u64(), note["until_seq"].as_u64()),
                (Some(10), last_queued),
                "{note}"
            );
            let error = note["error"]["message"].as_str().unwrap();
            assert!(error.starts_with("cut off 4 times in a row while catching up"), "{note}");
            assert_eq!(error.contains("no position"), last_queued.is_none(), "{note}");
            once(thread.join().unwrap(), tries);
            let _ = fs::remove_dir_all(&dir);
        }

        // An older brnr's, which says no `protocol`: its errors on the new
        // connections say how to restart it, as on the first (ADR 8.8).
        let (mut conn, thread, dir) = process("older", Some(30), tries, true);
        conn.meta = Some(json!({ "id": "7" }));
        assert!(conn.next_event(None).is_err());
        let note = conn.missed().expect("no events_missed note").clone();
        let error = note["error"]["message"].as_str().unwrap();
        assert!(error.contains("connection gone; process 7 runs an older brnr"), "{note}");
        assert!(error.contains("stop it (brnr process stop 7)"), "{note}");
        once(thread.join().unwrap(), tries);
        let _ = fs::remove_dir_all(&dir);
    }

    /// A process that stops while a command subscribes again, and hangs up
    /// before it answers `logged`, has its transcript whole once gone: the
    /// command catches up to its end, past where it subscribed (20), and
    /// misses nothing (#329).
    #[test]
    fn adr_0010_a_process_that_goes_while_subscribing_again_is_caught_up_to_its_end() {
        let dir = env::temp_dir().join(format!("brnr-relogged-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let (socket, host_log) = (dir.join("h.sock"), dir.join("host.jsonl"));
        let exited = json!({ "event": "process_exited", "exit_status": 0, "seq": 22 });
        let record = json!({ "format": 1, "ts": "t", "host_id": "h", "event": exited });
        fs::write(&host_log, format!("{record}\n")).unwrap();
        let listener = UnixListener::bind(&socket).unwrap();
        let at = socket.clone();
        let process = thread::spawn(move || {
            let mut asked = Vec::new();
            let mut stream = listener.incoming().next().unwrap().unwrap();
            let lines = BufReader::new(stream.try_clone().unwrap()).lines();
            for line in lines.map_while(Result::ok) {
                let req: Value = serde_json::from_str(&line).unwrap();
                asked.push(req["cmd"].as_str().unwrap().to_owned());
                let mut answer = match req["cmd"].as_str().unwrap() {
                    "was_cut_off" => json!({ "ok": true, "cut_off": true, "last_queued_seq": 30 }),
                    "subscribe" => {
                        json!({ "ok": true, "peer": "socket#1", "host_id": "h", "seq": 20 })
                    }
                    // Stopping: gone, its socket first, before it answers.
                    _ => {
                        fs::remove_file(&at).unwrap();
                        break;
                    }
                };
                answer["req_id"] = req["req_id"].clone();
                writeln!(stream, "{answer}").unwrap();
            }
            asked
        });
        // Cut off before it read anything.
        let (ours, theirs) = UnixStream::pair().unwrap();
        drop(theirs);
        let mut conn = Conn::new(ours).unwrap();
        conn.follow = Some(Follow {
            socket,
            host_id: "h".into(),
            pid: json!(7),
            host_log,
            peer: "socket#0".into(),
            events: vec!["process_exited".into()],
            session: None,
            last: 10,
            tries: 0,
        });
        let caught = conn.next_event(None).unwrap().unwrap();
        assert_eq!((&caught["event"], &caught["seq"]), (&json!("process_exited"), &json!(22)));
        assert!(conn.missed().is_none(), "{:?}", conn.missed());
        assert_eq!(process.join().unwrap(), ["was_cut_off", "subscribe", "logged"]);
        let _ = fs::remove_dir_all(&dir);
    }

    /// Processes that don't answer cost `discover` one wait in all, not one
    /// each: they are asked at once. Each is listed, as not answering (no
    /// status), beside one that answers, in the order they started
    /// (ADR 10.4).
    #[test]
    fn adr_0010_processes_not_answering_cost_one_wait_in_all() {
        let dir = env::temp_dir().join(format!("brnr-probes-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        paths::ensure_private(&dir).unwrap();
        let mut listeners = Vec::new();
        for n in 1..=4 {
            let socket = dir.join(format!("{n}.sock"));
            listeners.push(UnixListener::bind(&socket).unwrap());
            let started = format!("2026-01-0{}", 5 - n);
            let meta = json!({ "id": n.to_string(), "socket": socket, "host_pid": std::process::id(), "started": started });
            fs::write(dir.join(format!("{n}.json")), meta.to_string()).unwrap();
        }
        // 1 and 2 never accept, 3 accepts and never answers, 4 answers.
        let answers = listeners.pop().unwrap();
        thread::spawn(move || {
            let mut stream = answers.accept().unwrap().0;
            BufReader::new(&stream).read_line(&mut String::new()).unwrap();
            writeln!(stream, r#"{{"ok":true,"sessions":[]}}"#).unwrap();
        });
        let silent = listeners.pop().unwrap();
        thread::spawn(move || {
            let held = silent.accept().unwrap();
            thread::sleep(ANSWER_WAIT * 3);
            drop(held);
        });
        let start = Instant::now();
        let hosts = discover_in(&dir).unwrap();
        let took = start.elapsed();
        assert!(took < ANSWER_WAIT + PROBE_MARGIN + Duration::from_secs(1), "{took:?}");
        let listed: Vec<(&str, bool)> =
            hosts.iter().map(|h| (h.id(), h.status.is_some())).collect();
        assert_eq!(listed, [("4", true), ("3", false), ("2", false), ("1", false)]);
        drop(listeners);
        let _ = fs::remove_dir_all(&dir);
    }

    /// A request the process never answers fails after [`ANSWER_WAIT`], "not
    /// answering", as one it takes nothing of does, rather than waiting for
    /// ever (ADR 10.4).
    #[test]
    fn adr_0010_a_request_not_answered_or_taken_is_not_answering() {
        let timed = |conn: &mut Conn| {
            let start = Instant::now();
            (conn.call(json!({ "cmd": "status" })), start.elapsed())
        };
        let (silent, _held) = UnixStream::pair().unwrap();
        let mut silent = Conn::new(silent).unwrap();
        // Its socket's buffer full: nothing more is taken.
        let (full, _unread) = UnixStream::pair().unwrap();
        full.set_nonblocking(true).unwrap();
        while (&full).write(&[b'x'; 4096]).is_ok() {}
        full.set_nonblocking(false).unwrap();
        let mut full = Conn::new(full).unwrap();
        thread::scope(|s| {
            let answered = s.spawn(|| timed(&mut silent));
            let taken = s.spawn(|| timed(&mut full));
            for (result, took) in [answered.join().unwrap(), taken.join().unwrap()] {
                assert_eq!(result, Err("not answering".into()));
                assert!(took >= ANSWER_WAIT - Duration::from_millis(100), "{took:?}");
                assert!(took < ANSWER_WAIT + Duration::from_secs(2), "{took:?}");
            }
        });
    }

    /// An error the process says to no request in particular, before it
    /// closes the connection (a request too long to read, too many
    /// connections), is the answer to the request waiting, on a connection
    /// and for a single request alike (ADR 10.3, 10.4).
    #[test]
    fn adr_0010_an_error_said_to_no_request_is_the_answer() {
        let error = r#"{"ok":false,"error":{"code":"process_unreachable","message":"too many connections: 128 open"}}"#;
        let (ours, mut theirs) = UnixStream::pair().unwrap();
        writeln!(theirs, "{error}").unwrap();
        drop(theirs);
        let mut conn = Conn::new(ours).unwrap();
        let answer = conn.call(json!({ "cmd": "subscribe" }));
        let refused = Error::new(Code::process_unreachable, "too many connections: 128 open");
        assert_eq!(answer, Err(refused.clone()));
        let (host, process) = answering("unasked", vec![error.into()]);
        assert_eq!(host.call(&json!({ "cmd": "status" })), Err(refused));
        process.join().unwrap();
    }

    /// A live process that can't be caught up with (here, it has records it
    /// couldn't write) has the note say up to where: where the command
    /// subscribed again (at 20); null is for a process that has gone.
    #[test]
    fn adr_0010_a_live_process_that_cant_be_caught_up_with_says_until_where() {
        let (mut conn, process) = cut_off_at("status");
        conn.subscribe(&["turn_ended"], None).unwrap();
        assert!(conn.call(json!({ "cmd": "status" })).is_err());
        let note = conn.missed().unwrap();
        assert_eq!(
            (note["after_seq"].as_u64(), note["until_seq"].as_u64()),
            (Some(11), Some(20)),
            "{note}"
        );
        process.join().unwrap();
    }
}
