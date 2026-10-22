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
//!   ADR 5.5); `lost` if a file has lost records it hasn't noted yet (ADR 10.2)
//! - `send` `{session, text?, blocks?, mode?: prompt|steer|interrupt|context,
//!   replace?}` (ADR 4.2 in docs/adr): the response's `status` is
//!   `delivered`, `held` (until the running turn ends), `steered` (into it)
//!   or `interrupting`, and it has the message's id (`m<n>`), which the
//!   `user_message` of its turn carries, and the `turn_ended` in `messages`
//! - `cancel` `{session, keep_held?}`: cancel the running turn; held
//!   messages are dropped (and listed) unless `keep_held`
//! - `queue` `{session, drop?, clear?, clear_context?}`: the held messages
//!   and context, after removing what was asked; `{session, show}`: the held
//!   message `show` (an id; anything else is refused) in full, its content
//!   `blocks` included
//! - `subscribe` `{events?: [...] | "all"}`: events follow on this connection
//!   (started bridges are subscribed from the start). The answer names the
//!   connection (`peer`), the process (`host_id`) and the last event before
//!   it (`seq`): every event has a position, its `seq`, one more than the
//!   last event's or ACP message's of the process (ADR 10.2)
//! - `was_cut_off` `{peer}`: whether the process cut that connection off,
//!   and the last event it queued for it (`last_queued_seq`), so that a peer
//!   that has reconnected can catch up from the transcript (ADR 10.2). It
//!   says only where; the events are in the transcript
//! - `pending`: permission requests waiting for an answer, in full, and
//!   beside them `elicitations`, the agent's requests for input (ADR 4.5)
//! - `allow` / `reject` `{session, request, always?, option_id?}`: with the
//!   option of kind `allow_once` (`allow_always` with `always`), or
//!   `reject_once` (`reject_always`), or the option named (ADR 8.1)
//! - `elicitation` `{session, request, action: accept|decline|cancel,
//!   content?}` (ADR 4.5): answers an elicitation with ACP's action, an
//!   accept's `content` (an object of the form's fields) checked against its
//!   schema first; `session` is null for one scoped to a request outside any
//!   session
//! - `set_config` `{session, mode?, model?, thought_level?, options?: {id:
//!   value}}` (`config set`, ADR 8.1): resolved as a start's settings are
//!   (ADR 6.1), sent one at a time, and answered once the agent has set them
//!   all (`set`, what was sent) or refused one
//! - `fork` `{session}`: never in an editor's process
//! - `new` `{cwd?, mode?, model?, thought_level?, options?, text?, blocks?,
//!   timeout?}` and `resume` `{session, …}` (`session new --pid`, `session
//!   resume --pid`, ADR 8.1): a session opened in this process
//!   (`session/new`, or `session/resume` or `session/load`), in `cwd` (the
//!   process's by default), with the process's MCP servers, then its
//!   settings, over the profile's as a start's are, then the prompt `text`
//!   and `blocks` if any. Answered `{session, pid, message}` once it is set
//!   up, before the prompt goes: the commit, as a start's ready report is
//!   (ADR 3.4). One that fails after the agent opened it is closed again, and
//!   the answer says so: a setting, `timeout` seconds passing, the process
//!   stopping. Never in an editor's process, nor with `stop_when_idle` when
//!   the agent can't close sessions, as `fork`
//! - `close` `{session, take_over?}`: cancels a running turn first, and is
//!   answered once the agent has closed the session; a headless process
//!   whose last session closes stops. `take_over` is the pid of the process
//!   `session resume --take-over` resumes it in
//! - `stop`: close the agent's stdin, then SIGTERM, then SIGKILL (the
//!   agent's process group)
//!
//! On an editor's session every command that acts on it (`send`, `cancel`,
//! `queue clear --context`, `allow`, `reject`, `elicitation`, `set_config`
//! and `close`) is experimental: refused unless the editor's profile enables
//! it (ADR 7.1, see experimental.rs). Bridges observe it freely.
//!
//! Events (`{"event": …, "ts", "host_id", …}`): see [`EVENTS`]. Subscribing
//! without a list gets every event except `acp`, which is busy (one per
//! streamed chunk) and must be asked for by name. A held message that goes
//! unsent (`cancel`, `queue`, its session closing, the agent exiting) is a
//! `message_dropped`; a session that closes, `session_closed`, and one an
//! editor's load opens again after that, `session_reopened`; one the agent
//! deleted, `session_deleted`.
//!
//! Each peer's queue holds up to [`QUEUE_BYTES`]. A peer that lets it fill
//! up has stopped reading and is dropped rather than buffered for without
//! limit: a connection is shut down, a started bridge gets SIGTERM. The limit
//! is in bytes, not lines, so a burst of small events (an agent streaming
//! fast) doesn't look like a peer that stopped reading, and the line that
//! takes a peer past it doesn't count, nor (up to [`ASIDE_BYTES`] of them)
//! lines longer than the whole queue, so one long message doesn't either,
//! even when it comes twice, as its ACP message and its event (ADR 10.1).
//! A peer cut off is noted in the host log (`peer_dropped`), kept among the
//! last [`CUT_OFFS`] for `was_cut_off`, and told to the other peers, as a
//! `peer_cut_off` event (ADR 10.2).
//!
//! A request is a line of up to [`LINE_BYTES`], its newline included: a
//! peer that sends a longer one is answered with an error, noted in the
//! host log (`peer_dropped`, `line_too_long`) and cut off (ADR 10.3). At most
//! [`MAX_CONNECTIONS`] connections are open at once; one more is told so and
//! closed (ADR 10.4).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::Shutdown;
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use libc::pid_t;
use serde_json::{Value, json};

use super::approvals::{self, AgentRequest, Answer, Choice, Kind, Who};
use super::elicitation;
use super::flow::LINE_BYTES;
use super::requests::{HostRequest, Open, Opening, PeerOp, Setup, SetupOf, resolve_settings};
use super::strict::Beyond;
use super::turns::{Activity, Held, Mode, Status};
use super::{Ev, Host};
use crate::config::{Bridge, Experimental, Log};
use crate::log::{self, Dir};
use crate::request::{Prompt, Settings};
use crate::{json, paths, render, sys};

/// Every event name. `acp` (every ACP message the host passes on, with its
/// direction) is only sent to peers that ask for it by name.
pub(crate) const EVENTS: &[&str] = &[
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
    "elicitation_request",
    "elicitation_resolved",
    "elicitation_completed",
    "turn_ended",
    "message_dropped",
    "context_dropped",
    "session_closed",
    "session_reopened",
    "session_deleted",
    "history",
    "line_too_long",
    "line_too_deep",
    "peer_cut_off",
    "process_exited",
    "acp",
];

/// Notes: what brnr adds to a stream of events, and the host never emits
/// (ADR 5.9): the logger's gap in a transcript (ADR 10.1), and what a command
/// cut off couldn't catch up on (ADR 10.2). `--events` takes them;
/// `subscribe` and a bridge's `events` refuse them.
pub(crate) const NOTES: &[&str] = &["records_skipped", "events_missed"];

/// Events brnr leaves out of what it shows unless asked for by name: the
/// ACP messages, the agent's thoughts, usage and tool calls' progress.
/// `event watch` and `event log` without `--events`, and a session in the
/// foreground, in text and JSON alike.
pub(crate) const QUIET: &[&str] = &["acp", "agent_thought", "usage", "tool_progress"];

/// Bytes queued for one peer before it counts as having stopped reading.
const QUEUE_BYTES: usize = 16 << 20;

/// Bytes of lines a peer past [`QUEUE_BYTES`] may have queued that don't
/// count toward it (ADR 10.1): two of the longest lines the host reads
/// (ADR 10.3), a message's ACP line and its event.
const ASIDE_BYTES: usize = 64 << 20;

/// How many cut-offs the process keeps for `was_cut_off` (ADR 10.2): a peer
/// reconnecting after this many more were cut off is told it wasn't.
const CUT_OFFS: usize = 64;

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
    /// `line`, from a thread other than the event loop's: a peer that is
    /// full is cut off by the event loop's next line to it.
    pub(super) fn post(&self, line: String) {
        let _ = self.push(line);
    }

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
    /// a peer that keeps up behind (ADR 10.1). Nor does a line longer than the
    /// limit that comes while it is past it (the `agent_message` right after
    /// its ACP message, to a peer that asked for `acp`), up to
    /// [`ASIDE_BYTES`] set aside (ADR 10.1). At most the limit, the lines set
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

pub(crate) fn check_bridge(bridge: &Bridge) -> Result<(), String> {
    if bridge.command.is_empty() {
        return Err("a bridge needs a command".into());
    }
    check_events(bridge.events.as_deref().unwrap_or_default())
}

fn check_events(events: &[String]) -> Result<(), String> {
    match events.iter().find(|e| !EVENTS.contains(&e.as_str())) {
        // The host never sends one (ADR 5.9).
        Some(note) if NOTES.contains(&note.as_str()) => {
            Err(format!("{note:?} is a note, which the host never sends: only events are"))
        }
        Some(bad) => Err(format!("unknown event {bad:?} (events: {})", EVENTS.join(", "))),
        None => Ok(()),
    }
}

// ---- connections ---------------------------------------------------------

/// Connections open at once (ADR 10.4): each costs two threads and a queue
/// of up to [`QUEUE_BYTES`], so a runaway client of the user's can't take
/// the process's threads, file descriptors or memory.
const MAX_CONNECTIONS: usize = 128;

/// The wait after an accept fails, doubling while it keeps failing, up to
/// [`ACCEPT_WAIT_MAX`] (ADR 10.4).
const ACCEPT_WAIT: Duration = Duration::from_millis(10);
const ACCEPT_WAIT_MAX: Duration = Duration::from_secs(1);

pub(super) fn serve(listener: UnixListener, tx: SyncSender<Ev>) {
    serve_up_to(listener, tx, MAX_CONNECTIONS)
}

/// Serves up to `max` connections at once; one more is told so and closed.
/// An accept that fails (out of file descriptors, say) is tried again after
/// a wait that doubles while it keeps failing, not at once and for ever.
fn serve_up_to(listener: UnixListener, tx: SyncSender<Ev>, max: usize) {
    let open = Arc::new(AtomicUsize::new(0));
    let mut wait = None;
    for conn in listener.incoming() {
        let mut conn = match conn {
            Ok(conn) => conn,
            Err(_) => {
                let next = backoff(wait);
                wait = Some(next);
                thread::sleep(next);
                continue;
            }
        };
        wait = None;
        if open.fetch_add(1, Relaxed) >= max {
            open.fetch_sub(1, Relaxed);
            // A short line into an empty socket buffer, which never waits; it
            // is made sure of anyway, so a client can't hold up the others.
            let error = format!("too many connections: {max} open");
            let _ = conn.set_nonblocking(true);
            let _ = writeln!(conn, "{}", json!({ "ok": false, "error": error }));
            continue;
        }
        let (tx, slot) = (tx.clone(), Slot(open.clone()));
        thread::spawn(move || connection(conn, tx, slot));
    }
}

/// The wait before accepting again after the wait `last` (none after a
/// success).
fn backoff(last: Option<Duration>) -> Duration {
    last.map_or(ACCEPT_WAIT, |last| (last * 2).min(ACCEPT_WAIT_MAX))
}

/// A connection counted as open until it is dropped.
struct Slot(Arc<AtomicUsize>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Relaxed);
    }
}

fn connection(conn: UnixStream, tx: SyncSender<Ev>, slot: Slot) {
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
    // Free before the event loop hears, so a client that has seen one
    // connection end can open another.
    drop(slot);
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
/// UTF-8) is answered with an error; the peer stays. One longer than
/// [`LINE_BYTES`] isn't read past that: the peer is cut off (ADR 10.3).
fn read_requests(input: impl Read, peer: u64, tx: &SyncSender<Ev>) {
    let mut reader = BufReader::new(input);
    let mut line = Vec::new();
    // Never more of a line than the limit in memory.
    let mut next =
        |line: &mut Vec<u8>| (&mut reader).take(LINE_BYTES as u64).read_until(b'\n', line);
    while matches!(next(&mut line), Ok(n) if n > 0) {
        if line.len() == LINE_BYTES && line.last() != Some(&b'\n') {
            let _ = tx.send(Ev::PeerTooLong { peer });
            return;
        }
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
        let program = Path::new(&bridge.command);
        let name = program.file_name().unwrap_or(program.as_os_str()).to_string_lossy();
        let label = format!("{name}#{n}");
        let mut cmd = Command::new(program);
        cmd.args(&bridge.args)
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
        let mut child = cmd.spawn().map_err(|e| format!("bridge {}: {e}", bridge.command))?;
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
        // Into the host log as lines, a piece of 64 KiB at most (ADR 10.3).
        thread::spawn(move || {
            super::read_stderr(stderr, |line| {
                t.send(Ev::BridgeStderr { label: l.clone(), line }).is_ok()
            })
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
        self.sink
            .note(None, json!({ "event": "bridge_started", "peer": label, "bridge_pid": pid }));
        Ok(())
    }

    /// Logs an ACP message and shows it to peers subscribed to `acp`, at
    /// the next position, whether a peer wants it or not: one that is cut
    /// off finds it in the transcript there (ADR 10.2).
    pub(super) fn record(&mut self, session: Option<&str>, dir: Dir, bytes: &[u8]) {
        self.seq += 1;
        let seq = self.seq;
        self.sink.msg(session, dir, bytes, Some(seq));
        if self.peers.values().any(|p| p.wants("acp")) {
            let body = bytes.strip_suffix(b"\n").unwrap_or(bytes);
            // As the host reads it (see json.rs).
            let msg = json::parse::<Value>(body, json::DEPTH)
                .unwrap_or_else(|| Value::String(String::from_utf8_lossy(body).into_owned()));
            let event =
                json!({ "event": "acp", "dir": dir.name(), "session": session, "msg": msg });
            self.emit_at(seq, event);
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
    /// transcript, where `brnr event log` reads it back, at the next
    /// position (ADR 10.2). Returns it as sent.
    pub(super) fn emit(&mut self, event: Value) -> Value {
        self.seq += 1;
        self.emit_at(self.seq, event)
    }

    /// [`emit`](Self::emit) at position `seq`, an ACP message's.
    fn emit_at(&mut self, seq: u64, mut event: Value) -> Value {
        event["ts"] = json!(log::rfc3339(SystemTime::now()));
        event["host_id"] = json!(self.host_id);
        event["pid"] = json!(std::process::id());
        let name = event["event"].as_str().unwrap_or_default().to_owned();
        // History a load replays, which brnr had no transcript of (ADR 5.6).
        if let Some(i) = event["session"].as_str().and_then(|s| self.find(s))
            && self.sessions[i].replay.as_ref().is_some_and(|r| r.record)
        {
            event["replayed"] = json!(true);
        }
        event["seq"] = json!(seq);
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
                // (ADR 3.5).
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
        self.tell_cut_offs();
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

    /// Cuts off a peer that stopped reading. It was sent nothing after the
    /// last event so far, which is kept among the last cut-offs and told to
    /// the other peers once the line that cut it off has reached them, so
    /// that each sees events in order (ADR 10.2).
    fn drop_peer(&mut self, peer: u64) {
        let Some(p) = self.peers.remove(&peer) else { return };
        let last = self.seq;
        let reason = "fell_behind";
        let event = json!({ "event": "peer_dropped", "peer": p.label, "reason": reason, "last_queued_seq": last });
        self.sink.note(None, event);
        if self.cut_offs.len() == CUT_OFFS {
            self.cut_offs.pop_front();
        }
        self.cut_offs.push_back((p.label.clone(), last));
        let told = json!({ "event": "peer_cut_off", "peer": p.label, "last_queued_seq": last });
        self.untold.push_back(told);
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

    /// Cuts off a peer whose request ran past [`LINE_BYTES`], unread, once
    /// it is told why (ADR 10.3): where its next request would start can't
    /// be known. Not one that fell behind: there is nothing to catch up on.
    /// A connection closes once the answer is written; a started bridge gets
    /// SIGTERM, as one cut off does.
    pub(super) fn request_too_long(&mut self, peer: u64) {
        let Some(p) = self.peers.remove(&peer) else { return };
        let error = format!("a request longer than {LINE_BYTES} bytes: cut off");
        let _ = p.tx.push(json!({ "ok": false, "error": error }).to_string());
        let reason = "line_too_long";
        let event = json!({ "event": "peer_dropped", "peer": p.label, "reason": reason, "limit": LINE_BYTES });
        self.sink.note(None, event);
        if let Closer::Bridge(pid) = p.closer
            && self.bridge_pids.contains(&pid)
        {
            sys::kill(pid, libc::SIGTERM);
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
        self.tell_cut_offs();
    }

    /// The `peer_cut_off` events of the peers cut off since, each after the
    /// line that cut it off (ADR 10.2).
    fn tell_cut_offs(&mut self) {
        while let Some(event) = self.untold.pop_front() {
            self.emit(event);
        }
    }

    /// `was_cut_off`: whether the peer named `peer` was cut off, and the last
    /// event queued for it then (ADR 10.2). A position, never an event: what
    /// it missed is in the transcript, which the asker reads as itself.
    fn was_cut_off(&self, req: &Value) -> Result<Value, String> {
        let peer = req["peer"].as_str().ok_or("missing peer")?;
        let cut = self.cut_offs.iter().rev().find(|(label, _)| label == peer);
        Ok(json!({
            "ok": true,
            "peer": peer,
            "cut_off": cut.is_some(),
            "last_queued_seq": cut.map(|(_, last)| *last),
        }))
    }

    /// `Ok(None)`: the answer comes from the agent, later.
    pub(super) fn command(&mut self, peer: u64, req: &Value) -> Result<Option<Value>, String> {
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
            Some("was_cut_off") => now(self.was_cut_off(req)),
            Some("pending") => Ok(Some(json!({
                "ok": true,
                "pending": self.pending_permissions(),
                "elicitations": self.pending_elicitations(),
            }))),
            Some("allow") => now(self.answer(peer, req, Choice::Allow)),
            Some("reject") => now(self.answer(peer, req, Choice::Reject)),
            Some("elicitation") => now(self.elicitation(peer, req)),
            Some("set_config" | "fork" | "close") => self.agent_op(peer, req).map(|()| None),
            Some("new" | "resume") => self.open(peer, req).map(|()| None),
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

    /// `peer`'s lines, to answer it from another thread.
    pub(super) fn peer_queue(&self, peer: u64) -> Option<Queue> {
        Some(self.peers.get(&peer)?.tx.clone())
    }

    /// `logged`: answered from the logger's thread once it has written what
    /// the process recorded before it was asked, so that what a peer has
    /// seen happen is in the transcript (ADR 5.5), with `lost` if a file has
    /// lost records it hasn't noted yet (ADR 10.2). The event loop doesn't
    /// wait for it (P1); a peer gone meanwhile isn't answered.
    fn when_logged(&mut self, peer: u64, req_id: Option<Value>) {
        let Some(queue) = self.peer_queue(peer) else { return };
        let mut response = json!({ "ok": true });
        if let Some(req_id) = req_id {
            response["req_id"] = req_id;
        }
        self.sink.when_written(move |lost| {
            response["lost"] = json!(lost);
            queue.post(response.to_string());
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
        // Where it starts, to catch up from if it is cut off (ADR 10.2).
        let (label, host_id, seq) = (p.label.clone(), &self.host_id, self.seq);
        let pid = std::process::id();
        Ok(
            json!({ "ok": true, "events": names, "peer": label, "host_id": host_id, "pid": pid, "seq": seq }),
        )
    }

    fn answer(&mut self, peer: u64, req: &Value, choice: Choice) -> Result<Value, String> {
        let i = self.session_index(req)?;
        self.check_experimental(Experimental::Permission)?;
        let session = self.sessions[i].id.clone();
        let handle = req["request"].as_str().ok_or("missing request")?.to_owned();
        let by = Who::peer(self.peers.get(&peer).map(|p| p.label.clone()));
        let always = req["always"].as_bool() == Some(true);
        // Its old name, refused rather than ignored: the option it named
        // would not be the one answered (ADR 8.1, P3).
        if req.get("option").is_some() {
            let cmd = req["cmd"].as_str().unwrap_or("allow");
            return Err(format!("{cmd} takes option_id, not option"));
        }
        // Absent, it picks by kind; anything but a string, null too, is
        // refused: read as absent, the kind's option would be answered in
        // place of the one named (ADR 8.1, P3).
        let option_id = match req.get("option_id") {
            None => None,
            Some(Value::String(id)) => Some(id.as_str()),
            Some(_) => return Err("option_id isn't a string".into()),
        };
        let resolved = self.approvals.answer(
            Kind::Permission,
            &handle,
            Some(&session),
            &by,
            self.client(),
            |r| approvals::choose(r, choice, always, option_id),
        )?;
        let outcome = resolved.answer.clone();
        self.resolved(resolved);
        Ok(json!({ "ok": true, "session": session, "request": handle, "outcome": outcome }))
    }

    fn pending_permissions(&self) -> Vec<Value> {
        let owner = if self.editor_attached() { "editor" } else { "headless" };
        // Whether `brnr permission allow` would be taken, and if not, why
        // (ADR 7.1).
        let why_not = self.check_experimental(Experimental::Permission).err();
        self.approvals
            .held()
            .filter(|r| r.is_permission())
            .map(|r| {
                json!({
                    "request": r.held(),
                    "session": r.session,
                    "owner": owner,
                    "answerable": why_not.is_none(),
                    "why_not": why_not,
                    "title": r.params["toolCall"]["title"],
                    "kind": r.params["toolCall"]["kind"],
                    "tool_call": r.params["toolCall"],
                    "options": r.params["options"],
                    "timeout_left": r.deadline().map(|d| d.saturating_duration_since(Instant::now()).as_secs()),
                })
            })
            .collect()
    }

    /// `elicitation` (ADR 4.5): answers elicitation `request` of `session`
    /// (null for one scoped to a request outside any) with `action`, and an
    /// accept's `content`, checked against the form's schema before anything
    /// is sent.
    fn elicitation(&mut self, peer: u64, req: &Value) -> Result<Value, String> {
        let session = match &req["session"] {
            Value::Null => None,
            _ => Some(self.sessions[self.session_index(req)?].id.clone()),
        };
        self.check_experimental(Experimental::Elicitation)?;
        let handle = req["request"].as_str().ok_or("missing request")?.to_owned();
        let action = req["action"].as_str().ok_or("missing action: accept, decline or cancel")?;
        let by = Who::peer(self.peers.get(&peer).map(|p| p.label.clone()));
        let resolved = self.approvals.answer(
            Kind::Elicitation,
            &handle,
            session.as_deref(),
            &by,
            self.client(),
            |r| elicitation::response(r, action, &req["content"]).map(Answer::Result),
        )?;
        let result = resolved.answer.clone();
        self.resolved(resolved);
        Ok(json!({ "ok": true, "session": session, "request": handle, "result": result }))
    }

    /// The elicitations waiting, each in full (see elicitation.rs), with
    /// whether `brnr elicitation accept` would be taken (ADR 7.1) and when the
    /// timeout cancels it.
    fn pending_elicitations(&self) -> Vec<Value> {
        let owner = if self.editor_attached() { "editor" } else { "headless" };
        let why_not = self.check_experimental(Experimental::Elicitation).err();
        self.approvals
            .held()
            .filter(|r| r.is_elicitation())
            .map(|r| {
                let mut e = elicitation::describe(r);
                e["request"] = json!(r.held());
                e["session"] = json!(r.session);
                e["owner"] = json!(owner);
                e["answerable"] = json!(why_not.is_none());
                e["why_not"] = json!(why_not);
                e["timeout_left"] = json!(
                    r.deadline().map(|d| d.saturating_duration_since(Instant::now()).as_secs())
                );
                e
            })
            .collect()
    }

    fn status_report(&self) -> Value {
        let mut report = self.info.clone();
        report["ok"] = json!(true);
        report["owner"] = json!(if self.editor_attached() { "editor" } else { "headless" });
        // Counted: `pending` describes them.
        report["pending"] = json!(self.approvals.held().filter(|r| r.is_permission()).count());
        report["elicitations"] =
            json!(self.approvals.held().filter(|r| r.is_elicitation()).count());
        report["uptime"] = json!(self.started.elapsed().as_secs());
        report["stop_when_idle"] = json!(self.stop_when_idle.map(|d| d.as_secs()));
        report["starting"] = json!(!self.start_done);
        report["stopping"] = json!(self.stop_requested);
        report["closing"] = json!(self.sessions.closing());
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
                let held = |r: &&AgentRequest| r.session.as_ref() == Some(&s.id);
                let requests = self.approvals.held().filter(held);
                let (asking, pending): (Vec<_>, Vec<_>) =
                    requests.partition(|r| r.is_elicitation());
                let (pending, elicitations) = (pending.len(), asking.len());
                // Waiting for the user's answer, not for a turn.
                let busy = s.turns.activity() == Activity::Busy;
                let state = match (pending + elicitations, busy) {
                    (n, _) if n > 0 => "waiting",
                    (_, true) => "busy",
                    _ => "idle",
                };
                // Its transcript: events, and raw ACP (ADR 5.1).
                let events = paths::session_log(&s.cwd, &s.id);
                let acp = paths::acp_log(&events);
                let fields = json!({
                    "session": s.id,
                    "state": state,
                    "last_active": log::rfc3339(s.last_active),
                    "cwd": s.cwd.to_string_lossy(),
                    "forked_from": s.forked_from,
                    "busy": busy,
                    "turn_elapsed": s.turns.running_for().map(|d| d.as_secs()),
                    "prompts": s.turns.prompts(),
                    "held": s.turns.held(),
                    "context": s.turns.context().len(),
                    "pending": pending,
                    "elicitations": elicitations,
                    "shared": s.shared(),
                    "lock_error": s.lock_error(),
                    "last_turn": s.turns.last_turn(),
                    "events_log": (self.logging != Log::Off).then(|| events.to_string_lossy()),
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

    /// `send` (ADR 4.2): a prompt of its own, held while a turn runs; steered
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
            self.sessions[i].turns.add_context(text.clone(), replace);
            self.sink.note(
                Some(&session),
                json!({ "event": "context_accepted", "status": "held", "text": text }),
            );
            return Ok(json!({ "ok": true, "status": "held", "session": session }));
        }
        if !self.start_done {
            // The start commits before the agent gets any work (ADR 3.4).
            return Err("the session is still starting".into());
        }
        let how = match mode {
            "steer" => Mode::Steer,
            "interrupt" => Mode::Interrupt,
            _ => Mode::Prompt,
        };
        if self.sessions[i].turns.status(how) == Status::Steered {
            self.check_strict(Beyond::Steering)?;
            if !self.caps.steering {
                let why = "it doesn't advertise _session/steering";
                return Err(format!("the agent can't steer a running turn: {why}"));
            }
        }
        let held = Held { id: self.message_id(), text: text.clone(), blocks };
        let message = held.id.clone();
        let (status, todo) = self.sessions[i].turns.send(held, how);
        self.act(i, todo);
        let status = status.name();
        self.sink.note(
            Some(&session),
            json!({ "event": "message_accepted", "mode": mode, "status": status, "message": message, "text": text }),
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
        let keep_held = req["keep_held"].as_bool() == Some(true);
        let (status, dropped, todo) = self.sessions[i].turns.cancel(keep_held);
        self.act(i, todo);
        self.sink.note(Some(&session), json!({ "event": "cancel_requested", "status": status }));
        Ok(json!({ "ok": true, "status": status, "session": session, "dropped": dropped }))
    }

    /// `queue`: one held message in full (`show`), or the held messages and
    /// context after dropping one (`drop`) or clearing them (`clear`,
    /// `clear_context`).
    fn queue(&mut self, req: &Value) -> Result<Value, String> {
        let i = self.session_index(req)?;
        // A show that isn't an id is refused, not taken as absent: read as
        // absent, `{show: 2, clear: true}` would clear everything (P3).
        if let Some(show) = req.get("show") {
            let id = show.as_str().ok_or("show must be a message id")?;
            if ["drop", "clear", "clear_context"].iter().any(|k| req.get(k).is_some()) {
                return Err("queue's show takes no drop or clear".into());
            }
            let s = &self.sessions[i];
            let (h, interrupt) = s.turns.held_message(id).ok_or(format!("no held message {id}"))?;
            return Ok(json!({
                "ok": true,
                "session": s.id,
                "message": h.id,
                "text": h.text,
                "interrupt": interrupt,
                "attachments": h.blocks.len(),
                "blocks": h.blocks,
            }));
        }
        if req["clear_context"].as_bool() == Some(true) {
            self.check_experimental(Experimental::Context)?;
        }
        let (clear, clear_context) =
            (req["clear"].as_bool() == Some(true), req["clear_context"].as_bool() == Some(true));
        let queued = self.sessions[i].turns.queue(req["drop"].as_str(), clear, clear_context);
        let (dropped, todo) = queued?;
        self.act(i, todo);
        let s = &self.sessions[i];
        let (held, context) = (s.turns.queued(), s.turns.context());
        Ok(
            json!({ "ok": true, "session": s.id, "held": held, "context": context, "dropped": dropped }),
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
                let of = SetupOf::Config(peer, req_id);
                self.run_setup(Setup { session, steps, done: Vec::new(), of });
            }
            "fork" => {
                // A forked session would be a headless one in a process that
                // ends with the editor, which ACP can't tell of it (ADR 7.1).
                if self.editor_attached() {
                    return Err("the editor owns this process; fork sessions there".into());
                }
                self.check_strict(Beyond::Fork)?;
                if !caps.fork {
                    return Err("the agent can't fork sessions".into());
                }
                // A second session could never close when idle, and the
                // process would never stop (ADR 3.8).
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

    /// `new` and `resume` (`session new --pid`, `session resume --pid`,
    /// ADR 8.1): a session opened in this process with `session/new`, or
    /// `session/resume` or `session/load` (ADR 3.11), its settings set, and its
    /// prompt sent, committed as one as a start is (see requests.rs). Refused
    /// where `fork` is but in strict mode: these are stable ACP.
    fn open(&mut self, peer: u64, req: &Value) -> Result<(), String> {
        if self.status.is_some() || self.agent_in.is_none() || self.stop_requested {
            return Err("the agent is no longer accepting input".into());
        }
        // A session opened here would be a headless one in a process that
        // ends with the editor, which ACP can't tell of it (ADR 7.1).
        if self.editor_attached() {
            return Err("the editor owns this process; open sessions there".into());
        }
        if !self.start_done {
            return Err("the process is still starting".into());
        }
        let caps = self.caps;
        // A second session could never close when idle, and the process
        // would never stop (ADR 3.8).
        if self.stop_when_idle.is_some() && !caps.close {
            return Err("the agent can't close sessions: with stop_when_idle, a second session \
                        would never close"
                .into());
        }
        let cwd = match &req["cwd"] {
            Value::Null => self.cwd.clone(),
            Value::String(dir) if Path::new(dir).is_absolute() => PathBuf::from(dir),
            _ => return Err("cwd isn't an absolute path".into()),
        };
        let settings = settings_of(req)?;
        let text = req["text"].as_str().unwrap_or_default().to_owned();
        let blocks = match &req["blocks"] {
            Value::Null => Vec::new(),
            Value::Array(blocks) => blocks.clone(),
            _ => return Err("blocks must be a list of ACP content blocks".into()),
        };
        self.check_blocks(&blocks)?;
        let prompt =
            (!text.trim().is_empty() || !blocks.is_empty()).then(|| Prompt { text, blocks });
        // The command's: past it, the opening is abandoned (ADR 3.4).
        let deadline = match &req["timeout"] {
            Value::Null => None,
            secs => {
                let secs = secs.as_u64().ok_or("timeout must be a number of seconds")?;
                Instant::now().checked_add(Duration::from_secs(secs))
            }
        };
        let req_id = req.get("req_id").cloned();
        let dir = cwd.to_string_lossy().into_owned();
        let opening = Box::new(Opening { peer, req_id, cwd, settings, prompt, deadline });
        let mcp = json!(self.mcp_servers);
        if req["cmd"] == "new" {
            let params = json!({ "cwd": dir, "mcpServers": mcp });
            self.host_request("session/new", params, HostRequest::Opening(Open::New, opening));
            return Ok(());
        }
        let session = req["session"].as_str().ok_or("missing session")?.to_owned();
        if self.sessions.knows(&session) {
            return Err(format!("{session} is already open in this process"));
        }
        if !caps.resume && !caps.load {
            return Err(
                "the agent can't resume sessions (no session/resume or session/load)".into()
            );
        }
        // Taken before the agent hears of it: a session another process
        // holds is refused (ADR 3.9).
        self.sessions.own(&session)?;
        let params = json!({ "sessionId": session, "cwd": dir, "mcpServers": mcp });
        if caps.resume {
            let open = HostRequest::Opening(Open::Resume(session), opening);
            self.host_request("session/resume", params, open);
        } else {
            // The agent replays the history: recorded unless brnr has a
            // transcript of the session, in any folder, as for the editor's
            // load; one holding only openings, left by a load that failed,
            // isn't one (ADR 5.6).
            let record = !self.has_transcript(&session);
            let i = self.open_session(&session, Some(&dir));
            self.sessions[i].opening = true;
            self.begin_replay(i, record);
            let open = HostRequest::Opening(Open::Load(session), opening);
            self.host_request("session/load", params, open);
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
        if self.sessions[i].opening {
            return Err(format!("{wanted} is still opening"));
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
    fn adr_0010_a_message_and_its_acp_line_fit_at_once() {
        let (queue, _rx, _) = Queue::new();
        let big = 20_000_000;
        // Its ACP line, the turn's answer, its event, the turn's end.
        assert_eq!(taken(&queue, &[big, 200, big, 300]), 4);
    }

    #[test]
    fn adr_0010_a_peer_that_stopped_reading_is_cut_off_soon_after_the_limit() {
        let (queue, _rx, _) = Queue::new();
        assert_eq!(taken(&queue, &[1 << 20; 40]), 18, "the limit, the line past it, one more");

        // Lines longer than the limit are set aside up to ASIDE_BYTES.
        let (queue, _rx, queued) = Queue::new();
        let big = QUEUE_BYTES + 1;
        assert_eq!(taken(&queue, &[big; 10]), ASIDE_BYTES / big + 1, "set aside, and one more");
        assert!(queued.load(Relaxed) <= QUEUE_BYTES + ASIDE_BYTES + big);
    }

    /// A request is read up to the limit, its newline included, and no
    /// further: past it, the peer is cut off, and what comes after is never
    /// read (ADR 10.3).
    #[test]
    fn adr_0010_a_request_is_read_up_to_the_limit_and_no_further() {
        let xs = |n: usize| "x".repeat(n);
        let status = r#"{"cmd":"status"}"#;
        let input = format!("{status}\n{}\n{}{status}\n", xs(LINE_BYTES - 1), xs(LINE_BYTES));
        let (tx, rx) = mpsc::sync_channel(8);
        read_requests(input.as_bytes(), 7, &tx);
        drop(tx);
        let read: Vec<String> = rx
            .iter()
            .map(|ev| match ev {
                Ev::PeerRequest { req, .. } if req.get("parse_error").is_some() => "unread".into(),
                Ev::PeerRequest { req, .. } => req["cmd"].to_string(),
                Ev::PeerTooLong { peer } => format!("too long: {peer}"),
                _ => "other".into(),
            })
            .collect();
        assert_eq!(read, [r#""status""#, "unread", "too long: 7"]);
    }

    /// Past `max` connections open, one more is told so and closed, and no
    /// peer is made of it; once one has closed, the next is served
    /// (ADR 10.4).
    #[test]
    fn adr_0010_a_connection_past_the_cap_is_told_and_closed() {
        let dir = std::env::temp_dir().join(format!("brnr-cap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let socket = dir.join("h.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let (tx, rx) = mpsc::sync_channel(16);
        thread::spawn(move || serve_up_to(listener, tx, 2));
        let wait = Duration::from_secs(5);
        let opened = |rx: &Receiver<Ev>| matches!(rx.recv_timeout(wait), Ok(Ev::PeerOpened { .. }));
        let first = UnixStream::connect(&socket).unwrap();
        assert!(opened(&rx));
        let _second = UnixStream::connect(&socket).unwrap();
        assert!(opened(&rx));
        let mut told = String::new();
        UnixStream::connect(&socket).unwrap().read_to_string(&mut told).unwrap();
        assert_eq!(told, "{\"ok\":false,\"error\":\"too many connections: 2 open\"}\n");
        assert!(rx.try_recv().is_err(), "a peer was made of it");
        drop(first);
        assert!(matches!(rx.recv_timeout(wait), Ok(Ev::PeerClosed { .. })));
        let _third = UnixStream::connect(&socket).unwrap();
        assert!(opened(&rx), "not served once one closed");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An accept that fails is tried again after 10 ms, then twice as long
    /// each time it fails again, up to 1 s; a success starts again from
    /// 10 ms (ADR 10.4). (Running out of file descriptors for real would
    /// take them from every test in the process.)
    #[test]
    fn adr_0010_accept_errors_back_off_doubling_to_a_second() {
        let mut wait = None;
        let waits: Vec<u128> = (0..9)
            .map(|_| {
                wait = Some(backoff(wait));
                wait.unwrap().as_millis()
            })
            .collect();
        assert_eq!(waits, [10, 20, 40, 80, 160, 320, 640, 1000, 1000]);
        assert_eq!(backoff(None), ACCEPT_WAIT, "after a success");
    }
}
