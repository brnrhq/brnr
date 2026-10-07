//! Bridges: everything that talks to the host in JSON lines instead of ACP.
//!
//! A bridge is a process the host starts from the profile's `bridges`
//! (requests on its stdout, responses and events on its stdin), or anything
//! that connects to the control socket, such as brnr. Both speak the same
//! protocol, one JSON object per line. A started bridge is one until it
//! exits: its stdout closing only means it has no more requests. It should
//! exit when its stdin closes, which happens when the host stops; then it
//! gets SIGTERM.
//!
//! Requests: `{"cmd": …, "req_id"?: …}`; the response echoes `req_id`.
//! `session` is a session's exact id; the commands about a session need it.
//! - `status`
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
//! - `approve` / `deny` `{session, request, option?}`; only while no editor
//!   is attached
//! - `set_mode` `{session, mode}`, `set_config` `{session, option, value}`,
//!   `set_model` `{session, model}` (the config option whose category is
//!   `model`): answered once the agent has
//! - `fork` `{session}`, `close` `{session}`: only while no editor is
//!   attached. `close` cancels a running turn first, and is answered once
//!   the agent has closed the session; a headless process whose last
//!   session closes stops
//! - `stop`: close the agent's stdin, then SIGTERM, then SIGKILL (the
//!   agent's process group)
//!
//! Events (`{"event": …, "ts", "host_id", …}`): see [`EVENTS`]. Subscribing
//! without a list gets every event except `acp`, which is busy (one per
//! streamed chunk) and must be asked for by name. A held message that goes
//! unsent (`cancel`, `queue`, its session closing, the agent exiting) is a
//! `message_dropped`; a session that closes, `session_closed`. While an
//! editor is attached, bridges observe; the editor answers the agent.
//!
//! Each peer's queue holds up to [`QUEUE_BYTES`]. A peer that lets it fill
//! up has stopped reading and is dropped rather than buffered for without
//! limit: a connection is shut down, a started bridge gets SIGTERM. The limit
//! is in bytes, not lines, so a burst of small events (an agent streaming
//! fast) doesn't look like a peer that stopped reading.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::Shutdown;
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Instant, SystemTime};

use libc::pid_t;
use serde_json::{Value, json};

use super::acp::{Choice, Held};
use super::requests::PeerOp;
use super::strict::Beyond;
use super::{Ev, Host};
use crate::config::{Bridge, Log};
use crate::log::{self, Dir};
use crate::{json, paths, render, spawn};

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
    "session_closed",
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

pub(super) static NEXT_PEER: AtomicU64 = AtomicU64::new(1);

/// How to cut a peer off.
pub(super) enum Closer {
    Socket(UnixStream),
    Bridge(pid_t),
}

/// Lines for one peer, and how many bytes of them its writer hasn't written
/// yet.
pub(super) struct Queue {
    tx: Sender<String>,
    queued: Arc<AtomicUsize>,
}

impl Queue {
    /// The queue, and the writer's end of it.
    pub(super) fn new() -> (Queue, Receiver<String>, Arc<AtomicUsize>) {
        let (tx, rx) = mpsc::channel();
        let queued = Arc::new(AtomicUsize::new(0));
        (Queue { tx, queued: queued.clone() }, rx, queued)
    }

    /// Full once the backlog is past the limit: until then a peer takes the
    /// next line however big it is (a long agent message, a status), so
    /// one big line can't make a peer that keeps up look stopped. At most
    /// the limit plus a line is queued.
    fn push(&self, line: String) -> Queued {
        let len = line.len() + 1;
        if self.queued.load(Relaxed) > QUEUE_BYTES {
            return Queued::Full;
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

pub(super) fn serve(listener: UnixListener, tx: Sender<Ev>) {
    for conn in listener.incoming() {
        let Ok(conn) = conn else { continue };
        let tx = tx.clone();
        thread::spawn(move || connection(conn, tx));
    }
}

fn connection(conn: UnixStream, tx: Sender<Ev>) {
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
fn read_requests(input: impl Read, peer: u64, tx: &Sender<Ev>) {
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
        tx: &Sender<Ev>,
    ) -> Result<(), String> {
        let label = format!("{}#{n}", bridge.command[0]);
        // A bare name is looked for next to brnr first, as an agent's is: so
        // `brnr` is this brnr, whatever the editor's PATH.
        let program = paths::expand(&bridge.command[0]);
        let mut cmd = Command::new(spawn::bundled(program.as_os_str()).unwrap_or(program));
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
    /// transcript, where `brnr log` reads it back. Returns it as sent.
    pub(super) fn emit(&mut self, mut event: Value) -> Value {
        event["ts"] = json!(log::rfc3339(SystemTime::now()));
        event["host_id"] = json!(self.host_id);
        let name = event["event"].as_str().unwrap_or_default().to_owned();
        let line = event.to_string();
        if name != "acp" {
            self.sink.note(event["session"].as_str(), event.clone());
            if let Some(i) = event["session"].as_str().and_then(|s| self.find(s)) {
                self.sessions[i].last_active = SystemTime::now();
            }
            if self.show_events && !QUIET.contains(&name.as_str()) {
                if self.json_events {
                    println!("{}", render::clean(&line));
                } else if let Some(text) = render::event(&event, &render::Options::foreground()) {
                    println!("{text}");
                }
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
            Closer::Bridge(pid) if self.bridge_pids.contains(&pid) => unsafe {
                libc::kill(pid, libc::SIGTERM);
            },
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
            Some("send") => now(self.send(req)),
            Some("cancel") => now(self.cancel_turn(req)),
            Some("queue") => now(self.queue(req)),
            Some("subscribe") => now(self.subscribe(peer, req)),
            Some("pending") => {
                Ok(Some(json!({ "ok": true, "pending": self.pending_permissions() })))
            }
            Some("approve") => now(self.answer(peer, req, Choice::Allow)),
            Some("deny") => now(self.answer(peer, req, Choice::Deny)),
            Some("set_mode" | "set_config" | "set_model" | "fork" | "close") => {
                self.agent_op(peer, req).map(|()| None)
            }
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
        let outcome = self.resolve_permission(&handle, choice, req["option"].as_str(), &by)?;
        Ok(json!({ "ok": true, "session": session, "request": handle, "outcome": outcome }))
    }

    fn pending_permissions(&self) -> Vec<Value> {
        let owner = if self.editor_attached() { "editor" } else { "headless" };
        self.agent_requests
            .iter()
            .filter(|r| r.handle.is_some())
            .map(|r| {
                json!({
                    "request": r.handle,
                    "session": r.session,
                    "owner": owner,
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
            if self.capabilities()["steering"] != true {
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
                self.cancel(&session);
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
                    if self.capabilities()["image"] != true {
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
        let session = self.sessions[i].id.clone();
        let dropped: Vec<Value> = if req["keep_held"].as_bool() == Some(true) {
            Vec::new()
        } else {
            self.drop_held(i, "cancel")
        };
        let busy = !self.sessions[i].prompts.is_empty();
        if busy {
            self.cancel(&session);
        }
        let status = if busy { "cancelling" } else { "idle" };
        self.sink.note(Some(&session), json!({ "event": "cancel", "status": status }));
        Ok(json!({ "ok": true, "status": status, "session": session, "dropped": dropped }))
    }

    fn queue(&mut self, req: &Value) -> Result<Value, String> {
        let i = self.session_index(req)?;
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
        if req["clear_context"].as_bool() == Some(true) {
            s.context.clear();
        }
        let dropped = self.dropped(i, dropped, "queue");
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
        let caps = self.capabilities();
        let i = self.session_index(req)?;
        let session = self.sessions[i].id.clone();
        let text = |key: &str| req[key].as_str().map(str::to_owned).ok_or(format!("missing {key}"));
        match cmd {
            "set_mode" => {
                let mode = text("mode")?;
                let state = &self.sessions[i].state;
                let params = json!({ "sessionId": session, "modeId": mode });
                let option = state.option("mode").map(|o| o["id"].clone());
                if let Some(modes) = &state.modes {
                    let known = modes["availableModes"]
                        .as_array()
                        .is_none_or(|m| m.iter().any(|x| x["id"] == mode.as_str()));
                    if !known {
                        return Err(format!("no mode {mode} (see brnr mode)"));
                    }
                    self.peer_op(
                        peer,
                        req_id,
                        PeerOp::Mode { session, mode },
                        "session/set_mode",
                        params,
                    );
                } else if let Some(id) = option {
                    // An agent with modes only as a config option.
                    let params = json!({ "sessionId": session, "configId": id, "value": mode });
                    self.peer_op(
                        peer,
                        req_id,
                        PeerOp::Config { session },
                        "session/set_config_option",
                        params,
                    );
                } else {
                    return Err("the agent offers no modes".into());
                }
            }
            "set_config" => {
                let (option, value) = (text("option")?, text("value")?);
                let params = json!({ "sessionId": session, "configId": option, "value": value });
                self.peer_op(
                    peer,
                    req_id,
                    PeerOp::Config { session },
                    "session/set_config_option",
                    params,
                );
            }
            "set_model" => {
                // The config option of category `model`; never
                // `session/set_model` (ADR 28).
                let model = text("model")?;
                let option = self.sessions[i].state.option("model");
                let id = option.map(|o| o["id"].clone()).ok_or("the agent offers no model choice")?;
                let params = json!({ "sessionId": session, "configId": id, "value": model });
                self.peer_op(
                    peer,
                    req_id,
                    PeerOp::Config { session },
                    "session/set_config_option",
                    params,
                );
            }
            "fork" | "close" => {
                if self.editor_attached() {
                    return Err(format!("the editor owns this process; {cmd} sessions there"));
                }
                if cmd == "fork" {
                    self.check_strict(Beyond::Fork)?;
                }
                if caps[cmd] != true {
                    return Err(format!("the agent can't {cmd} sessions"));
                }
                // A second session could never close when idle, and the
                // process would never stop (ADR 12).
                if cmd == "fork" && self.stop_when_idle.is_some() && caps["close"] != true {
                    return Err("the agent can't close sessions: with stop_when_idle, a forked \
                                session would never close"
                        .into());
                }
                let cwd = self.sessions[i].cwd.clone();
                if cmd == "fork" {
                    let params = json!({ "sessionId": session, "cwd": cwd.to_string_lossy(), "mcpServers": self.mcp_servers });
                    let op = PeerOp::Fork { cwd };
                    self.peer_op(peer, req_id, op, "session/fork", params);
                } else {
                    self.close(i, peer, req_id, "close");
                }
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
