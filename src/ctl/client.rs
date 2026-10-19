//! How the commands reach a running brnr process: over its control socket
//! (ADR 3.9), each answer read as the host reads the agent's line and used
//! on a stack that takes it (ADR 2.5), a subscription caught up once the
//! process cuts it off (ADR 10.2).
//!
//! [`discover`] finds the processes, asking each for its status, one after
//! another. A [`Host`] is one of them: [`Host::call`] asks it something and
//! waits for the answer, [`Host::pending`] has what waits for an answer
//! there, and [`Conn::open`] subscribes to its events and asks it things on
//! the same connection. An answer is the process's `ok` and `error` taken
//! off: an error it says is the command's. What the agent sent nests as
//! deep as the agent made it, so what holds it (`pending`, events) is used
//! in a closure, run on a stack that takes it, where it is let go of too;
//! what [`Host::call`] returns is brnr's own, and is refused past
//! [`json::SHALLOW`], what any stack takes.

use std::collections::VecDeque;
use std::fs;
use std::io::{self, BufRead, BufReader, ErrorKind, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::host::alive;
use crate::{json, lock, paths, render};

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
    pub(super) fn call(&self, req: &Value) -> Result<Value, String> {
        self.kept(req, ANSWER_WAIT)
    }

    /// [`Host::call`], for a request that waits on the agent.
    pub(super) fn call_agent(&self, req: &Value) -> Result<Value, String> {
        self.kept(req, AGENT_WAIT)
    }

    /// The answer to `req`, waiting `wait` for it, kept: as deep as any
    /// stack takes, as brnr's own answers are.
    fn kept(&self, req: &Value, wait: Duration) -> Result<Value, String> {
        self.ask(req, wait, keep)?.map_err(|e| format!("process {}: {e}", self.id()))
    }

    /// What waits for an answer in the process, for `f`, which runs on a
    /// stack that takes it (ADR 2.5).
    pub(super) fn pending<R: Send>(
        &self,
        f: impl FnOnce(&Pending) -> R + Send,
    ) -> Result<R, String> {
        self.ask(&json!({ "cmd": "pending" }), ANSWER_WAIT, |answer| f(&Pending::from(answer)))
    }

    /// Asks `req`, waiting `wait` for the answer, and runs `f` with it (see
    /// [`read`]). `Err`: the process's error, or why there is no
    /// answer to use, which names the process.
    fn ask<R: Send>(
        &self,
        req: &Value,
        wait: Duration,
        f: impl FnOnce(Value) -> R + Send,
    ) -> Result<R, String> {
        let named = |e: String| format!("process {}: {e}", self.id());
        let line = self.exchange(req, wait).map_err(|e| named(e.to_string()))?;
        read(&line, f).map_err(named)?
    }

    /// A request and the line that answers it, waiting up to `timeout` for
    /// it.
    fn exchange(&self, req: &Value, timeout: Duration) -> io::Result<String> {
        let mut conn = self.connect()?;
        conn.set_read_timeout(Some(timeout))?;
        writeln!(conn, "{req}")?;
        let mut line = String::new();
        BufReader::new(conn).read_line(&mut line).map_err(|e| match e.kind() {
            ErrorKind::WouldBlock | ErrorKind::TimedOut => io::Error::other("not answering"),
            _ => e,
        })?;
        Ok(line)
    }

    fn connect(&self) -> io::Result<UnixStream> {
        let socket =
            self.meta["socket"].as_str().ok_or(io::Error::other("no socket in metadata"))?;
        UnixStream::connect(PathBuf::from(socket))
    }
}

/// Runs `f` with the answer `line` without its envelope, read as the host
/// reads the agent's line (see json.rs), on a stack that takes it, where
/// `f` lets go of it. `Err`: why it can't be read; `Ok(Err)`: the process's
/// `error`.
fn read<R: Send>(
    line: &str,
    f: impl FnOnce(Value) -> R + Send,
) -> Result<Result<R, String>, String> {
    let line = line.as_bytes();
    let depth = json::depth(line);
    let read = json::on_stack(depth, || {
        let answer = json::parse(line).ok_or("its answer isn't JSON")?;
        Ok(unwrapped(answer).map(f))
    });
    read.unwrap_or_else(|| Err(format!("no stack to be had for an answer {depth} deep")))
}

/// An answer, `ok`, without `ok` and `req_id`; else the `error` it says.
fn unwrapped(mut answer: Value) -> Result<Value, String> {
    if answer["ok"].as_bool() != Some(true) {
        return Err(answer["error"].as_str().unwrap_or("request failed").to_owned());
    }
    if let Some(map) = answer.as_object_mut() {
        map.shift_remove("ok");
        map.shift_remove("req_id");
    }
    Ok(answer)
}

/// An answer to keep past the stack it was read on: one nested deeper than
/// any stack takes is refused, there.
fn keep(answer: Value) -> Result<Value, String> {
    match json::nesting([&answer]) {
        depth if depth > json::SHALLOW => Err(format!("its answer is {depth} deep")),
        _ => Ok(answer),
    }
}

/// Session `id` in a process's `status` answer, if it serves it.
pub(super) fn session_in<'a>(status: &'a Value, id: &str) -> Option<&'a Value> {
    status["sessions"].as_array()?.iter().find(|s| s["session_id"] == id)
}

/// What waits for an answer in a process, as its `pending` answer has it:
/// permission requests and elicitations, as deep as the agent made them.
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

/// Every process with a metadata file, asking each for its live status.
/// Files left by a process that is gone are removed, even when its pid is
/// another process's now (see `gone`). The runtime directory must be
/// private, as the processes themselves require: anyone who could write to
/// it could list a process of their own, and be sent what brnr sends.
pub(super) fn discover() -> Result<Vec<Host>, String> {
    let dir = paths::runtime_dir();
    match paths::check_private(&dir) {
        Ok(()) => {}
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("{}: {e}", dir.display())),
    }
    let Ok(entries) = fs::read_dir(&dir) else { return Ok(Vec::new()) };
    let mut hosts = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        let Some(meta) = read_meta(&path) else { continue };
        let mut host = Host { meta, status: None };
        match probe(&path, &host) {
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
        Ok(line) => match read(&line, keep) {
            Ok(Ok(Ok(status))) => Probe::Answered(status),
            Ok(Ok(Err(why)) | Err(why)) | Err(why) => Probe::Silent(why),
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
/// wrapping around). A pid that isn't the user's isn't brnr's (`alive`).
pub(super) fn gone(pid: i64, socket: &Path, file: &Path, err: &io::Error) -> bool {
    if !alive(pid) {
        return true;
    }
    if !refused(err) {
        return false;
    }
    std::thread::sleep(REFUSED_AGAIN);
    match UnixStream::connect(socket) {
        Err(e) if refused(&e) => !made(pid, file),
        _ => false,
    }
}

/// How long a process that hangs up on a request unanswered has to go: one
/// that is stopping writes what it has left before its socket goes.
const STOPPING_WAIT: Duration = Duration::from_secs(10);

/// Whether the process whose socket is `socket` has gone, within `wait`. A
/// refusal alone isn't enough: on macOS a full backlog refuses too, so its
/// metadata beside the socket says whose it is (`gone`); with none left,
/// a refusal is.
fn gone_within(socket: &Path, wait: Duration) -> bool {
    let meta = socket.with_extension("json");
    let pid = read_meta(&meta).and_then(|m| m["host_pid"].as_i64());
    let until = Instant::now() + wait;
    loop {
        match UnixStream::connect(socket) {
            Err(e) if refused(&e) && pid.is_none_or(|pid| gone(pid, socket, &meta, &e)) => {
                return true;
            }
            _ if Instant::now() >= until => return false,
            _ => std::thread::sleep(REFUSED_AGAIN),
        }
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
/// connection (ADR 10.2, see catch_up.rs). What it reads may nest as deep as
/// the agent made it: [`Conn::deep`] uses it and lets go of it on a stack
/// that takes it (ADR 2.5).
pub(super) struct Conn {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
    events: VecDeque<Value>,
    /// How deep what it read, and may still hold in `events`, nests.
    deepest: usize,
    /// How deep the stack it runs on takes (see `deep`).
    stack: usize,
    next_req: u64,
    /// The process's socket, to subscribe on again.
    socket: Option<PathBuf>,
    /// Subscribed: where it is, to catch up from.
    follow: Option<Follow>,
    /// What it missed, once it has given up catching up.
    missed: Option<Value>,
    /// How many times it has connected again.
    reconnects: u32,
}

impl Conn {
    pub(super) fn open(host: &Host) -> Result<Conn, String> {
        let stream = host.connect().map_err(|e| format!("process {}: {e}", host.id()))?;
        let mut conn = Conn::new(stream)?;
        conn.socket = host.meta["socket"].as_str().map(PathBuf::from);
        Ok(conn)
    }

    /// One over `stream`: a control connection, or the start channel.
    pub(super) fn new(stream: UnixStream) -> Result<Conn, String> {
        let writer = stream.try_clone().map_err(|e| e.to_string())?;
        Ok(Conn {
            reader: BufReader::new(stream),
            writer,
            events: VecDeque::new(),
            deepest: 0,
            stack: json::SHALLOW,
            next_req: 0,
            socket: None,
            follow: None,
            missed: None,
            reconnects: 0,
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
    ) -> Result<(), String> {
        let answer = self.call(json!({ "cmd": "subscribe", "events": events }))?;
        if let Some(socket) = self.socket.clone() {
            self.follow = Follow::new(socket, &answer, events, session);
        }
        Ok(())
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

    /// How a command ends that has got `Err(why)` from the connection: with
    /// [`MISSED`] once it has said what it missed, if it gave up catching
    /// up; else as `why`.
    pub(super) fn ended(&self, why: String) -> Result<ExitCode, String> {
        let Some(note) = &self.missed else { return Err(why) };
        let text = render::event(note, &render::Options { session: false, time: false });
        errln!("brnr: {}", text.unwrap_or_default());
        Ok(ExitCode::from(MISSED))
    }

    /// Runs `f` with `value` on a stack that takes it and what the connection
    /// holds: this one, or a thread's of its own (see json.rs). `f` lets go of
    /// `value` there. `Err` says there is no such stack to be had (P3); then
    /// `value` is kept until the process exits, rather than let go of here.
    pub(super) fn deep<R: Send>(
        &mut self,
        value: Value,
        f: impl FnOnce(&mut Conn, Value) -> R + Send,
    ) -> Result<R, String> {
        let need = json::nesting([&value]).max(self.deepest);
        let mut value = Some(value);
        let result = if need <= self.stack {
            Some(f(self, value.take().unwrap()))
        } else {
            let outer = self.stack;
            let result = json::on_stack(need, || {
                self.stack = need;
                f(self, value.take().unwrap())
            });
            self.stack = outer;
            result
        };
        // Not taken: no stack to be had, to let go of it on either.
        std::mem::forget(value);
        // What was deep may be gone, as in the host's `deep`.
        if self.deepest > json::SHALLOW {
            self.deepest = json::nesting(&self.events);
        }
        result.ok_or_else(|| format!("no stack to be had for a message {need} deep"))
    }

    /// A request whose failure is an error, and its answer.
    pub(super) fn call(&mut self, req: Value) -> Result<Value, String> {
        self.call_until(req, None)?.ok_or_else(|| "the process closed the connection".into())
    }

    /// What waits for an answer in the process, for `f`, which runs on a
    /// stack that takes it, as [`Conn::deep`] runs.
    pub(super) fn pending<R: Send>(
        &mut self,
        f: impl FnOnce(&Pending) -> R + Send,
    ) -> Result<R, String> {
        let answer = self.call(json!({ "cmd": "pending" }))?;
        self.deep(answer, |_, answer| f(&Pending::from(answer)))
    }

    /// The events read and not yet taken, oldest first.
    pub(super) fn queued(&self) -> impl DoubleEndedIterator<Item = &Value> {
        self.events.iter()
    }

    /// A request whose failure is an error, answered by `deadline`;
    /// `Ok(None)` when it passes first. One that only asks (`status`,
    /// `pending`) is asked again once caught up, if the process cut the
    /// connection off before it answered.
    pub(super) fn call_until(
        &mut self,
        mut req: Value,
        deadline: Option<Instant>,
    ) -> Result<Option<Value>, String> {
        self.next_req += 1;
        let id = format!("r{}", self.next_req);
        req["req_id"] = json!(id);
        writeln!(self.writer, "{req}").map_err(|e| e.to_string())?;
        let again = matches!(req["cmd"].as_str(), Some("status" | "pending"));
        loop {
            let msg = match self.read(deadline) {
                Ok(Some(msg)) => msg,
                Ok(None) => return Ok(None),
                Err(why) if again && self.follow.is_some() => {
                    self.catch_up(why)?;
                    writeln!(self.writer, "{req}").map_err(|e| e.to_string())?;
                    continue;
                }
                Err(why) => return Err(why),
            };
            if msg["req_id"] == id.as_str() {
                return unwrapped(msg).map(Some);
            }
            if msg.get("event").is_some() {
                self.events.push_back(msg);
            } else {
                json::free(msg); // An answer to another request.
            }
        }
    }

    /// The next event; `Ok(None)` when `deadline` passes, `Err` when the
    /// connection closes, but for a cut-off it has caught up on.
    pub(super) fn next_event(
        &mut self,
        deadline: Option<Instant>,
    ) -> Result<Option<Value>, String> {
        loop {
            if let Some(event) = self.events.pop_front() {
                return Ok(Some(event));
            }
            match self.read(deadline) {
                Ok(None) => return Ok(None),
                Ok(Some(msg)) if msg.get("event").is_some() => return Ok(Some(msg)),
                Ok(Some(msg)) => json::free(msg), // A response nobody waits for.
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
                Ok(Some(msg)) => json::free(msg),
                Ok(None) => {}
                Err(why) => break why,
            }
        };
        let cut = match self.follow.as_ref() {
            Some(f) => {
                match UnixStream::connect(&f.socket).map_err(|e| e.to_string()).and_then(Conn::new)
                {
                    Ok(mut conn) => match conn.was_cut_off(&f.peer) {
                        Ok(cut) => cut.is_some(),
                        // Stopping (#275): its host log says, once it has gone.
                        Err(_) => {
                            gone_within(&f.socket, STOPPING_WAIT) && f.cut_off_before_it_went()
                        }
                    },
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
    /// position then (`last_queued`), if it says; none if it didn't, or
    /// doesn't answer.
    fn was_cut_off(&mut self, peer: &str) -> Result<Option<Option<u64>>, String> {
        let asked = self.call(json!({ "cmd": "was_cut_off", "peer": peer }))?;
        Ok((asked["cut_off"] == true).then(|| asked["last_queued"].as_u64()))
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
            let fresh =
                UnixStream::connect(&f.socket).map_err(|e| e.to_string()).and_then(Conn::new);
            let mut conn = match fresh {
                Ok(conn) => conn,
                // Gone: its transcript is whole (ADR 5.5), and is read to its end.
                Err(_) if cut.is_some() || f.cut_off_before_it_went() => break (None, None),
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
                Ok((peer, until)) => {
                    f.peer = peer;
                    break (Some(conn), Some(until));
                }
                Err((Some(at), e)) => return Err(self.give_up(&f, Some(at), &e)),
                Err((None, e)) if f.tries > TRIES => {
                    // Live, as far as it knows: up to where the process
                    // said it cut the peer off.
                    let known = cut.flatten();
                    let n = f.tries;
                    let mut e = format!("cut off {n} times in a row while catching up ({e})");
                    if known.is_none() {
                        e.push_str("; the process says no position, so up to where is unknown");
                    }
                    return Err(self.give_up(&f, known, &e));
                }
                Err((None, _)) => {}
            }
        };
        if f.tries > TRIES {
            let e = format!("cut off {} times in a row while catching up", f.tries);
            return Err(self.give_up(&f, until, &e));
        }
        let caught = match f.read(until) {
            Ok(caught) => caught,
            Err(e) => return Err(self.give_up(&f, until, &e)),
        };
        f.last = until.or(caught.last().and_then(|e| e["seq"].as_u64())).unwrap_or(f.last);
        self.deepest = self.deepest.max(json::nesting(&caught));
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
        self.deepest = self.deepest.max(conn.deepest);
        std::mem::swap(&mut self.reader, &mut conn.reader);
        std::mem::swap(&mut self.writer, &mut conn.writer);
        self.reconnects += 1;
        self.follow = Some(f);
        Ok(())
    }

    /// Subscribes as `f` did, and waits for the process to have written what
    /// it recorded until then: the new subscription's peer, and where it
    /// starts, which the transcript has everything up to. `Err` says why it
    /// can't, and where the new subscription starts once it has one.
    fn resubscribe(&mut self, f: &Follow) -> Result<(String, u64), (Option<u64>, String)> {
        let events: Vec<&str> = f.events.iter().map(String::as_str).collect();
        let answer = self.call(json!({ "cmd": "subscribe", "events": events }));
        let answer = answer.map_err(|e| (None, e))?;
        let next = Follow::new(f.socket.clone(), &answer, &events, f.session.as_deref());
        let next = next.ok_or((None, "the process doesn't say where its events are".into()))?;
        let at = Some(next.last);
        if self.call(json!({ "cmd": "logged" })).map_err(|e| (at, e))?["lost"] == true {
            let why = "the process has records it couldn't write yet (a full disk?)";
            return Err((at, why.into()));
        }
        Ok((next.peer, next.last))
    }

    /// Gives up catching up: `missed` says what it missed, and why.
    fn give_up(&mut self, f: &Follow, until: Option<u64>, why: &str) -> String {
        let note = f.missed(until, why);
        let text = render::event(&note, &render::Options { session: false, time: false });
        self.missed = Some(note);
        text.unwrap_or_else(|| why.to_owned())
    }

    /// The next line, an event or a response (or the start channel's
    /// report); an event moves on where a subscription is.
    pub(super) fn read(&mut self, deadline: Option<Instant>) -> Result<Option<Value>, String> {
        let timeout = deadline.map(|d| d.saturating_duration_since(Instant::now()));
        if timeout == Some(Duration::ZERO) {
            return Ok(None);
        }
        // macOS refuses it on a connection the process has closed (EINVAL),
        // where reading doesn't wait anyway: it reads what is left, then EOF.
        if let Err(e) = self.reader.get_ref().set_read_timeout(timeout)
            && e.raw_os_error() != Some(libc::EINVAL)
        {
            return Err(e.to_string());
        }
        let mut line = String::new();
        match self.reader.read_line(&mut line) {
            Ok(0) => Err("the process closed the connection".into()),
            // Cut short: the process cut the connection off mid-line.
            Ok(_) if !line.ends_with('\n') => Err("the process closed the connection".into()),
            Ok(_) => {
                // As the host reads it (see json.rs): an event carries what
                // the agent sent as deep as it nested it (ADR 2.5).
                let line = line.as_bytes();
                let depth = json::depth(line);
                let msg = json::on_stack(depth, || json::parse::<Value>(line));
                let msg = msg.flatten().unwrap_or(Value::Null);
                self.deepest = self.deepest.max(depth);
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

impl Drop for Conn {
    /// What it still holds may be deep: let go of on a stack that takes it.
    fn drop(&mut self) {
        json::free(Value::Array(std::mem::take(&mut self.events).into()));
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
mod tests {
    use super::*;
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
        (Host { meta: json!({ "id": "7", "socket": socket }), status: None }, process)
    }

    /// What `call` gets is the answer without its envelope; an error the
    /// process says is the command's as it says it; and an answer that isn't
    /// JSON, or too deep to keep, is refused, naming the process.
    #[test]
    fn a_call_is_answered_or_says_why_not() {
        let deep = format!(r#"{{"ok":true,"x":{}{}}}"#, "[".repeat(200), "]".repeat(200));
        let answers = [
            r#"{"ok":true,"req_id":"r","status":"stopping"}"#,
            r#"{"ok":false,"error":"no session s"}"#,
            r#"{"ok":true"#,
            &deep,
        ];
        let (host, process) = answering("call", answers.map(String::from).to_vec());
        assert_eq!(host.call(&json!({ "cmd": "stop" })), Ok(json!({ "status": "stopping" })));
        assert_eq!(host.call(&json!({ "cmd": "stop" })), Err("no session s".into()));
        let isnt = Err("process 7: its answer isn't JSON".into());
        assert_eq!(host.call_agent(&json!({ "cmd": "fork" })), isnt);
        let deep = Err("process 7: its answer is 201 deep".into());
        assert_eq!(host.call(&json!({ "cmd": "queue" })), deep);
        let asked: Vec<Value> =
            process.join().unwrap().into_iter().map(|r| r["cmd"].clone()).collect();
        assert_eq!(asked, ["stop", "stop", "fork", "queue"]);
    }

    /// `pending` is read however deep what the agent sent nests (ADR 2.5),
    /// and finds a request by its session and handle: an elicitation outside
    /// any session by none.
    #[test]
    fn adr_0002_pending_is_read_however_deep() {
        for depth in [200, 1_000_000] {
            let input = format!("{}{}", "[".repeat(depth), "]".repeat(depth));
            let answer = format!(
                r#"{{"ok":true,"pending":[{{"session":"s","request":"p1","tool_call":{{"rawInput":{input}}}}},{{"session":"t","request":"p1"}}],"elicitations":[{{"session":null,"request":"e1"}},{{"session":"s","request":"e2"}}]}}"#
            );
            let (host, process) = answering(&format!("pending-{depth}"), vec![answer]);
            let found = host.pending(|pending| {
                // Each as `<session>:<request>`, `-` for none.
                let named = |items: &mut dyn Iterator<Item = &Value>| -> Vec<String> {
                    let name = |v: &Value| v["session"].as_str().unwrap_or("-").to_owned();
                    items
                        .map(|v| format!("{}:{}", name(v), v["request"].as_str().unwrap()))
                        .collect()
                };
                (
                    pending.approval("s", "p1").map(|p| json::nesting([p])),
                    named(&mut pending.approvals(None)),
                    named(&mut pending.approvals(Some("t"))),
                    named(&mut pending.elicitations(None)),
                    named(&mut pending.elicitations(Some("s"))),
                    [(None, "e1"), (Some("s"), "e1"), (Some("s"), "e2")]
                        .map(|(session, request)| pending.elicitation(session, request).is_some()),
                )
            });
            let want = (
                Some(depth + 2),
                vec!["s:p1".to_owned(), "t:p1".to_owned()],
                vec!["t:p1".to_owned()],
                vec!["-:e1".to_owned(), "s:e2".to_owned()],
                vec!["s:e2".to_owned()],
                [true, false, true],
            );
            assert_eq!(found, Ok(want), "{depth}");
            assert_eq!(process.join().unwrap(), [json!({ "cmd": "pending" })]);
        }
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
        assert_eq!(note["error"], "cut off 4 times in a row while catching up", "{note}");
        assert_eq!((note["after"].as_u64(), note["until"].as_u64()), (Some(40), Some(50)));
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
            .map(|n| format!("{}\n", json!({ "ts": "t", "host_id": "h", "event": event(n) })))
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

    /// A socket that refuses is gone only if its process is: on macOS a
    /// full backlog refuses too, while the process (here, this one, whose
    /// metadata is written after it started) lives and may write more.
    #[cfg(target_os = "macos")]
    #[test]
    fn adr_0010_a_refusing_socket_whose_process_lives_isnt_gone() {
        let dir = env::temp_dir().join(format!("brnr-backlog-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let (socket, meta) = (dir.join("h.sock"), dir.join("h.json"));
        drop(UnixListener::bind(&socket).unwrap()); // Nobody listens: refused.
        let ours = json!({ "host_pid": std::process::id(), "socket": socket.to_string_lossy() });
        fs::write(&meta, ours.to_string()).unwrap();
        assert!(!gone_within(&socket, Duration::from_millis(100)));
        fs::remove_file(&meta).unwrap();
        assert!(gone_within(&socket, Duration::from_millis(100)));
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
        let record =
            |event: Value| format!("{}\n", json!({ "ts": "t", "host_id": "h", "event": event }));
        let dropped = record(json!({ "event": "peer-dropped", "peer": "socket#0" }));
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
    /// (`last_queued`, 30), or null with the error saying why if it said no
    /// position.
    #[test]
    fn adr_0010_a_new_subscription_that_fails_is_one_more_cut_off() {
        // A process that says it cut the peer off (`last_queued`) and fails
        // each new subscription (no `seq`, or an error), then, after
        // `serves` connections, goes or (`stays`) only stops answering.
        let process = |name: &str, last_queued: Option<u64>, serves: usize, stays: bool| {
            let dir = env::temp_dir().join(format!("brnr-resub-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            let (socket, host_log) = (dir.join("h.sock"), dir.join("host.jsonl"));
            let exited = json!({ "event": "exited", "status": 0, "seq": 12 });
            let record = json!({ "ts": "t", "host_id": "h", "event": exited });
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
                                json!({ "ok": true, "cut_off": true, "last_queued": last_queued })
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
                host_log,
                peer: "socket#0".into(),
                events: vec!["exited".into()],
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
        assert_eq!((&caught["event"], &caught["seq"]), (&json!("exited"), &json!(12)));
        assert!(conn.missed().is_none());
        once(thread.join().unwrap(), 2);
        let _ = fs::remove_dir_all(&dir);

        // Still running: gives up at the 4th, up to where it was cut off.
        for (name, last_queued) in [("stays", Some(30)), ("nowhere", None)] {
            let (mut conn, thread, dir) = process(name, last_queued, tries, true);
            assert!(conn.next_event(None).is_err());
            let note = conn.missed().expect("no events-missed note").clone();
            assert_eq!(
                (note["after"].as_u64(), note["until"].as_u64()),
                (Some(10), last_queued),
                "{note}"
            );
            let error = note["error"].as_str().unwrap();
            assert!(error.starts_with("cut off 4 times in a row while catching up"), "{note}");
            assert_eq!(error.contains("no position"), last_queued.is_none(), "{note}");
            once(thread.join().unwrap(), tries);
            let _ = fs::remove_dir_all(&dir);
        }
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
            (note["after"].as_u64(), note["until"].as_u64()),
            (Some(11), Some(20)),
            "{note}"
        );
        process.join().unwrap();
    }
}
