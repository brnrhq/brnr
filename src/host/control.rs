//! Bridges: everything that talks to the host in JSON lines instead of ACP.
//!
//! A bridge is a process the host starts from the profile's `bridges`
//! (requests on its stdout, responses and events on its stdin, and it
//! should exit when its stdin closes, which happens when the host dies), or
//! anything that connects to the control socket, such as brnr. Both
//! speak the same protocol, one JSON object per line.
//!
//! Requests: `{"cmd": …, "req_id"?: …}`; the response echoes `req_id`.
//! - `status`
//! - `send` `{text, mode?: now|after-turn|interrupt|context, replace?, session?}`
//! - `subscribe` `{events?: [...] | "all"}`: events follow on this connection
//!   (started bridges are subscribed from the start)
//! - `pending`: permission requests waiting for an answer
//! - `approve` / `deny` `{request?, option?, session?}`; only while no
//!   editor is attached
//! - `stop`: close the agent's stdin, then SIGTERM, then SIGKILL (the
//!   agent's process group)
//!
//! Events (`{"event": …, "ts", "host_id", …}`): see [`EVENTS`]. Subscribing
//! without a list gets every event except `acp`, which is busy (one per
//! streamed chunk) and must be asked for by name. While an editor is
//! attached, bridges observe; the editor answers the agent.
//!
//! Each peer has a queue of [`QUEUE`] lines. A peer that lets it fill up has
//! stopped reading and is dropped rather than buffered for without limit: a
//! connection is shut down, a started bridge gets SIGTERM.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::Shutdown;
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender, TrySendError};
use std::thread;
use std::time::SystemTime;

use libc::pid_t;
use serde_json::{Value, json};

use super::acp::Choice;
use super::{Ev, Host};
use crate::config::Bridge;
use crate::log::{self, Dir};
use crate::paths;

/// Every event name. `acp` (every ACP message the host passes on, with its
/// direction) is only sent to peers that ask for it by name.
pub(super) const EVENTS: &[&str] = &[
    "user_message",
    "agent_message",
    "permission_request",
    "permission_resolved",
    "turn_ended",
    "owner_changed",
    "exited",
    "acp",
];

/// Lines queued for one peer before it counts as having stopped reading.
const QUEUE: usize = 4096;

static NEXT_PEER: AtomicU64 = AtomicU64::new(1);

/// How to cut a peer off.
pub(super) enum Closer {
    Socket(UnixStream),
    Bridge(pid_t),
}

pub(super) struct Peer {
    tx: SyncSender<String>,
    label: String,
    closer: Closer,
    subscribed: bool,
    /// `None`: every event.
    events: Option<Vec<String>>,
}

impl Peer {
    pub(super) fn new(tx: SyncSender<String>, label: String, closer: Closer) -> Peer {
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
    let (out_tx, out_rx) = mpsc::sync_channel(QUEUE);
    thread::spawn(move || write_lines(writer, out_rx));
    let label = format!("socket#{peer}");
    let opened = Ev::PeerOpened { peer, tx: out_tx, label, closer: Closer::Socket(closer) };
    if tx.send(opened).is_err() {
        return;
    }
    read_requests(conn, peer, &tx);
    let _ = tx.send(Ev::PeerClosed { peer });
}

fn write_lines(mut out: impl Write, lines: Receiver<String>) {
    for line in lines {
        if writeln!(out, "{line}").and_then(|()| out.flush()).is_err() {
            return;
        }
    }
}

fn read_requests(input: impl Read, peer: u64, tx: &Sender<Ev>) {
    let mut reader = BufReader::new(input);
    let mut line = String::new();
    while matches!(reader.read_line(&mut line), Ok(n) if n > 0) {
        if !line.trim().is_empty() {
            let req = serde_json::from_str::<Value>(&line)
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
        let mut cmd = Command::new(paths::expand(&bridge.command[0]));
        cmd.args(&bridge.command[1..])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("BRNR_HOST", self.info["id"].as_str().unwrap_or_default())
            .env("BRNR_HOST_ID", &self.host_id)
            .env("BRNR_SOCKET", &self.sock_path)
            // Out of a terminal's reach when the host is run by hand; it
            // exits when the host closes its stdin.
            .process_group(0);
        let mut child = cmd.spawn().map_err(|e| format!("bridge {}: {e}", bridge.command[0]))?;
        let pid = child.id() as pid_t;
        let peer = NEXT_PEER.fetch_add(1, Relaxed);
        let (out_tx, out_rx) = mpsc::sync_channel(QUEUE);
        let stdin = child.stdin.take().unwrap();
        thread::spawn(move || write_lines(stdin, out_rx));
        let stdout = child.stdout.take().unwrap();
        let t = tx.clone();
        thread::spawn(move || {
            read_requests(stdout, peer, &t);
            let _ = t.send(Ev::PeerClosed { peer });
        });
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
        thread::spawn(move || {
            let status = child.wait().ok().and_then(|s| s.code());
            let _ = t.send(Ev::BridgeExited { label: l, status, pid });
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
            let msg = serde_json::from_slice::<Value>(body)
                .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(body).into_owned()));
            self.emit(json!({ "event": "acp", "dir": dir.name(), "session": session, "msg": msg }));
        }
    }

    /// Sends `event` to every subscribed peer that wants it.
    pub(super) fn emit(&mut self, mut event: Value) {
        event["ts"] = json!(log::rfc3339(SystemTime::now()));
        event["host_id"] = json!(self.host_id);
        let name = event["event"].as_str().unwrap_or_default().to_owned();
        let line = event.to_string();
        let peers: Vec<u64> =
            self.peers.iter().filter(|(_, p)| p.wants(&name)).map(|(&id, _)| id).collect();
        for peer in peers {
            self.send_to(peer, line.clone());
        }
    }

    /// Queues `line` for `peer`, dropping a peer that has gone or fallen
    /// behind.
    fn send_to(&mut self, peer: u64, line: String) {
        let Some(p) = self.peers.get(&peer) else { return };
        match p.tx.try_send(line) {
            Ok(()) => {}
            Err(TrySendError::Disconnected(_)) => {
                self.peers.remove(&peer);
            }
            Err(TrySendError::Full(_)) => self.drop_peer(peer),
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

    pub(super) fn peer_request(&mut self, peer: u64, req: Value) {
        let mut response =
            self.command(peer, &req).unwrap_or_else(|e| json!({ "ok": false, "error": e }));
        if let Some(req_id) = req.get("req_id") {
            response["req_id"] = req_id.clone();
        }
        self.send_to(peer, response.to_string());
    }

    fn command(&mut self, peer: u64, req: &Value) -> Result<Value, String> {
        if let Some(err) = req.get("parse_error") {
            return Err(format!("bad request: {}", err.as_str().unwrap_or_default()));
        }
        match req["cmd"].as_str() {
            Some("status") => Ok(self.status_report()),
            Some("send") => self.send(req),
            Some("subscribe") => self.subscribe(peer, req),
            Some("pending") => Ok(json!({ "ok": true, "pending": self.pending_permissions() })),
            Some("approve") => self.answer(peer, req, Choice::Allow),
            Some("deny") => self.answer(peer, req, Choice::Deny),
            Some("stop") => {
                if self.status.is_some() {
                    return Err("the agent has already exited".into());
                }
                self.begin_stop();
                Ok(json!({ "ok": true, "status": "stopping" }))
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
        Ok(
            json!({ "ok": true, "events": events.unwrap_or_else(|| EVENTS.iter().filter(|e| **e != "acp").map(|e| e.to_string()).collect()) }),
        )
    }

    fn answer(&mut self, peer: u64, req: &Value, choice: Choice) -> Result<Value, String> {
        let handle = match req["request"].as_str() {
            Some(handle) => handle.to_owned(),
            None => {
                let session = req["session"].as_str();
                let waiting: Vec<String> = self
                    .agent_requests
                    .iter()
                    .filter(|r| session.is_none_or(|s| r.session.as_deref() == Some(s)))
                    .filter_map(|r| r.handle.clone())
                    .collect();
                match &waiting[..] {
                    [one] => one.clone(),
                    [] => return Err("no permission request is waiting".into()),
                    _ => {
                        return Err(format!(
                            "several requests are waiting, pick one: {}",
                            waiting.join(", ")
                        ));
                    }
                }
            }
        };
        let by = self.peers.get(&peer).map_or("control".to_owned(), |p| p.label.clone());
        let outcome = self.resolve_permission(&handle, choice, req["option"].as_str(), &by)?;
        Ok(json!({ "ok": true, "request": handle, "outcome": outcome }))
    }

    fn pending_permissions(&self) -> Vec<Value> {
        let owner = if self.editor_attached() { "editor" } else { "host" };
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
                    "options": r.params["options"],
                })
            })
            .collect()
    }

    fn status_report(&self) -> Value {
        let mut report = self.info.clone();
        report["ok"] = json!(true);
        report["owner"] = json!(if self.editor_attached() { "editor" } else { "host" });
        report["pending"] = json!(self.pending_permissions().len());
        report["bridges"] = json!(
            self.peers
                .values()
                .filter(|p| !p.label.starts_with("socket#"))
                .map(|p| &p.label)
                .collect::<Vec<_>>()
        );
        report["sessions"] = self
            .sessions
            .iter()
            .map(|s| {
                json!({
                    "session_id": s.id,
                    "cwd": s.cwd.to_string_lossy(),
                    "busy": !s.prompts.is_empty(),
                    "prompts": s.prompts.len(),
                    "held": s.held.len(),
                    "context": s.context.len(),
                    "log": self.log.host_log().map(|_| paths::session_log(&s.cwd, &s.id).to_string_lossy().into_owned()),
                })
            })
            .collect();
        report
    }

    fn send(&mut self, req: &Value) -> Result<Value, String> {
        let text = req["text"].as_str().ok_or("missing text")?.to_owned();
        let mode = req["mode"].as_str().unwrap_or("now");
        let replace = req["replace"].as_bool().unwrap_or(false);
        if self.status.is_some() || self.agent_in.is_none() {
            return Err("the agent is no longer accepting input".into());
        }
        let i = self.pick_session(req["session"].as_str())?;
        let session = self.sessions[i].id.clone();
        let busy = !self.sessions[i].prompts.is_empty();
        let status = match mode {
            "now" => {
                self.send_prompt(i, text.clone());
                if busy { "queued" } else { "delivered" }
            }
            "after-turn" => {
                if busy || !self.sessions[i].held.is_empty() {
                    self.sessions[i].held.push_back(text.clone());
                    "held"
                } else {
                    self.send_prompt(i, text.clone());
                    "delivered"
                }
            }
            "interrupt" => {
                if busy {
                    let s = &mut self.sessions[i];
                    s.held.insert(s.interrupts, text.clone());
                    s.interrupts += 1;
                    self.cancel(&session);
                    "interrupting"
                } else {
                    self.send_prompt(i, text.clone());
                    "delivered"
                }
            }
            "context" => {
                let context = &mut self.sessions[i].context;
                match context.last_mut() {
                    Some(last) if replace => *last = text.clone(),
                    _ => context.push(text.clone()),
                }
                "held"
            }
            other => return Err(format!("unknown mode: {other}")),
        };
        self.sink.note(
            Some(&session),
            json!({ "event": "send", "mode": mode, "replace": replace, "status": status, "text": text }),
        );
        Ok(json!({ "ok": true, "status": status, "session": session }))
    }

    /// The session a request means: the one named (exactly or by unique
    /// prefix), or the only one there is.
    fn pick_session(&self, wanted: Option<&str>) -> Result<usize, String> {
        let ids = || self.sessions.iter().map(|s| s.id.as_str()).collect::<Vec<_>>().join(", ");
        let Some(wanted) = wanted else {
            return match self.sessions.len() {
                0 => Err("no ACP session yet".into()),
                1 => Ok(0),
                _ => Err(format!("several sessions, pick one: {}", ids())),
            };
        };
        if let Some(i) = self.find(wanted) {
            return Ok(i);
        }
        let matches: Vec<usize> =
            (0..self.sessions.len()).filter(|&i| self.sessions[i].id.starts_with(wanted)).collect();
        match matches[..] {
            [i] => Ok(i),
            [] => Err(format!("no session {wanted}")),
            _ => Err(format!("{wanted} matches several sessions: {}", ids())),
        }
    }
}
