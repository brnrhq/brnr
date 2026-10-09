//! Bridges: everything that talks to the host in JSON lines instead of ACP.
//!
//! A bridge is a process the host starts from the profile's `bridges`
//! (requests on its stdout, responses and events on its stdin), or anything
//! that connects to the control socket, such as brnr. Both speak the same
//! protocol, one JSON object per line. A started bridge is one until it
//! exits: its stdout closing only means it has no more requests. It should
//! exit when its stdin closes, which happens when the host stops; one still
//! running 2 s later gets SIGTERM.
//!
//! Requests: `{"cmd": …, "req_id"?: …}`; the response echoes `req_id`.
//! `session` is a session's exact id; the commands about a session need it.
//! - `status`
//! - `logged`: answered once what the process recorded before it was asked
//!   is in its transcript, which a logger thread writes (`brnr event log`,
//!   ADR 48)
//! - `send` `{session, text?, blocks?, mode?: prompt|steer|interrupt|context,
//!   replace?}` (ADR 18 in docs/adr): the response's `status` is
//!   `delivered`, `held` (until the running turn ends), `steered` (into it)
//!   or `interrupting`, and it has the message's id (`m<n>`), which the
//!   `user_message` of its turn carries, and the `turn_ended` in `messages`
//! - `cancel` `{session, keep_held?}`: cancel the running turn; held
//!   messages are dropped (and listed) unless `keep_held`
//! - `queue` `{session, drop?, clear?, clear_context?}`: the held messages
//!   and context, after removing what was asked
//! - `subscribe` `{events?: [...] | "all"}`: events follow on this connection
//!   (started bridges are subscribed from the start)
//! - `pending`: permission requests waiting for an answer, in full
//! - `allow` / `reject` `{session, request, always?, option?}`: with the
//!   option of kind `allow_once` (`allow_always` with `always`), or
//!   `reject_once` (`reject_always`), or the option named (ADR 63)
//! - `set_config` `{session, mode?, model?, thought_level?, options?: {id:
//!   value}}` (`config set`, ADR 63): resolved as a start's settings are
//!   (ADR 58), sent one at a time, and answered once the agent has set them
//!   all (`set`, what was sent) or refused one
//! - `fork` `{session}`: never in an editor's process
//! - `close` `{session, take_over?}`: cancels a running turn first, and is
//!   answered once the agent has closed the session; a headless process
//!   whose last session closes stops. `take_over` is the pid of the process
//!   `session resume --take-over` resumes it in
//! - `stop`: close the agent's stdin, then SIGTERM, then SIGKILL (the
//!   agent's process group)
//!
//! On an editor's session every command that acts on it (`send`, `cancel`,
//! `queue --clear-context`, `allow`, `reject`, `set_config` and `close`) is
//! experimental: refused unless the editor's profile enables it (ADR 4, see
//! experimental.rs). Bridges observe it freely.
//!
//! Events (`{"event": …, "ts", "host_id", …}`): see [`EVENTS`]. Subscribing
//! without a list gets every event except `acp`, which is busy (one per
//! streamed chunk) and must be asked for by name. A held message that goes
//! unsent (`cancel`, `queue`, its session closing, the agent exiting) is a
//! `message_dropped`; a session that closes, `session_closed`.
//!
//! Each peer's queue holds up to [`QUEUE_BYTES`]. A peer that lets it fill
//! up has stopped reading and is dropped rather than buffered for without
//! limit: a connection is shut down, a started bridge gets SIGTERM. The limit
//! is in bytes, not lines, so a burst of small events (an agent streaming
//! fast) doesn't look like a peer that stopped reading, and the line that
//! takes a peer past it doesn't count, nor (up to [`ASIDE_BYTES`] of them)
//! lines longer than the whole queue, so one long message doesn't either,
//! even when it comes twice, as its ACP message and its event (ADR 49, 60).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::Shutdown;
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::thread;
use std::time::{Instant, SystemTime};

use libc::pid_t;
use serde_json::{Value, json};

use super::acp::{Choice, Held};
use super::requests::{PeerOp, Setup, resolve_settings};
use super::strict::Beyond;
use super::{Ev, Host};
use crate::config::{Bridge, Experimental, Log};
use crate::log::{self, Dir};
use crate::request::Settings;
use crate::{json, paths, render, sys};

/// Every event name. `acp` (every ACP message the host passes on, with its
/// direction) is only sent to peers that ask for it by name.
pub const EVENTS: &[&str] = &[
    "user_message",
    "agent_message",
    "agent_thought",
    "tool_call",
    "tool_progress",
    "plan",
    "usage",
    "session_changed",
    "permission_request",
    "permission_resolved",
    "turn_ended",
    "message_dropped",
    "context_dropped",
    "session_closed",
    "history",
    "line_too_long",
    "exited",
    "acp",
];

/// Events brnr leaves out of what it shows unless asked for by name: the
/// ACP messages, the agent's thoughts, usage and tool calls' progress.
/// `watch` and `log` without `--events`, and a session in the foreground,
/// in text and JSON alike.
pub const QUIET: &[&str] = &["acp", "agent_thought", "usage", "tool_progress"];

/// Bytes queued for one peer before it counts as having stopped reading.
const QUEUE_BYTES: usize = 16 << 20;

/// Bytes of lines a peer past [`QUEUE_BYTES`] may have queued that don't
/// count toward it (ADR 60): two of the longest lines the host reads
/// (ADR 51), a message's ACP line and its event.
const ASIDE_BYTES: usize = 64 << 20;

pub(super) static NEXT_PEER: AtomicU64 = AtomicU64::new(1);

/// How to cut a peer off.
pub(super) enum Closer {
    Socket(UnixStream),
    Bridge(pid_t),
}

/// Lines for one peer, and how many bytes of them its writer hasn't written
/// yet.
#[derive(Clone)]
pub(super) struct Queue {
    tx: Sender<String>,
    queued: Arc<AtomicUsize>,
    /// While the backlog is past the limit, the size of the lines set aside,
    /// which don't count toward it: the one that took it past, and those
    /// longer than the limit since.
    aside: Arc<AtomicUsize>,
}

impl Queue {
    /// The queue, and the writer's end of it.
    pub(super) fn new() -> (Queue, Receiver<String>, Arc<AtomicUsize>) {
        let (tx, rx) = mpsc::channel();
        let queued = Arc::new(AtomicUsize::new(0));
        (Queue { tx, queued: queued.clone(), aside: Arc::default() }, rx, queued)
    }

    /// Full once the backlog is past the limit, not counting the lines set
    /// aside. Until then a peer takes the next line however big it is (a
    /// long agent message, a status), and that line doesn't make the line
    /// after it (the `turn_ended` after a turn's long `agent_message`) find
    /// a peer that keeps up behind (ADR 49). Nor does a line longer than the
    /// limit that comes while it is past it (the `agent_message` right after
    /// its ACP message, to a peer that asked for `acp`), up to
    /// [`ASIDE_BYTES`] set aside (ADR 60). At most the limit, the lines set
    /// aside (the first however long) and one more are queued.
    fn push(&self, line: String) -> Queued {
        let len = line.len() + 1;
        let queued = self.queued.load(Relaxed);
        if queued <= QUEUE_BYTES {
            self.aside.store(if queued + len > QUEUE_BYTES { len } else { 0 }, Relaxed);
        } else {
            let aside = self.aside.load(Relaxed);
            if queued - aside.min(queued) > QUEUE_BYTES {
                return Queued::Full;
            }
            if len > QUEUE_BYTES && aside + len <= ASIDE_BYTES {
                self.aside.store(aside + len, Relaxed);
            }
        }
        self.queued.fetch_add(len, Relaxed);
        match self.tx.send(line) {
            Ok(()) => Queued::Ok,
            Err(_) => Queued::Gone,
        }
    }
}

enum Queued {
    Ok,
    Gone,
    Full,
}

pub(super) struct Peer {
    tx: Queue,
    label: String,
    closer: Closer,
    pub(super) subscribed: bool,
    /// `None`: every event.
    pub(super) events: Option<Vec<String>>,
}

impl Peer {
    pub(super) fn new(tx: Queue, label: String, closer: Closer) -> Peer {
        Peer { tx, label, closer, subscribed: false, events: None }
    }

    fn wants(&self, event: &str) -> bool {
        self.subscribed
            && match &self.events {
                None => event != "acp",
                Some(events) => events.iter().any(|e| e == event),
            }
    }
}

pub fn check_bridge(bridge: &Bridge) -> Result<(), String> {
    if bridge.command.is_empty() {
        return Err("a bridge needs a command".into());
    }
    check_events(bridge.events.as_deref().unwrap_or_default())
}

fn check_events(events: &[String]) -> Result<(), String> {
    match events.iter().find(|e| !EVENTS.contains(&e.as_str())) {
        Some(bad) => Err(format!("unknown event {bad:?} (events: {})", EVENTS.join(", "))),
        None => Ok(()),
    }
}

// ---- connections ---------------------------------------------------------

pub(super) fn serve(listener: UnixListener, tx: SyncSender<Ev>) {
    for conn in listener.incoming() {
        let Ok(conn) = conn else { continue };
        let tx = tx.clone();
        thread::spawn(move || connection(conn, tx));
    }
}

fn connection(conn: UnixStream, tx: SyncSender<Ev>) {
    let (Ok(writer), Ok(closer)) = (conn.try_clone(), conn.try_clone()) else { return };
    let peer = NEXT_PEER.fetch_add(1, Relaxed);
    let (out_tx, out_rx, queued) = Queue::new();
    thread::spawn(move || write_lines(writer, out_rx, queued));
    let label = format!("socket#{peer}");
    let opened = Ev::PeerOpened { peer, tx: out_tx, label, closer: Closer::Socket(closer) };
    if tx.send(opened).is_err() {
        return;
    }
    read_requests(conn, peer, &tx);
    let _ = tx.send(Ev::PeerClosed { peer });
}

pub(super) fn write_lines(mut out: impl Write, lines: Receiver<String>, queued: Arc<AtomicUsize>) {
    for line in lines {
        if writeln!(out, "{line}").and_then(|()| out.flush()).is_err() {
            return;
        }
        queued.fetch_sub(line.len() + 1, Relaxed);
    }
}

/// Requests, one per line, until EOF. A line that isn't JSON (not even
/// UTF-8) is answered with an error; the peer stays.
fn read_requests(input: impl Read, peer: u64, tx: &SyncSender<Ev>) {
    let mut reader = BufReader::new(input);
    let mut line = Vec::new();
    while matches!(reader.read_until(b'\n', &mut line), Ok(n) if n > 0) {
        if !line.trim_ascii().is_empty() {
            let req = serde_json::from_slice::<Value>(&line)
                .unwrap_or_else(|e| json!({ "parse_error": e.to_string() }));
            if tx.send(Ev::PeerRequest { peer, req }).is_err() {
                return;
            }
        }
        line.clear();
    }
}

impl Host {
    /// Starts a bridge from the profile, subscribed to its events.
    pub(super) fn start_bridge(
        &mut self,
        n: usize,
        bridge: &Bridge,
        tx: &SyncSender<Ev>,
    ) -> Result<(), String> {
        // As the request has it: whoever started the process found a bare
        // name next to brnr, as they did the agent's (see request.rs), so
        // `brnr` is this brnr, whatever the editor's PATH.
        let program = Path::new(&bridge.command[0]);
        let name = program.file_name().unwrap_or(program.as_os_str()).to_string_lossy();
        let label = format!("{name}#{n}");
        let mut cmd = Command::new(program);
        cmd.args(&bridge.command[1..])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("BRNR_PID", self.info["id"].as_str().unwrap_or_default())
            .env("BRNR_SOCKET", &self.sock_path)
            // Out of a terminal's reach when the host is run by hand; it
            // exits when the host closes its stdin.
            .process_group(0);
        unsafe {
            // SAFETY: the closure runs in the child between fork and exec,
            // where only async-signal-safe calls may be made:
            // restore_for_child is an atomic load and signal(2), and allocates
            // nothing.
            cmd.pre_exec(|| {
                crate::signals::restore_for_child();
                Ok(())
            })
        };
        let mut child = cmd.spawn().map_err(|e| format!("bridge {}: {e}", bridge.command[0]))?;
        let pid = child.id() as pid_t;
        let peer = NEXT_PEER.fetch_add(1, Relaxed);
        let (out_tx, out_rx, queued) = Queue::new();
        let stdin = child.stdin.take().unwrap();
        thread::spawn(move || write_lines(stdin, out_rx, queued));
        // Its stdout closing ends its requests, not it (see `Ev::BridgeExited`).
        let stdout = child.stdout.take().unwrap();
        let t = tx.clone();
        thread::spawn(move || read_requests(stdout, peer, &t));
        let stderr = child.stderr.take().unwrap();
        let (t, l) = (tx.clone(), label.clone());
        thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if t.send(Ev::BridgeStderr { label: l.clone(), line }).is_err() {
                    return;
                }
            }
        });
        let (t, l) = (tx.clone(), label.clone());
        drop(child); // Reaped by the host (see `Ev::BridgeExited`).
        thread::spawn(move || {
            if super::wait_exited(pid).is_ok() {
                let _ = t.send(Ev::BridgeExited { label: l, pid, peer });
            }
        });

        let mut p = Peer::new(out_tx, label.clone(), Closer::Bridge(pid));
        p.subscribed = true;
        p.events = bridge.events.clone();
        self.peers.insert(peer, p);
        self.bridge_pids.push(pid);
        self.sink.note(None, json!({ "event": "bridge-started", "bridge": label, "pid": pid }));
        Ok(())
    }

    /// Logs an ACP message and shows it to peers subscribed to `acp`.
    pub(super) fn record(&mut self, session: Option<&str>, dir: Dir, bytes: &[u8]) {
        self.sink.msg(session, dir, bytes);
        if self.peers.values().any(|p| p.wants("acp")) {
            let body = bytes.strip_suffix(b"\n").unwrap_or(bytes);
            // As the host reads it, where this stack takes it (see json.rs).
            let msg = (json::depth(body) <= self.stack)
                .then(|| json::parse::<Value>(body))
                .flatten()
                .unwrap_or_else(|| Value::String(String::from_utf8_lossy(body).into_owned()));
            self.emit(json!({ "event": "acp", "dir": dir.name(), "session": session, "msg": msg }));
        }
    }

    /// An event of the host's own (no session) that happened to every
    /// session too, such as the agent exiting: emitted, and recorded in each
    /// session's transcript as well as the host log.
    pub(super) fn emit_to_sessions(&mut self, event: Value) {
        let event = self.emit(event);
        for s in &self.sessions {
            self.sink.note(Some(&s.id), event.clone());
        }
    }

    /// Sends `event` to every subscribed peer that wants it, and (but for
    /// `acp`, which is the raw transcript already) records it in the
    /// transcript, where `brnr event log` reads it back. Returns it as sent.
    pub(super) fn emit(&mut self, mut event: Value) -> Value {
        event["ts"] = json!(log::rfc3339(SystemTime::now()));
        event["host_id"] = json!(self.host_id);
        let name = event["event"].as_str().unwrap_or_default().to_owned();
        // History a load replays, which brnr had no transcript of (ADR 57).
        if let Some(i) = event["session"].as_str().and_then(|s| self.find(s))
            && self.sessions[i].replay.as_ref().is_some_and(|r| r.record)
        {
            event["replayed"] = json!(true);
        }
        let line = event.to_string();
        if name != "acp" {
            self.sink.note(event["session"].as_str(), event.clone());
            if let Some(i) = event["session"].as_str().and_then(|s| self.find(s)) {
                self.sessions[i].last_active = SystemTime::now();
            }
            if self.show_events
                && !QUIET.contains(&name.as_str())
                && let Some(display) = &self.display
            {
                // On the display's thread, which never holds the host up
                // (ADR 9).
                display.event(|| {
                    if self.json_events {
                        Some(render::clean(&line).into_owned())
                    } else {
                        render::event(&event, &render::Options::foreground())
                    }
                });
            }
        }
        let peers: Vec<u64> =
            self.peers.iter().filter(|(_, p)| p.wants(&name)).map(|(&id, _)| id).collect();
        for peer in peers {
            self.send_to(peer, line.clone());
        }
        event
    }

    /// Queues `line` for `peer`, dropping a peer that has gone or fallen
    /// behind.
    fn send_to(&mut self, peer: u64, line: String) {
        let Some(p) = self.peers.get(&peer) else { return };
        match p.tx.push(line) {
            Queued::Ok => {}
            Queued::Gone => {
                self.peers.remove(&peer);
            }
            Queued::Full => self.drop_peer(peer),
        }
    }

    /// Drops every peer, waiting up to `timeout` for their writers to send
    /// what's queued (dropping the queue lets a writer finish it and stop).
    pub(super) fn flush_peers(&mut self, timeout: std::time::Duration) {
        let queued: Vec<Arc<AtomicUsize>> =
            self.peers.values().map(|p| p.tx.queued.clone()).collect();
        self.peers.clear();
        let until = Instant::now() + timeout;
        while queued.iter().any(|q| q.load(Relaxed) > 0) && Instant::now() < until {
            thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    /// Cuts off a peer that stopped reading.
    fn drop_peer(&mut self, peer: u64) {
        let Some(p) = self.peers.remove(&peer) else { return };
        let event = json!({ "event": "peer-dropped", "peer": p.label, "reason": "fell behind" });
        self.sink.note(None, event);
        match p.closer {
            Closer::Socket(conn) => {
                let _ = conn.shutdown(Shutdown::Both);
            }
            Closer::Bridge(pid) if self.bridge_pids.contains(&pid) => {
                sys::kill(pid, libc::SIGTERM);
            }
            Closer::Bridge(_) => {}
        }
    }

    /// Answers a request, now or (for what the agent must answer first)
    /// once the agent has.
    pub(super) fn peer_request(&mut self, peer: u64, req: Value) {
        let req_id = req.get("req_id").cloned();
        match self.command(peer, &req) {
            Ok(Some(response)) => self.reply(peer, req_id, response),
            Ok(None) => {} // See requests.rs: answered when the agent answers.
            Err(e) => self.reply(peer, req_id, json!({ "ok": false, "error": e })),
        }
    }

    pub(super) fn reply(&mut self, peer: u64, req_id: Option<Value>, mut response: Value) {
        if let Some(req_id) = req_id {
            response["req_id"] = req_id;
        }
        self.send_to(peer, response.to_string());
    }

    /// `Ok(None)`: the answer comes from the agent, later.
    fn command(&mut self, peer: u64, req: &Value) -> Result<Option<Value>, String> {
        if let Some(err) = req.get("parse_error") {
            return Err(format!("bad request: {}", err.as_str().unwrap_or_default()));
        }
        let now = |v: Result<Value, String>| v.map(Some);
        match req["cmd"].as_str() {
            Some("status") => Ok(Some(self.status_report())),
            Some("logged") => {
                self.when_logged(peer, req.get("req_id").cloned());
                Ok(None)
            }
            Some("send") => now(self.send(req)),
            Some("cancel") => now(self.cancel_turn(req)),
            Some("queue") => now(self.queue(req)),
            Some("subscribe") => now(self.subscribe(peer, req)),
            Some("pending") => {
                Ok(Some(json!({ "ok": true, "pending": self.pending_permissions() })))
            }
            Some("allow") => now(self.answer(peer, req, Choice::Allow)),
            Some("reject") => now(self.answer(peer, req, Choice::Reject)),
            Some("set_config" | "fork" | "close") => self.agent_op(peer, req).map(|()| None),
            Some("stop") => {
                if self.status.is_some() {
                    return Err("the agent has already exited".into());
                }
                self.begin_stop();
                Ok(Some(json!({ "ok": true, "status": "stopping" })))
            }
            Some(other) => Err(format!("unknown command: {other}")),
            None => Err("missing cmd".into()),
        }
    }

    /// `logged`: answered from the logger's thread once it has written what
    /// the process recorded before it was asked, so that what a peer has
    /// seen happen is in the transcript (ADR 48). The event loop doesn't
    /// wait for it (P1); a peer gone meanwhile isn't answered.
    fn when_logged(&mut self, peer: u64, req_id: Option<Value>) {
        let Some(p) = self.peers.get(&peer) else { return };
        let queue = p.tx.clone();
        let mut response = json!({ "ok": true });
        if let Some(req_id) = req_id {
            response["req_id"] = req_id;
        }
        // A peer that is full is cut off by the event loop's next line to it.
        self.sink.when_written(move || {
            let _ = queue.push(response.to_string());
        });
    }

    fn subscribe(&mut self, peer: u64, req: &Value) -> Result<Value, String> {
        let events: Option<Vec<String>> = match req.get("events") {
            None | Some(Value::Null) => None,
            Some(Value::String(all)) if all == "all" => {
                Some(EVENTS.iter().map(|e| e.to_string()).collect())
            }
            Some(Value::Array(list)) => Some(
                list.iter()
                    .map(|e| e.as_str().map(str::to_owned).ok_or("events are strings"))
                    .collect::<Result<_, _>>()?,
            ),
            Some(_) => return Err("events must be a list".into()),
        };
        check_events(events.as_deref().unwrap_or_default())?;
        let p = self.peers.get_mut(&peer).ok_or("connection gone")?;
        p.subscribed = true;
        p.events = events.clone();
        let names = events.unwrap_or_else(|| {
            EVENTS.iter().filter(|e| **e != "acp").map(|e| e.to_string()).collect()
        });
        Ok(json!({ "ok": true, "events": names }))
    }

    fn answer(&mut self, peer: u64, req: &Value, choice: Choice) -> Result<Value, String> {
        let i = self.session_index(req)?;
        self.check_experimental(Experimental::Permission)?;
        let session = self.sessions[i].id.clone();
        let handle = req["request"].as_str().ok_or("missing request")?.to_owned();
        let ours = self
            .agent_requests
            .iter()
            .any(|r| r.handle.as_deref() == Some(&handle) && r.session.as_ref() == Some(&session));
        if !ours {
            return Err(format!("no pending request {handle} in session {session}"));
        }
        let by = self.peers.get(&peer).map_or("control".to_owned(), |p| p.label.clone());
        let always = req["always"].as_bool() == Some(true);
        let outcome =
            self.resolve_permission(&handle, choice, always, req["option"].as_str(), &by)?;
        Ok(json!({ "ok": true, "session": session, "request": handle, "outcome": outcome }))
    }

    fn pending_permissions(&self) -> Vec<Value> {
        let owner = if self.editor_attached() { "editor" } else { "headless" };
        // Whether `brnr permission allow` would be taken, and if not, why
        // (ADR 4).
        let why_not = self.check_experimental(Experimental::Permission).err();
        self.agent_requests
            .iter()
            .filter(|r| r.handle.is_some())
            .map(|r| {
                json!({
                    "request": r.handle,
                    "session": r.session,
                    "owner": owner,
                    "answerable": why_not.is_none(),
                    "why_not": why_not,
                    "title": r.params["toolCall"]["title"],
                    "kind": r.params["toolCall"]["kind"],
                    "tool_call": r.params["toolCall"],
                    "options": r.params["options"],
                    "timeout_seconds": r.deadline.map(|d| d.saturating_duration_since(Instant::now()).as_secs()),
                })
            })
            .collect()
    }

    fn status_report(&self) -> Value {
        let mut report = self.info.clone();
        report["ok"] = json!(true);
        report["owner"] = json!(if self.editor_attached() { "editor" } else { "headless" });
        report["pending"] = json!(self.pending_permissions().len());
        report["uptime_seconds"] = json!(self.started.elapsed().as_secs());
        report["stop_when_idle"] = json!(self.stop_when_idle.map(|d| d.as_secs()));
        report["stopping"] = json!(self.stop_requested);
        report["bridges"] = json!(
            self.peers
                .values()
                .filter(|p| !p.label.starts_with("socket#"))
                .map(|p| &p.label)
                .collect::<Vec<_>>()
        );
        report["sessions"] = (0..self.sessions.len())
            .map(|i| {
                let s = &self.sessions[i];
                let mut session = s.state.report();
                let pending = self
                    .agent_requests
                    .iter()
                    .filter(|r| r.handle.is_some() && r.session.as_ref() == Some(&s.id))
                    .count();
                let state = match (pending, s.prompts.is_empty()) {
                    (n, _) if n > 0 => "waiting",
                    (_, false) => "busy",
                    _ => "idle",
                };
                // Its transcript: events, and raw ACP (ADR 22).
                let events = paths::session_log(&s.cwd, &s.id);
                let acp = paths::acp_log(&events);
                let fields = json!({
                    "session_id": s.id,
                    "state": state,
                    "last_active": log::rfc3339(s.last_active),
                    "cwd": s.cwd.to_string_lossy(),
                    "busy": !s.prompts.is_empty(),
                    "turn_seconds": s.turn_started.map(|t| t.elapsed().as_secs()),
                    "prompts": s.prompts.len(),
                    "held": s.held.len(),
                    "context": s.context.len(),
                    "pending": pending,
                    "shared": s.shared(),
                    "lock_error": s.lock_error(),
                    "last_turn": s.last_turn,
                    "log": (self.logging != Log::Off).then(|| events.to_string_lossy()),
                    "acp_log": (self.logging == Log::All).then(|| acp.to_string_lossy()),
                });
                if let (Some(session), Value::Object(fields)) = (session.as_object_mut(), fields) {
                    session.extend(fields);
                }
                session
            })
            .collect();
        report
    }

    /// `send` (ADR 18): a prompt of its own, held while a turn runs; steered
    /// into the running turn; interrupting it; or context for the next
    /// prompt. A second prompt never goes while one runs.
    fn send(&mut self, req: &Value) -> Result<Value, String> {
        let text = req["text"].as_str().unwrap_or_default().to_owned();
        let blocks = match &req["blocks"] {
            Value::Null => Vec::new(),
            Value::Array(blocks) => blocks.clone(),
            _ => return Err("blocks must be a list of ACP content blocks".into()),
        };
        let mode = req["mode"].as_str().unwrap_or("prompt");
        if !matches!(mode, "prompt" | "steer" | "interrupt" | "context") {
            return Err(format!("unknown mode: {mode}"));
        }
        let replace = req["replace"].as_bool().unwrap_or(false);
        if text.trim().is_empty() && blocks.is_empty() {
            return Err("missing text".into());
        }
        if self.status.is_some() || self.agent_in.is_none() {
            return Err("the agent is no longer accepting input".into());
        }
        self.check_blocks(&blocks)?;
        let i = self.session_index(req)?;
        self.check_editor_send(i, mode)?;
        let session = self.sessions[i].id.clone();
        if mode == "context" {
            if !blocks.is_empty() {
                return Err("context is text only".into());
            }
            let context = &mut self.sessions[i].context;
            match context.last_mut() {
                Some(last) if replace => *last = text.clone(),
                _ => context.push(text.clone()),
            }
            self.sink.note(
                Some(&session),
                json!({ "event": "send", "mode": mode, "status": "held", "text": text }),
            );
            return Ok(json!({ "ok": true, "status": "held", "session": session }));
        }
        if !self.start_done {
            // The start commits before the agent gets any work (ADR 7).
            return Err("the session is still starting".into());
        }
        let s = &self.sessions[i];
        let busy = !s.prompts.is_empty();
        // Messages waiting to go (held, or steers the agent hasn't answered)
        // go first.
        let waiting = !s.held.is_empty() || !s.steering.is_empty();
        if mode == "steer" && (busy || waiting) {
            self.check_strict(Beyond::Steering)?;
            if !self.caps.steering {
                let why = "it doesn't advertise _session/steering";
                return Err(format!("the agent can't steer a running turn: {why}"));
            }
        }
        let held = Held { id: self.message_id(), text: text.clone(), blocks };
        let message = held.id.clone();
        let status = match mode {
            "prompt" if busy || waiting => {
                self.sessions[i].held.push_back(held);
                "held"
            }
            "steer" if busy || waiting => {
                self.steer(i, held);
                "steered"
            }
            "interrupt" if busy => {
                let s = &mut self.sessions[i];
                s.held.insert(s.interrupts, held);
                s.interrupts += 1;
                self.cancel(&session, "cancel");
                "interrupting"
            }
            _ => {
                self.send_prompt(i, held);
                "delivered"
            }
        };
        self.sink.note(
            Some(&session),
            json!({ "event": "send", "mode": mode, "status": status, "message": message, "text": text }),
        );
        Ok(json!({ "ok": true, "status": status, "session": session, "message": message }))
    }

    /// Content blocks a bridge may add to a prompt.
    pub(super) fn check_blocks(&self, blocks: &[Value]) -> Result<(), String> {
        for block in blocks {
            match block["type"].as_str() {
                Some("resource_link") if block["uri"].is_string() => {}
                Some("image") if block["data"].is_string() && block["mimeType"].is_string() => {
                    if !self.caps.image {
                        return Err("the agent doesn't take images".into());
                    }
                }
                Some("text") if block["text"].is_string() => {}
                _ => return Err(format!("unsupported content block: {block}")),
            }
        }
        Ok(())
    }

    /// `cancel`: stops the running turn. Held messages would otherwise start
    /// the next turn, so they are dropped unless `keep_held`.
    fn cancel_turn(&mut self, req: &Value) -> Result<Value, String> {
        let i = self.session_index(req)?;
        self.check_experimental(Experimental::Cancel)?;
        let session = self.sessions[i].id.clone();
        let dropped: Vec<Value> = if req["keep_held"].as_bool() == Some(true) {
            Vec::new()
        } else {
            self.drop_held(i, "cancel")
        };
        let busy = !self.sessions[i].prompts.is_empty();
        if busy {
            self.cancel(&session, "cancel");
        }
        let status = if busy { "cancelling" } else { "idle" };
        self.sink.note(Some(&session), json!({ "event": "cancel", "status": status }));
        Ok(json!({ "ok": true, "status": status, "session": session, "dropped": dropped }))
    }

    fn queue(&mut self, req: &Value) -> Result<Value, String> {
        let i = self.session_index(req)?;
        if req["clear_context"].as_bool() == Some(true) {
            self.check_experimental(Experimental::Context)?;
        }
        let s = &mut self.sessions[i];
        let mut dropped = Vec::new();
        if let Some(id) = req["drop"].as_str() {
            let pos =
                s.held.iter().position(|h| h.id == id).ok_or(format!("no held message {id}"))?;
            if pos < s.interrupts {
                s.interrupts -= 1;
            }
            dropped.push(s.held.remove(pos).unwrap());
        }
        if req["clear"].as_bool() == Some(true) {
            s.interrupts = 0;
            dropped.extend(s.held.drain(..));
        }
        let dropped = self.dropped(i, dropped, "queue");
        if req["clear_context"].as_bool() == Some(true) {
            self.drop_context(i, "queue");
        }
        let s = &self.sessions[i];
        let held: Vec<Value> = s
            .held
            .iter()
            .enumerate()
            .map(|(n, h)| json!({ "message": h.id, "text": h.text, "interrupt": n < s.interrupts, "attachments": h.blocks.len() }))
            .collect();
        Ok(
            json!({ "ok": true, "session": s.id, "held": held, "context": s.context, "dropped": dropped }),
        )
    }

    /// Requests the agent answers: they go out now and the bridge is
    /// answered when the agent is (see requests.rs).
    fn agent_op(&mut self, peer: u64, req: &Value) -> Result<(), String> {
        if self.status.is_some() || self.agent_in.is_none() {
            return Err("the agent is no longer accepting input".into());
        }
        let cmd = req["cmd"].as_str().unwrap_or_default();
        let req_id = req.get("req_id").cloned();
        let caps = self.caps;
        let i = self.session_index(req)?;
        let session = self.sessions[i].id.clone();
        match cmd {
            "set_config" => {
                self.check_experimental(Experimental::Config)?;
                let settings = settings_of(req)?;
                let state = &self.sessions[i].state;
                let steps = resolve_settings(&settings, &Settings::default(), state)?;
                if steps.is_empty() {
                    return Err("nothing to set".into());
                }
                self.run_setup(Setup {
                    session,
                    steps,
                    done: Vec::new(),
                    peer: Some((peer, req_id)),
                });
            }
            "fork" => {
                // A forked session would be a headless one in a process that
                // ends with the editor, which ACP can't tell of it (ADR 4).
                if self.editor_attached() {
                    return Err("the editor owns this process; fork sessions there".into());
                }
                self.check_strict(Beyond::Fork)?;
                if !caps.fork {
                    return Err("the agent can't fork sessions".into());
                }
                // A second session could never close when idle, and the
                // process would never stop (ADR 12).
                if self.stop_when_idle.is_some() && !caps.close {
                    return Err("the agent can't close sessions: with stop_when_idle, a forked \
                                session would never close"
                        .into());
                }
                let cwd = self.sessions[i].cwd.clone();
                let params = json!({ "sessionId": session, "cwd": cwd.to_string_lossy(), "mcpServers": self.mcp_servers });
                self.peer_op(peer, req_id, PeerOp::Fork { cwd }, "session/fork", params);
            }
            "close" => {
                self.check_experimental(Experimental::Close)?;
                if !caps.close {
                    return Err("the agent can't close sessions".into());
                }
                let taken_by = req["take_over"].as_u64().and_then(|pid| u32::try_from(pid).ok());
                self.close(i, peer, req_id, "close");
                self.closing_under_editor(&session, taken_by);
            }
            _ => unreachable!("checked in command"),
        }
        Ok(())
    }

    /// The session a request names, by its exact id. One that is closing
    /// takes no more requests.
    fn session_index(&self, req: &Value) -> Result<usize, String> {
        let wanted = req["session"].as_str().ok_or("missing session")?;
        let i = self.find(wanted).ok_or_else(|| format!("no session {wanted}"))?;
        if self.sessions[i].closing.is_some() {
            return Err(format!("{wanted} is closing"));
        }
        Ok(i)
    }
}

/// A `set_config`'s settings: `mode`, `model`, `thought_level`, and
/// `options` by id, each value a string.
fn settings_of(req: &Value) -> Result<Settings, String> {
    let text = |key: &str| match &req[key] {
        Value::Null => Ok(None),
        Value::String(v) => Ok(Some(v.clone())),
        _ => Err(format!("{key} isn't a string")),
    };
    let mut options = std::collections::BTreeMap::new();
    match &req["options"] {
        Value::Null => {}
        Value::Object(map) => {
            for (id, value) in map {
                let value = value.as_str().ok_or(format!("options: {id} isn't a string"))?;
                options.insert(id.clone(), value.to_owned());
            }
        }
        _ => return Err("options isn't an object".into()),
    }
    let (mode, model, thought_level) = (text("mode")?, text("model")?, text("thought_level")?);
    Ok(Settings { mode, model, thought_level, options })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lines of `n` bytes, newline included, into a queue nobody writes
    /// from, as for a peer that stopped reading: how many were taken before
    /// the first that wasn't.
    fn taken(queue: &Queue, sizes: &[usize]) -> usize {
        sizes.iter().take_while(|&&n| matches!(queue.push("x".repeat(n - 1)), Queued::Ok)).count()
    }

    #[test]
    fn adr_0060_a_message_and_its_acp_line_fit_at_once() {
        let (queue, _rx, _) = Queue::new();
        let big = 20_000_000;
        // Its ACP line, the turn's answer, its event, the turn's end.
        assert_eq!(taken(&queue, &[big, 200, big, 300]), 4);
    }

    #[test]
    fn adr_0060_a_peer_that_stopped_reading_is_cut_off_soon_after_the_limit() {
        let (queue, _rx, _) = Queue::new();
        assert_eq!(taken(&queue, &[1 << 20; 40]), 18, "the limit, the line past it, one more");

        // Lines longer than the limit are set aside up to ASIDE_BYTES.
        let (queue, _rx, queued) = Queue::new();
        let big = QUEUE_BYTES + 1;
        assert_eq!(taken(&queue, &[big; 10]), ASIDE_BYTES / big + 1, "set aside, and one more");
        assert!(queued.load(Relaxed) <= QUEUE_BYTES + ASIDE_BYTES + big);
    }
}
