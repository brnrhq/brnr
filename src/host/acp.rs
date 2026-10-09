//! The ACP stream: sessions, turns, injection, and the host as the agent's
//! client when no editor is attached.
//!
//! The host reads ACP line by line from both sides. It changes the stream
//! in these ways; everything else passes through unchanged:
//!
//! - The editor's `initialize` loses the `fs` and `terminal` client
//!   capabilities ([`DROPPED_CAPABILITIES`]), but in strict mode.
//! - An injected message goes to the agent as a `session/prompt` with a host
//!   id (`brnr-<n>`), never while a prompt is running: until then it is
//!   held. With `--steer` it goes into the running turn instead, as
//!   `_session/steering` (ADR 18 in docs/adr). The responses are kept from
//!   the editor, and the editor is shown the text as it is sent, as a
//!   completed tool call (`echo`). On an editor's session it is the
//!   experimental `send` (ADR 4), refused while a prompt runs.
//! - Held context is appended to the next `session/prompt`, whoever sends
//!   it.
//! - What the side channel does to an editor's session (ADR 4, see
//!   experimental.rs): a permission request the host answered in the
//!   editor's place is withdrawn from it (`$/cancel_request`), and its late
//!   answer dropped; brnr's notes to it take the echo's form; and a session
//!   brnr closed under it has its later requests answered by the host.
//! - With no editor attached, the host answers what the agent asks of its
//!   client. Permission requests wait for an approve or deny from a bridge
//!   or the CLI, until `permission_timeout` denies them; how much the agent
//!   asks is the agent's mode (`--mode`, see ADR 27 in docs/adr).
//!   Elicitation is declined, and anything else gets "method not found".
//! - While `session/load` replays a resumed session's history, the replayed
//!   updates are neither recorded nor turned into events: the transcript has
//!   them already. What they say the session is now (its title, mode, config
//!   options and commands) is kept.
//! - The editor's `session/load` or `session/resume` of a session another
//!   process holds is answered by the host with an error, naming the process
//!   and how to release it, and never reaches the agent (ADR 3); with
//!   `shared_sessions` in the profile (ADR 42) it goes through, and the
//!   session is served shared (see `Hold`).
//!
//! The editor's own steers (`_session/steering`) pass through untouched; once
//! the agent has taken one into the turn, it is a `user_message` by the
//! editor.
//!
//! Lines are read as json.rs reads them: any JSON text, a lone surrogate as
//! U+FFFD. One that isn't JSON at all passes through untracked (ADR 26 in
//! docs/adr): an editor's answer to an id the host doesn't know goes on to
//! the agent, but for its late answers to requests the host answered itself.
//! What the host acts on in a line is read with ACP's schema types, and
//! what they don't take as far as the host can (ADR 43, see schema.rs).

use std::collections::VecDeque;
use std::fs;
use std::mem::take;
use std::path::PathBuf;
use std::time::{Instant, SystemTime};

use serde_json::{Map, Value, json};

use super::flow::LINE_BYTES;
use super::requests::{HostRequest, PeerOp};
use super::state::SessionState;
use super::{Host, id_key, text_block};
use crate::config::Feature;
use crate::lock::{self, Lock};
use crate::log::{self, Dir};
use crate::schema::{
    self, ContentBlock, MessageId, NewSessionResponse, PermissionOptionKind, SessionUpdate,
};
use crate::{frame, json, paths};

/// Client capabilities removed from the editor's `initialize`. ACP v2 drops
/// them and they add nothing an agent needs, so no agent comes to rely on
/// them (ADR 2 in docs/adr).
const DROPPED_CAPABILITIES: &[&str] = &["fs", "terminal"];

/// A `session/prompt` the agent hasn't answered yet.
pub(super) struct Prompt {
    id: String,
    injected: bool,
    /// The `m<n>` ids of the messages its turn carries: the one the host sent
    /// as the prompt, if it did, and those steered into it (ADR 17).
    messages: Vec<String>,
}

/// A message accepted by the host and not yet sent as a prompt.
#[derive(Clone)]
pub(super) struct Held {
    /// `m<n>`: how `send --wait` finds the turn that answers it.
    pub(super) id: String,
    pub(super) text: String,
    /// Content blocks besides the text (`resource_link`, `image`).
    pub(super) blocks: Vec<Value>,
}

impl Held {
    /// As ACP content: the text, then the other blocks.
    fn content(&self) -> Vec<Value> {
        let text = (!self.text.is_empty()).then(|| text_block(&self.text));
        text.into_iter().chain(self.blocks.iter().cloned()).collect()
    }
}

pub(super) struct Session {
    pub(super) id: String,
    pub(super) cwd: PathBuf,
    /// Prompts sent and not yet answered, in the order they were sent.
    /// Non-empty means a turn is running.
    pub(super) prompts: VecDeque<Prompt>,
    /// Injected messages waiting for the session to go idle.
    pub(super) held: VecDeque<Held>,
    /// How many of `held`, from the front, are interrupts (or steers the
    /// agent turned back): a new one goes after them, so they keep the order
    /// they were sent in.
    pub(super) interrupts: usize,
    /// Messages steered into the running turn whose steer the agent hasn't
    /// answered, in the order sent. Nothing held goes while there are any.
    pub(super) steering: VecDeque<Held>,
    /// The `turn_ended` of a turn that ended while steers into it were
    /// unanswered: which messages it carried isn't known until the agent
    /// has answered them all (or they are dropped), so it waits (ADR 56).
    pub(super) ended: Option<Value>,
    /// Context to append to the next prompt.
    pub(super) context: Vec<String>,
    /// The agent message (or thought) so far, for the `agent_message` and
    /// `agent_thought` events.
    agent_text: String,
    agent_text_kind: &'static str,
    agent_message_id: Option<MessageId>,
    /// What the agent has said about the session (see state.rs).
    pub(super) state: SessionState,
    /// When the running turn started.
    pub(super) turn_started: Option<Instant>,
    /// `session/load` is replaying history (see the module docs).
    pub(super) replaying: bool,
    /// Since when nothing has been running, held or waiting for an answer
    /// (see `fire_idle_timers`); `idle_done` once its time ran out.
    idle_since: Option<Instant>,
    idle_done: bool,
    /// When it last had an event, or opened.
    pub(super) last_active: SystemTime,
    /// How the last turn ended: `{stop_reason, error}`, as `turn_ended`.
    pub(super) last_turn: Option<Value>,
    /// How this process holds it (ADR 3).
    pub(super) hold: Hold,
    /// A close asked for, until the agent has closed it: meanwhile the
    /// session takes no more requests (see `close`).
    pub(super) closing: Option<Close>,
}

/// How a process holds a session it serves (ADR 3).
pub(super) enum Hold {
    /// Its lock: the process is the session's owner, and keeps its
    /// transcript. `None` once it has let go, as the process exits.
    Owner(Option<Lock>),
    /// The lock couldn't be taken (why, naming its path), so whether another
    /// process holds it can't be known. A headless process never serves such
    /// a session (P4: the client refuses); an editor's passes it through and
    /// keeps its transcript all the same (P1, P4: the proxy passes on what it
    /// can't be sure of), and `status` says it isn't locked (ADR 50).
    Unlocked(String),
    /// Process `pid` held the lock when the editor loaded the session here
    /// (`shared_sessions`, ADR 42): that one keeps the transcript, and this
    /// one records the session in its host log only.
    Shared(u32),
}

pub(super) enum Close {
    /// `session/close` goes once the turn it cancelled has ended, and
    /// `peer` hears when the agent has closed it.
    AfterTurn {
        peer: u64,
        req_id: Option<Value>,
        by: &'static str,
    },
    Sent,
}

impl Session {
    pub(super) fn shared(&self) -> bool {
        matches!(self.hold, Hold::Shared(_))
    }

    /// Why this process isn't the session's owner, if it isn't, to follow
    /// "which": a headless process doesn't serve such a session (ADR 50).
    pub(super) fn not_owned(&self) -> Option<String> {
        match &self.hold {
            Hold::Owner(_) => None,
            Hold::Shared(pid) => Some(format!("is running in process {pid}")),
            Hold::Unlocked(why) => Some(format!("can't be locked: {why}")),
        }
    }

    /// Why its lock couldn't be taken, for one served without it.
    pub(super) fn lock_error(&self) -> Option<&str> {
        match &self.hold {
            Hold::Unlocked(why) => Some(why),
            _ => None,
        }
    }
}

/// A request whose response the host acts on: one that creates or ends a
/// session, the editor's `initialize`, or the editor's own steer.
pub(super) enum Pending {
    /// `session/new`. Its request is logged once the response says which
    /// session it belongs to.
    New {
        cwd: Option<String>,
        request: Option<Vec<u8>>,
        dir: Dir,
    },
    Attach {
        session: String,
        cwd: Option<String>,
    },
    Close {
        session: String,
    },
    /// The editor's `initialize`: the host notes what the agent can do.
    Initialize,
    /// The editor's `_session/steering`, passed through as it is: its text
    /// is a `user_message` once the agent has taken it into the turn.
    Steering {
        session: String,
        text: String,
    },
}

/// A request from the agent to its client.
pub(super) struct AgentRequest {
    pub(super) key: String,
    pub(super) id: Value,
    method: String,
    pub(super) session: Option<String>,
    pub(super) params: Value,
    /// `p<n>` for a permission request: what bridges and brnr call it.
    pub(super) handle: Option<String>,
    /// When `permission_timeout` denies it.
    pub(super) deadline: Option<Instant>,
}

#[derive(Clone, Copy)]
pub(super) enum Choice {
    Allow,
    Deny,
}

impl Host {
    // ---- editor → agent ------------------------------------------------

    /// Splits what the editor writes into lines. One longer than
    /// `LINE_BYTES` isn't read: it goes to the agent as it comes (ADR 51).
    pub(super) fn editor_bytes(&mut self, mut bytes: &[u8]) {
        if self.editor_long {
            let Some(i) = bytes.iter().position(|&b| b == b'\n') else {
                return self.write_agent(bytes);
            };
            self.write_agent(&bytes[..=i]);
            self.editor_long = false;
            bytes = &bytes[i + 1..];
        }
        // Only the new bytes are looked through: a long line arrives in many
        // reads.
        let mut from = self.editor_buf.len();
        self.editor_buf.extend_from_slice(bytes);
        let mut start = 0;
        while let Some(i) = self.editor_buf[from..].iter().position(|&b| b == b'\n') {
            let end = from + i;
            let line = start..end;
            (start, from) = (end + 1, end + 1);
            if start - line.start > LINE_BYTES {
                self.too_long("editor");
                let piece = self.editor_buf[line.start..start].to_vec();
                self.write_agent(&piece);
            } else {
                self.editor_line(self.editor_buf[line].to_vec());
            }
        }
        self.editor_buf.drain(..start);
        if self.editor_buf.len() > LINE_BYTES {
            self.too_long("editor");
            self.editor_long = true;
            let piece = take(&mut self.editor_buf);
            self.write_agent(&piece);
        }
    }

    /// A line past `LINE_BYTES` from `from` (ADR 51): with an editor it goes
    /// on unread, headless (from the agent) it is dropped.
    fn too_long(&mut self, from: &str) {
        let relayed = from == "editor" || self.link.is_some();
        self.emit_to_sessions(json!({
            "event": "line_too_long", "from": from, "limit": LINE_BYTES, "relayed": relayed,
        }));
    }

    /// A piece of a line from the agent past `LINE_BYTES`, `end` if the line
    /// ends with it: as `too_long` says.
    pub(super) fn agent_piece(&mut self, bytes: Vec<u8>, end: bool) {
        if !take(&mut self.agent_long) {
            self.too_long("agent");
        }
        self.agent_long = !end;
        self.send_link_owned(frame::DATA, bytes);
    }

    fn editor_line(&mut self, line: Vec<u8>) {
        let depth = json::depth(&line);
        let mut line = Some(line);
        let read = self.deep(depth, |host| {
            let line = line.take().unwrap();
            let msg = host.read(&line, depth);
            host.editor_message(line, msg);
        });
        if read.is_none() {
            // No stack to read it on: as a line that isn't JSON.
            self.sink.note(None, json!({ "event": "line-too-deep", "depth": depth }));
            self.editor_message(line.take().unwrap(), None);
        }
    }

    fn editor_message(&mut self, mut line: Vec<u8>, msg: Option<Value>) {
        let mut session = None;
        let mut deferred = None;
        let mut recorded = None;
        if let Some(Value::Object(mut msg)) = msg {
            session = param_session(&msg);
            let method = msg.get("method").and_then(Value::as_str).map(str::to_owned);
            match (method, msg.get("id").cloned()) {
                (Some(method), Some(id)) => {
                    if matches!(method.as_str(), "session/load" | "session/resume")
                        && let Some(sid) = &session
                        && let Err(error) = self.attach(sid)
                    {
                        return self.refuse(id, sid, &method, &error);
                    }
                    if let Some(sid) = &session
                        && let Some(error) = self.closed_error(sid, &method)
                    {
                        return self.refuse(id, sid, &method, &error);
                    }
                    let key = id_key(&id);
                    if let Some(rewritten) = self.editor_request(&method, &key, &mut msg, &session)
                    {
                        line = rewritten;
                    }
                    self.client_requests.insert(key.clone(), session.clone());
                    if method == "session/new" {
                        deferred = Some(key);
                    }
                }
                (None, Some(id)) => {
                    let key = id_key(&id);
                    if self.agent_requests.iter().any(|r| r.key == key) {
                        session = self.agent_request_answered(&key, &msg, "editor");
                    } else if self.late_answer(&key, &id, &msg) {
                        // Answered by the host already (a cancel, an approve
                        // or deny): the agent must not get a second answer.
                        return;
                    }
                    // Otherwise it answers a request the host couldn't read,
                    // or none: it goes on, and the agent ignores an answer to
                    // something it never asked (ADR 26 in docs/adr).
                }
                _ => {}
            }
            recorded = log::redacted(&msg);
        }
        line.push(b'\n');
        // A session/new goes in the host log now, so one the agent never
        // answers is still on record, and in the session's file once the
        // response names it. What is recorded keeps the MCP servers' secrets
        // out (ADR 25 in docs/adr); the agent gets the line as it came.
        let session = if deferred.is_some() { None } else { session };
        let recorded = recorded.as_deref().unwrap_or(&line);
        self.record(session.as_deref(), Dir::EditorToAgent, recorded);
        if let Some(Pending::New { request, .. }) =
            deferred.and_then(|key| self.pending.get_mut(&key))
        {
            *request = Some(recorded.to_vec());
        }
        self.write_agent(&line);
    }

    /// What the editor sent after its last line, as it closed its stdin: it
    /// goes to the agent as it is, and is recorded as a line would be.
    pub(super) fn record_editor_rest(&mut self, rest: &[u8]) {
        let msg = (json::depth(rest) <= self.stack).then(|| json::parse::<Value>(rest)).flatten();
        let recorded = msg.as_ref().and_then(Value::as_object).and_then(log::redacted);
        self.record(None, Dir::EditorToAgent, recorded.as_deref().unwrap_or(rest));
    }

    /// Notes requests that change session state. Returns the message
    /// re-encoded if the host changed it.
    fn editor_request(
        &mut self,
        method: &str,
        key: &str,
        msg: &mut Map<String, Value>,
        session: &Option<String>,
    ) -> Option<Vec<u8>> {
        let cwd = msg.get("params").and_then(|p| p["cwd"].as_str()).map(str::to_owned);
        match (method, session) {
            ("initialize", _) => {
                self.pending.insert(key.to_owned(), Pending::Initialize);
                return self.drop_capabilities(msg);
            }
            ("session/new", _) => {
                let pending = Pending::New { cwd, request: None, dir: Dir::EditorToAgent };
                self.pending.insert(key.to_owned(), pending);
            }
            ("session/load" | "session/resume", Some(session)) => {
                self.pending
                    .insert(key.to_owned(), Pending::Attach { session: session.clone(), cwd });
            }
            ("session/close", Some(session)) => {
                // Dropped now: held until the agent answers, they would go out as
                // the running turn ends.
                if let Some(i) = self.find(session) {
                    self.drop_all(i, "close");
                }
                self.pending.insert(key.to_owned(), Pending::Close { session: session.clone() });
            }
            ("session/prompt", Some(session)) => return self.editor_prompt(session, key, msg),
            ("_session/steering", Some(session)) => {
                let text = prompt_text(msg.get("params").and_then(|p| p.get("prompt")));
                let steering = Pending::Steering { session: session.clone(), text };
                self.pending.insert(key.to_owned(), steering);
            }
            _ => {}
        }
        None
    }

    /// The editor's `session/load` or `session/resume` of `session`: its lock
    /// is taken before the agent hears of it. One another process holds is
    /// refused, saying how to release it (ADR 3), unless the profile shares
    /// sessions (`shared_sessions`, ADR 42).
    fn attach(&mut self, session: &str) -> Result<(), String> {
        if self.find(session).is_some() || self.claimed.contains_key(session) {
            return Ok(()); // Ours already.
        }
        let hold = self.take_lock(session);
        if let Hold::Shared(pid) = hold
            && !self.features.contains(&Feature::SharedSessions)
        {
            return Err(held_elsewhere(session, pid));
        }
        self.claimed.insert(session.to_owned(), hold);
        Ok(())
    }

    /// Answers the editor's request `id` with an error in the agent's place:
    /// the agent never hears of it.
    fn refuse(&mut self, id: Value, session: &str, method: &str, error: &str) {
        let event = json!({
            "event": "editor-request-refused",
            "session_id": session,
            "id": id,
            "method": method,
            "error": error,
        });
        self.sink.note(None, event);
        let msg =
            json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32603, "message": error } });
        let mut line = serde_json::to_vec(&msg).unwrap();
        line.push(b'\n');
        self.record(None, Dir::ControlToEditor, &line);
        self.send_link(frame::DATA, &line);
    }

    fn drop_capabilities(&mut self, msg: &mut Map<String, Value>) -> Option<Vec<u8>> {
        if self.strict {
            return None; // As stable ACP v1 has them (ADR 41).
        }
        let caps = msg.get_mut("params")?.get_mut("clientCapabilities")?.as_object_mut()?;
        let dropped: Map<String, Value> = DROPPED_CAPABILITIES
            .iter()
            .filter_map(|&k| Some((k.to_owned(), caps.shift_remove(k)?)))
            .collect();
        if dropped.is_empty() {
            return None;
        }
        self.sink.note(None, json!({ "event": "capabilities-dropped", "dropped": dropped }));
        Some(serde_json::to_vec(&msg).unwrap())
    }

    /// Tracks the editor's prompt as the start of a turn and appends any
    /// held context to it.
    fn editor_prompt(
        &mut self,
        session: &str,
        key: &str,
        msg: &mut Map<String, Value>,
    ) -> Option<Vec<u8>> {
        let i = self.open_session(session, None);
        self.start_turn(i, Prompt { id: key.to_owned(), injected: false, messages: Vec::new() });
        self.prompt_session.insert(key.to_owned(), session.to_owned());
        let rewritten = self.attach_context(i, session, msg);
        let text = prompt_text(msg.get("params").and_then(|p| p.get("prompt")));
        self.flush_agent_message(i);
        self.emit(json!({
            "event": "user_message",
            "session": session,
            "by": "editor",
            "prompt": key,
            "text": text,
        }));
        rewritten
    }

    /// Appends held context to an editor prompt and echoes it.
    fn attach_context(
        &mut self,
        i: usize,
        session: &str,
        msg: &mut Map<String, Value>,
    ) -> Option<Vec<u8>> {
        let context = take(&mut self.sessions[i].context);
        if context.is_empty() {
            return None;
        }
        let Some(Value::Array(blocks)) = msg.get_mut("params").and_then(|p| p.get_mut("prompt"))
        else {
            self.sessions[i].context = context;
            return None;
        };
        let attached: Vec<Value> = context.iter().map(|t| text_block(t)).collect();
        blocks.extend(attached.iter().cloned());
        let event =
            json!({ "event": "context-attached", "to": "editor-prompt", "count": context.len() });
        self.sink.note(Some(session), event);
        self.echo(session, "Context via brnr", &attached);
        Some(serde_json::to_vec(&msg).unwrap())
    }

    // ---- agent → editor ------------------------------------------------

    pub(super) fn agent_line(&mut self, line: &[u8]) {
        let body = line.strip_suffix(b"\n").unwrap_or(line);
        let depth = json::depth(body);
        let read = self.deep(depth, |host| {
            let msg = host.read(body, depth);
            host.agent_message(line, msg);
        });
        if read.is_none() {
            // No stack to read it on: as a line that isn't JSON.
            self.sink.note(None, json!({ "event": "line-too-deep", "depth": depth }));
            self.agent_message(line, None);
        }
    }

    fn agent_message(&mut self, line: &[u8], msg: Option<Value>) {
        let Some(Value::Object(msg)) = msg else {
            self.record(None, Dir::AgentToEditor, line);
            self.send_link(frame::DATA, line);
            return;
        };
        let method = msg.get("method").and_then(Value::as_str).map(str::to_owned);
        match (method, msg.get("id").cloned()) {
            (None, Some(id)) => self.agent_response(&id_key(&id), &msg, line),
            (Some(method), Some(id)) => self.agent_request(method, id, &msg, line),
            (method, None) => {
                let session = param_session(&msg);
                let replaying = session
                    .as_deref()
                    .and_then(|s| self.find(s))
                    .is_some_and(|i| self.sessions[i].replaying);
                if replaying {
                    // History the transcript has already; no editor to show it.
                    if method.as_deref() == Some("session/update")
                        && let Some(i) = session.as_deref().and_then(|s| self.find(s))
                    {
                        self.replayed_state(i, &msg["params"]["update"]);
                    }
                    return;
                }
                if method.as_deref() == Some("session/update")
                    && let Some(session) = &session
                {
                    self.track_update(session, &msg["params"]["update"]);
                }
                self.record(session.as_deref(), Dir::AgentToEditor, line);
                self.send_link(frame::DATA, line);
            }
        }
    }

    fn agent_response(&mut self, key: &str, msg: &Map<String, Value>, line: &[u8]) {
        let mut session = self.client_requests.remove(key).flatten();
        let ours = self.host_requests.remove(key);
        if let Some(pending) = self.pending.remove(key) {
            session = self.session_changed(pending, msg.get("result")).or(session);
        }
        let mut turn = None;
        if let Some(sid) = self.prompt_session.remove(key) {
            let mut prompt = None;
            if let Some(i) = self.find(&sid) {
                let s = &mut self.sessions[i];
                if let Some(pos) = s.prompts.iter().position(|p| p.id == key) {
                    prompt = s.prompts.remove(pos);
                }
                if s.prompts.is_empty() {
                    s.turn_started = None;
                }
            }
            let injected = prompt.as_ref().is_some_and(|p| p.injected);
            let messages = prompt.map(|p| p.messages).unwrap_or_default();
            session = Some(sid.clone());
            turn = Some((sid, injected, messages));
        }
        let forward = ours.is_none() && !turn.as_ref().is_some_and(|(_, injected, _)| *injected);
        let dir = if forward { Dir::AgentToEditor } else { Dir::AgentToControl };
        self.record(session.as_deref(), dir, line);
        if forward {
            self.send_link(frame::DATA, line);
        }
        if let Some(request) = ours {
            self.host_request_done(request, msg);
        }
        if let Some((sid, injected, messages)) = turn {
            let stop_reason = msg.get("result").and_then(|r| r.get("stopReason"));
            if let Some(i) = self.find(&sid) {
                self.flush_agent_message(i);
                self.sessions[i].last_turn =
                    Some(json!({ "stop_reason": stop_reason, "error": msg.get("error") }));
            }
            let event = json!({
                "event": "turn_ended",
                "session": sid,
                "by": if injected { "control" } else { "editor" },
                "prompt": key,
                "messages": messages,
                "stop_reason": stop_reason,
                "error": msg.get("error"),
            });
            // Waits for the steers into the turn, if the agent hasn't
            // answered them all (see `Session::ended`).
            match self.find(&sid) {
                Some(i) if !self.sessions[i].steering.is_empty() => {
                    self.sessions[i].ended = Some(event);
                }
                _ => {
                    self.emit(event);
                }
            }
            self.next_turn(&sid);
        }
    }

    /// Returns the session the response was about.
    fn session_changed(&mut self, pending: Pending, result: Option<&Value>) -> Option<String> {
        match pending {
            Pending::New { cwd, request, dir } => {
                let session = result.and_then(new_session);
                if let Some(session) = &session {
                    let i = self.open_session(session, cwd.as_deref());
                    self.sessions[i].state.result(result.unwrap_or(&Value::Null));
                }
                if let Some(request) = request {
                    self.sink.msg(session.as_deref(), dir, &request);
                }
                session
            }
            Pending::Attach { session, cwd } => {
                if let Some(result) = result {
                    let i = self.open_session(&session, cwd.as_deref());
                    self.sessions[i].state.result(result);
                } else {
                    self.claimed.remove(&session); // Its lock goes (see `attach`).
                }
                Some(session)
            }
            Pending::Close { session } => {
                if result.is_some()
                    && let Some(i) = self.find(&session)
                {
                    self.close_session(i, "editor");
                }
                Some(session)
            }
            Pending::Initialize => {
                if let Some(result) = result {
                    self.initialized(result);
                }
                None
            }
            Pending::Steering { session, text } => {
                if result.is_some_and(|r| r["outcome"] == "injected") {
                    self.editor_steered(&session, text);
                }
                Some(session)
            }
        }
    }

    fn agent_request(&mut self, method: String, id: Value, msg: &Map<String, Value>, line: &[u8]) {
        let session = param_session(msg);
        self.record(session.as_deref(), Dir::AgentToEditor, line);
        let mut req = AgentRequest {
            key: id_key(&id),
            id,
            method,
            session,
            params: msg.get("params").cloned().unwrap_or(Value::Null),
            handle: None,
            deadline: None,
        };
        if req.method == "session/request_permission" {
            self.next_permission += 1;
            req.handle = Some(format!("p{}", self.next_permission));
        }
        if !self.editor_attached() {
            return self.answer_as_client(req);
        }
        self.send_link(frame::DATA, line);
        let event = req.handle.is_some().then(|| permission_event(&req, "editor"));
        self.agent_requests.push(req);
        if let Some(event) = event {
            self.emit(event);
        }
    }

    /// The editor answered one of the agent's requests.
    fn agent_request_answered(
        &mut self,
        key: &str,
        msg: &Map<String, Value>,
        by: &str,
    ) -> Option<String> {
        let pos = self.agent_requests.iter().position(|r| r.key == key)?;
        let req = self.agent_requests.remove(pos);
        if let Some(handle) = &req.handle {
            self.emit(json!({
                "event": "permission_resolved",
                "session": req.session,
                "request": handle,
                "outcome": msg.get("result").map(|r| r["outcome"].clone()),
                "by": by,
            }));
        }
        req.session
    }

    // ---- the host as the agent's client --------------------------------

    fn answer_as_client(&mut self, mut req: AgentRequest) {
        match req.method.as_str() {
            "session/request_permission" => {
                // None, never, for a timeout too far off to say.
                req.deadline = self.permission_timeout.and_then(|t| Instant::now().checked_add(t));
                let event = permission_event(&req, "headless");
                self.agent_requests.push(req);
                self.emit(event);
            }
            "elicitation/create" => {
                self.respond(&req, json!({ "result": { "action": "decline" } }))
            }
            method => {
                let message = format!("{method} is not supported without an editor");
                self.respond(&req, json!({ "error": { "code": -32601, "message": message } }));
            }
        }
    }

    /// Answers permission request `handle`: with `option` if given (not one
    /// of the other kind: a deny can't pick an allow option), else the first
    /// allow (or reject) option. Denying a request that offers no reject
    /// option cancels it. On an editor's session, where this is the
    /// experimental `approve` (see experimental.rs), the request is then
    /// withdrawn from the editor.
    pub(super) fn resolve_permission(
        &mut self,
        handle: &str,
        choice: Choice,
        option: Option<&str>,
        by: &str,
    ) -> Result<Value, String> {
        let pos = self
            .agent_requests
            .iter()
            .position(|r| r.handle.as_deref() == Some(handle))
            .ok_or_else(|| format!("no pending request {handle}"))?;
        if self.status.is_some() || self.agent_in.is_none() {
            return Err("the agent is no longer accepting input".into());
        }
        let options =
            self.agent_requests[pos].params["options"].as_array().cloned().unwrap_or_default();
        // An option's kind, if the schema knows it.
        let kind = |o: &Value| schema::read::<PermissionOptionKind>(&o["kind"]);
        let allow = matches!(choice, Choice::Allow);
        let outcome = match option {
            Some(option) => {
                let Some(chosen) = options.iter().find(|o| o["optionId"] == option) else {
                    let ids: Vec<&str> =
                        options.iter().filter_map(|o| o["optionId"].as_str()).collect();
                    return Err(format!(
                        "{handle} has no option {option} (options: {})",
                        ids.join(", ")
                    ));
                };
                if kind(chosen).is_some_and(|k| allows(k) != allow) {
                    let kind = chosen["kind"].as_str().unwrap_or_default();
                    let verb = if allow { "deny" } else { "approve" };
                    return Err(format!("{handle}: {option} ({kind}) is for brnr {verb}"));
                }
                json!({ "outcome": "selected", "optionId": option })
            }
            None => {
                use PermissionOptionKind::{AllowAlways, AllowOnce, RejectAlways, RejectOnce};
                let kinds =
                    if allow { [AllowOnce, AllowAlways] } else { [RejectOnce, RejectAlways] };
                match kinds.iter().find_map(|k| options.iter().find(|o| kind(o) == Some(*k))) {
                    Some(o) => json!({ "outcome": "selected", "optionId": o["optionId"] }),
                    None if matches!(choice, Choice::Deny) => json!({ "outcome": "cancelled" }),
                    None => return Err(format!("{handle} offers no allow option")),
                }
            }
        };
        let req = self.agent_requests.remove(pos);
        self.respond(&req, json!({ "result": { "outcome": outcome } }));
        let how = match choice {
            Choice::Allow => "approved",
            Choice::Deny => "denied",
        };
        self.withdraw(&req, how, by);
        self.emit(json!({
            "event": "permission_resolved",
            "session": req.session,
            "request": handle,
            "outcome": outcome,
            "by": by,
        }));
        Ok(outcome)
    }

    /// ACP: a client that cancels a turn answers that session's pending
    /// permission requests with `cancelled`. If the editor is showing one,
    /// it is withdrawn, and its late answer dropped (see experimental.rs).
    fn cancel_permissions(&mut self, session: &str) {
        let (cancel, keep): (Vec<_>, Vec<_>) = take(&mut self.agent_requests)
            .into_iter()
            .partition(|r| r.handle.is_some() && r.session.as_deref() == Some(session));
        self.agent_requests = keep;
        for req in cancel {
            self.respond(&req, json!({ "result": { "outcome": { "outcome": "cancelled" } } }));
            self.withdraw(&req, "cancelled", "cancel");
            self.emit(json!({
                "event": "permission_resolved",
                "session": req.session,
                "request": req.handle,
                "outcome": { "outcome": "cancelled" },
                "by": "cancel",
            }));
        }
    }

    fn respond(&mut self, req: &AgentRequest, body: Value) {
        let mut msg = json!({ "jsonrpc": "2.0", "id": req.id });
        if let (Some(msg), Value::Object(body)) = (msg.as_object_mut(), body) {
            msg.extend(body);
        }
        let mut line = serde_json::to_vec(&msg).unwrap();
        line.push(b'\n');
        self.record(req.session.as_deref(), Dir::ControlToAgent, &line);
        self.write_agent(&line);
    }

    // ---- turns and injection -------------------------------------------

    /// Collects agent message and thought text for the `agent_message` and
    /// `agent_thought` events, and passes everything else to state.rs. An
    /// update the schema doesn't take changes nothing (see schema.rs).
    fn track_update(&mut self, session: &str, raw: &Value) {
        let Some(i) = self.find(session) else { return };
        let Some(update) = schema::read::<SessionUpdate>(raw) else { return };
        let (text_kind, chunk) = match &update {
            SessionUpdate::AgentMessageChunk(chunk) => ("agent_message", chunk),
            SessionUpdate::AgentThoughtChunk(chunk) => ("agent_thought", chunk),
            // These end the agent message (or thought) being assembled;
            // bookkeeping updates (usage, commands, mode) don't.
            SessionUpdate::UserMessageChunk(_)
            | SessionUpdate::ToolCall(_)
            | SessionUpdate::ToolCallUpdate(_)
            | SessionUpdate::Plan(_) => {
                self.flush_agent_message(i);
                return self.track_state(i, &update, raw);
            }
            _ => return self.track_state(i, &update, raw),
        };
        let s = &self.sessions[i];
        if s.agent_message_id != chunk.message_id || s.agent_text_kind != text_kind {
            self.flush_agent_message(i);
        }
        let s = &mut self.sessions[i];
        s.agent_message_id = chunk.message_id.clone();
        s.agent_text_kind = text_kind;
        if let ContentBlock::Text(text) = &chunk.content {
            s.agent_text.push_str(&text.text);
        }
    }

    pub(super) fn flush_agent_message(&mut self, i: usize) {
        let text = take(&mut self.sessions[i].agent_text);
        if text.is_empty() {
            return;
        }
        let s = &mut self.sessions[i];
        let kind = s.agent_text_kind;
        if kind == "agent_message" {
            s.state.last_message = Some(text.clone());
        }
        let session = s.id.clone();
        self.emit(json!({ "event": kind, "session": session, "text": text }));
    }

    /// After a prompt or a steer is answered: once the session is idle, and
    /// no steer is waiting for its answer, send the next held message.
    fn next_turn(&mut self, session: &str) {
        let Some(i) = self.find(session) else { return };
        let s = &mut self.sessions[i];
        if s.prompts.is_empty() && matches!(s.closing, Some(Close::AfterTurn { .. })) {
            if let Some(Close::AfterTurn { peer, req_id, by }) = s.closing.take() {
                self.send_close(i, peer, req_id, by);
            }
            return;
        }
        if s.prompts.is_empty()
            && s.steering.is_empty()
            && let Some(held) = s.held.pop_front()
        {
            s.interrupts = s.interrupts.saturating_sub(1);
            self.send_prompt(i, held);
        }
    }

    /// What a session that is closing or whose process is exiting held,
    /// messages and context alike, is dropped, each with its event.
    pub(super) fn drop_all(&mut self, i: usize, by: &str) {
        self.drop_held(i, by);
        self.drop_context(i, by);
    }

    /// Context session `i` held for its next prompt, which won't come: each
    /// is a `context_dropped` event, `by` `close`, `exit` or `queue` (ADR 20;
    /// context has no message id, ADR 17). Returns the texts.
    pub(super) fn drop_context(&mut self, i: usize, by: &str) -> Vec<String> {
        let context = take(&mut self.sessions[i].context);
        let session = self.sessions[i].id.clone();
        for text in &context {
            self.emit(json!({
                "event": "context_dropped",
                "session": session,
                "text": text,
                "by": by,
            }));
        }
        context
    }

    /// Drops everything session `i` holds (see `dropped`), and the steers
    /// the agent hasn't answered: whatever it answers, they don't go out.
    pub(super) fn drop_held(&mut self, i: usize, by: &str) -> Vec<Value> {
        let s = &mut self.sessions[i];
        s.interrupts = 0;
        let held: Vec<Held> = s.steering.drain(..).chain(s.held.drain(..)).collect();
        let dropped = self.dropped(i, held, by);
        self.turn_answered(i);
        dropped
    }

    /// The `turn_ended` that waited for the steers into its turn (see
    /// `Session::ended`), once none is left unanswered.
    fn turn_answered(&mut self, i: usize) {
        if self.sessions[i].steering.is_empty()
            && let Some(event) = self.sessions[i].ended.take()
        {
            self.emit(event);
        }
    }

    /// Messages taken from session `i`'s held ones, never to be sent: each is
    /// a `message_dropped` event, `by` `cancel`, `queue`, `close` or `exit`
    /// (ADR 20), or `steer` for a steer the agent refused. Returns them as
    /// `{message, text}`, for a response.
    pub(super) fn dropped(&mut self, i: usize, held: Vec<Held>, by: &str) -> Vec<Value> {
        let session = self.sessions[i].id.clone();
        held.into_iter()
            .map(|h| {
                self.emit(json!({
                    "event": "message_dropped",
                    "session": session,
                    "message": h.id,
                    "text": h.text,
                    "by": by,
                }));
                json!({ "message": h.id, "text": h.text })
            })
            .collect()
    }

    /// Closes session `i`, `by` `close` (`brnr close`, `--take-over`) or
    /// `idle`: a running turn is cancelled first, its pending approvals
    /// answered `cancelled`, and `session/close` goes once it has ended
    /// (ADR 16). `peer` hears when the agent has closed it (peer 0: nobody).
    pub(super) fn close(&mut self, i: usize, peer: u64, req_id: Option<Value>, by: &'static str) {
        // Dropped now: held until the agent answers, they would go out as the
        // running turn ends.
        self.drop_all(i, "close");
        if !self.is_idle(i) {
            let session = self.sessions[i].id.clone();
            self.cancel(&session);
        }
        if self.sessions[i].prompts.is_empty() {
            self.send_close(i, peer, req_id, by);
        } else {
            self.sessions[i].closing = Some(Close::AfterTurn { peer, req_id, by });
        }
    }

    fn send_close(&mut self, i: usize, peer: u64, req_id: Option<Value>, by: &'static str) {
        let session = self.sessions[i].id.clone();
        self.sessions[i].closing = Some(Close::Sent);
        let params = json!({ "sessionId": session });
        self.peer_op(peer, req_id, PeerOp::Close { session, by }, "session/close", params);
    }

    /// The agent has closed session `i`, `by` `close`, `idle` or `editor`:
    /// what it still holds is dropped, then `session_closed` (ADR 20).
    pub(super) fn close_session(&mut self, i: usize, by: &str) {
        self.flush_agent_message(i);
        self.drop_all(i, "close");
        let session = self.sessions.remove(i).id;
        self.emit(json!({ "event": "session_closed", "session": session, "by": by }));
    }

    /// Whether session `i` has nothing running, held or waiting for an
    /// answer.
    pub(super) fn is_idle(&self, i: usize) -> bool {
        let s = &self.sessions[i];
        s.prompts.is_empty()
            && s.held.is_empty()
            && s.steering.is_empty()
            && !self
                .agent_requests
                .iter()
                .any(|r| r.handle.is_some() && r.session.as_ref() == Some(&s.id))
    }

    /// `stop_when_idle`: a headless session idle that long closes, and the
    /// process stops with its last session. Idle time counts from the
    /// commit too, so a session started without a prompt doesn't run
    /// forever; one with a prompt is busy from then.
    pub(super) fn fire_idle_timers(&mut self, now: Instant) {
        let Some(limit) = self.stop_when_idle else { return };
        if self.editor_attached() || !self.start_done || self.stop_requested {
            return;
        }
        for i in 0..self.sessions.len() {
            let idle = self.is_idle(i);
            let s = &mut self.sessions[i];
            if !idle {
                (s.idle_since, s.idle_done) = (None, false);
            } else if s.idle_since.is_none() {
                s.idle_since = Some(now);
            }
        }
        let Some(i) = (0..self.sessions.len()).find(|&i| {
            let s = &self.sessions[i];
            let until = s.idle_since.and_then(|t| t.checked_add(limit)); // None: never.
            !s.idle_done && s.closing.is_none() && until.is_some_and(|t| now >= t)
        }) else {
            return;
        };
        self.sessions[i].idle_done = true;
        let session = self.sessions[i].id.clone();
        self.sink.note(Some(&session), json!({ "event": "idle-timeout" }));
        if self.sessions.len() == 1 {
            self.begin_stop();
        } else if self.caps.close {
            // As `brnr close` would, with nobody to answer (peer 0).
            self.close(i, 0, None, "idle");
        }
    }

    /// The next idle session's time running out, for the event loop.
    pub(super) fn next_idle_deadline(&self) -> Option<Instant> {
        let limit = self.stop_when_idle?;
        if self.editor_attached() || !self.start_done || self.stop_requested {
            return None;
        }
        // A session that just went idle has no `idle_since` until the loop
        // comes round; this wakes it then.
        (0..self.sessions.len())
            .filter(|&i| !self.sessions[i].idle_done && self.is_idle(i))
            .filter_map(|i| match self.sessions[i].idle_since {
                None => Some(Instant::now()),
                Some(t) => t.checked_add(limit), // None: never.
            })
            .min()
    }

    /// A new `m<n>` message id.
    pub(super) fn message_id(&mut self) -> String {
        self.next_message += 1;
        format!("m{}", self.next_message)
    }

    fn start_turn(&mut self, i: usize, prompt: Prompt) {
        let s = &mut self.sessions[i];
        if s.prompts.is_empty() {
            s.turn_started = Some(Instant::now());
        }
        s.prompts.push_back(prompt);
    }

    pub(super) fn send_prompt(&mut self, i: usize, held: Held) {
        self.next_id += 1;
        let id = Value::String(format!("brnr-{}", self.next_id));
        let key = id_key(&id);
        let session = self.sessions[i].id.clone();
        let context = take(&mut self.sessions[i].context);
        let mut blocks = held.content();
        blocks.extend(context.iter().map(|t| text_block(t)));
        let msg = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "session/prompt",
            "params": { "sessionId": session, "prompt": blocks },
        });
        let prompt = Prompt { id: key.clone(), injected: true, messages: vec![held.id.clone()] };
        self.start_turn(i, prompt);
        self.prompt_session.insert(key.clone(), session.clone());
        self.client_requests.insert(key.clone(), Some(session.clone()));
        let text = prompt_text(Some(&json!(blocks)));
        self.echo(&session, "Message via brnr", &blocks);
        self.emit(json!({
            "event": "user_message",
            "session": session,
            "by": "control",
            "message": held.id,
            "prompt": key,
            "text": text,
        }));
        let mut line = serde_json::to_vec(&msg).unwrap();
        line.push(b'\n');
        self.record(Some(&session), Dir::ControlToAgent, &line);
        self.write_agent(&line);
    }

    /// `send --steer`: `held` goes into session `i`'s running turn as
    /// `_session/steering`, asking the agent to answer `promptRequired`
    /// rather than start a turn of its own if none is running
    /// (claude-agent-acp's `steer()` has the contract). Held context waits
    /// for the next prompt.
    pub(super) fn steer(&mut self, i: usize, held: Held) {
        let session = self.sessions[i].id.clone();
        let params = json!({
            "sessionId": session,
            "prompt": held.content(),
            "_meta": { "steering": { "idleBehavior": "promptRequired" } },
        });
        let message = held.id.clone();
        self.sessions[i].steering.push_back(held);
        self.host_request("_session/steering", params, HostRequest::Steer { session, message });
    }

    /// The agent's answer to steering `message`. `injected`: it is in the
    /// running turn, whose `turn_ended` lists it, even if the turn's own
    /// answer came first (its `turn_ended` waits for the steers').
    /// `promptRequired`: the turn ended first, and it goes next as a prompt
    /// of its own, ahead of what is held as it would have been in the turn
    /// (after interrupts, which end that turn). Anything else, an error
    /// too, drops it.
    pub(super) fn steer_answered(
        &mut self,
        session: &str,
        message: &str,
        msg: &Map<String, Value>,
    ) {
        let Some(i) = self.find(session) else { return };
        let s = &mut self.sessions[i];
        // Gone meanwhile (a cancel, a close): it doesn't go out.
        let Some(pos) = s.steering.iter().position(|h| h.id == message) else { return };
        let held = s.steering.remove(pos).unwrap();
        match msg.get("result").and_then(|r| r["outcome"].as_str()) {
            Some("injected") => self.injected(i, held),
            Some("promptRequired") => {
                s.held.insert(s.interrupts, held);
                s.interrupts += 1;
            }
            _ => {
                let answer = msg.get("error").or(msg.get("result"));
                let event = json!({ "event": "steer-refused", "answer": answer });
                self.sink.note(Some(session), event);
                self.dropped(i, vec![held], "steer");
            }
        }
        self.turn_answered(i);
        self.next_turn(session);
    }

    /// A steered message the agent took into session `i`'s turn, which
    /// carries it from now: it is shown as sent, as `send_prompt` shows a
    /// prompt. The turn is the running one, or the one that ended before the
    /// agent answered (whose `turn_ended` is waiting for it).
    fn injected(&mut self, i: usize, held: Held) {
        let s = &mut self.sessions[i];
        let session = s.id.clone();
        let key = if let Some(ended) = &mut s.ended {
            if let Some(messages) = ended["messages"].as_array_mut() {
                messages.push(json!(held.id));
            }
            ended["prompt"].as_str().unwrap_or_default().to_owned()
        } else if let Some(prompt) = s.prompts.front_mut() {
            prompt.messages.push(held.id.clone());
            prompt.id.clone()
        } else {
            // No turn to carry it. A steer goes while a turn runs, or while
            // one's `turn_ended` waits for an earlier steer's answer, so this
            // shouldn't happen; if it does, it isn't left without an end.
            let event = json!({ "event": "steer-after-turn", "message": held.id });
            self.sink.note(Some(&session), event);
            self.dropped(i, vec![held], "steer");
            return;
        };
        let blocks = held.content();
        let text = prompt_text(Some(&json!(blocks)));
        self.echo(&session, "Message via brnr", &blocks);
        self.emit(json!({
            "event": "user_message",
            "session": session,
            "by": "control",
            "message": held.id,
            "prompt": key,
            "text": text,
        }));
    }

    /// The editor's own steer, which the agent took into the running turn of
    /// `session`: a `user_message` of that turn, by the editor, as brnr's own
    /// steers are (see `injected`). The steer itself went through untouched.
    fn editor_steered(&mut self, session: &str, text: String) {
        let Some(i) = self.find(session) else { return };
        let Some(key) = self.sessions[i].prompts.front().map(|p| p.id.clone()) else {
            let event = json!({ "event": "steer-after-turn", "by": "editor" });
            return self.sink.note(Some(session), event);
        };
        self.flush_agent_message(i);
        self.emit(json!({
            "event": "user_message",
            "session": session,
            "by": "editor",
            "prompt": key,
            "text": text,
        }));
    }

    /// Denies permission requests nobody answered within
    /// `permission_timeout`.
    pub(super) fn fire_permission_timers(&mut self, now: Instant) {
        if self.editor_attached() {
            return;
        }
        let expired: Vec<String> = self
            .agent_requests
            .iter()
            .filter(|r| r.deadline.is_some_and(|d| now >= d))
            .filter_map(|r| r.handle.clone())
            .collect();
        for handle in expired {
            if let Some(req) =
                self.agent_requests.iter_mut().find(|r| r.handle.as_ref() == Some(&handle))
            {
                req.deadline = None;
            }
            if let Err(err) = self.resolve_permission(&handle, Choice::Deny, None, "timeout") {
                let event = json!({ "event": "timeout-failed", "request": handle, "error": err });
                self.sink.note(None, event);
            }
        }
    }

    /// The earliest permission deadline, for the event loop's wake-up.
    pub(super) fn next_permission_deadline(&self) -> Option<Instant> {
        if self.editor_attached() {
            return None;
        }
        self.agent_requests.iter().filter_map(|r| r.deadline).min()
    }

    pub(super) fn cancel(&mut self, session: &str) {
        let msg = json!({
            "jsonrpc": "2.0",
            "method": "session/cancel",
            "params": { "sessionId": session },
        });
        let mut line = serde_json::to_vec(&msg).unwrap();
        line.push(b'\n');
        self.record(Some(session), Dir::ControlToAgent, &line);
        self.write_agent(&line);
        self.cancel_permissions(session);
    }

    /// Shows the editor an injected message, as a completed tool call:
    /// the one update an editor renders as a block of its own wherever the
    /// turn is. A `user_message_chunk` out of turn is not rendered (see
    /// ADR 5 in docs/adr). brnr's notes to the editor take the same form
    /// (see experimental.rs).
    pub(super) fn echo(&mut self, session: &str, title: &str, blocks: &[Value]) {
        if let Some(i) = self.find(session) {
            self.flush_agent_message(i);
        }
        if !self.editor_attached() {
            return;
        }
        self.next_id += 1;
        let content: Vec<Value> =
            blocks.iter().map(|b| json!({ "type": "content", "content": b })).collect();
        self.update_editor(
            session,
            json!({
                "sessionUpdate": "tool_call",
                "toolCallId": format!("brnr-echo-{}", self.next_id),
                "title": title,
                "kind": "other",
                "status": "completed",
                "content": content,
            }),
        );
    }

    /// A `session/update` of brnr's own for the editor.
    pub(super) fn update_editor(&mut self, session: &str, update: Value) {
        let params = json!({ "sessionId": session, "update": update });
        let msg = json!({ "jsonrpc": "2.0", "method": "session/update", "params": params });
        self.send_editor(Some(session), &msg);
    }

    /// A message of brnr's own for the editor, recorded as such; none
    /// without an editor.
    pub(super) fn send_editor(&mut self, session: Option<&str>, msg: &Value) {
        if !self.editor_attached() {
            return;
        }
        let mut line = serde_json::to_vec(msg).unwrap();
        line.push(b'\n');
        self.record(session, Dir::ControlToEditor, &line);
        self.send_link(frame::DATA, &line);
    }

    // ---- sessions ------------------------------------------------------

    pub(super) fn find(&self, session: &str) -> Option<usize> {
        self.sessions.iter().position(|s| s.id == session)
    }

    /// Index of `session`, started (with its lock and log file) if this is
    /// the first we hear of it. Without a cwd, the host's own is used.
    pub(super) fn open_session(&mut self, session: &str, cwd: Option<&str>) -> usize {
        if let Some(i) = self.find(session) {
            return i;
        }
        let cwd = cwd.map(PathBuf::from).unwrap_or_else(|| self.cwd.clone());
        let hold = match self.claimed.remove(session) {
            Some(hold) => hold,
            None => self.take_lock(session),
        };
        match hold {
            Hold::Shared(pid) => {
                let event =
                    json!({ "event": "session-shared", "session_id": session, "held_by": pid });
                self.sink.note(None, event);
            }
            Hold::Owner(_) => self.sink.open_session(session, &cwd),
            // One the headless process won't serve gets no transcript.
            Hold::Unlocked(_) if self.editor_attached() => self.sink.open_session(session, &cwd),
            Hold::Unlocked(_) => {}
        }
        self.sessions.push(Session {
            id: session.to_owned(),
            cwd,
            prompts: VecDeque::new(),
            held: VecDeque::new(),
            context: Vec::new(),
            interrupts: 0,
            steering: VecDeque::new(),
            ended: None,
            agent_text: String::new(),
            agent_text_kind: "agent_message",
            agent_message_id: None,
            state: SessionState::default(),
            turn_started: None,
            replaying: false,
            idle_since: None,
            idle_done: false,
            last_active: SystemTime::now(),
            last_turn: None,
            hold,
            closing: None,
        });
        self.sessions.len() - 1
    }

    /// `session`'s lock, for a session this process is to serve. One another
    /// process holds is shared, as `attach` lets it be; one that can't be
    /// locked is `Unlocked`, and the host log says why. Whoever opened it
    /// decides what becomes of either (ADR 3, ADR 50).
    fn take_lock(&mut self, session: &str) -> Hold {
        match lock::take(session) {
            Ok(lock) => Hold::Owner(Some(lock)),
            Err(lock::Error::Held(pid)) => Hold::Shared(pid),
            Err(lock::Error::Io(error)) => {
                let event =
                    json!({ "event": "lock-failed", "session_id": session, "error": error });
                self.sink.note(None, event);
                Hold::Unlocked(error)
            }
        }
    }

    /// Takes the lock of `session`, which a headless start resumes, before
    /// the agent is asked for it: one another process holds, or one that
    /// can't be locked, can't be served here (ADR 3, ADR 50).
    pub(super) fn own(&mut self, session: &str) -> Result<(), String> {
        let lock = lock::take(session).map_err(|err| match err {
            lock::Error::Held(pid) => format!("{session} is running in process {pid}"),
            lock::Error::Io(why) => format!("{session} can't be locked: {why}"),
        })?;
        self.claimed.insert(session.to_owned(), Hold::Owner(Some(lock)));
        Ok(())
    }
}

/// What the editor is told of a session process `pid` holds: how to
/// release it.
fn held_elsewhere(session: &str, pid: u32) -> String {
    let meta = fs::read(paths::runtime_dir().join(format!("{pid}.json"))).ok();
    let meta: Value = meta.and_then(|m| serde_json::from_slice(&m).ok()).unwrap_or_default();
    if meta["proxy_pid"].is_number() {
        format!(
            "brnr: session {session} is open in another editor (brnr process {pid}); close it \
             there first"
        )
    } else {
        format!(
            "brnr: session {session} is running in brnr process {pid}; release it first with \
             `brnr close {session}`"
        )
    }
}

/// A prompt's content as text: its text blocks, with other blocks shown as
/// `[image]`, `[resource <uri>]` and so on.
pub(super) fn prompt_text(blocks: Option<&Value>) -> String {
    let blocks = blocks.and_then(Value::as_array).map_or(&[][..], Vec::as_slice);
    let parts: Vec<String> = blocks
        .iter()
        .map(|b| match b["type"].as_str() {
            Some("text") => b["text"].as_str().unwrap_or_default().to_owned(),
            Some(kind) => match b["uri"].as_str().or(b["resource"]["uri"].as_str()) {
                Some(uri) => format!("[{kind} {uri}]"),
                None => format!("[{kind}]"),
            },
            None => "[?]".to_owned(),
        })
        .collect();
    parts.join("\n")
}

/// The session a `session/new` result opened, or a `session/fork` one:
/// fork answers as new does, and the schema has its own type only among
/// its unstable ones (see schema.rs).
pub(super) fn new_session(result: &Value) -> Option<String> {
    schema::read::<NewSessionResponse>(result).map(|r| r.session_id.to_string())
}

fn param_session(msg: &Map<String, Value>) -> Option<String> {
    msg.get("params")?.get("sessionId")?.as_str().map(str::to_owned)
}

/// Whether a permission option of `kind` allows the tool call, rather than
/// rejecting it.
fn allows(kind: PermissionOptionKind) -> bool {
    matches!(kind, PermissionOptionKind::AllowOnce | PermissionOptionKind::AllowAlways)
}

fn permission_event(req: &AgentRequest, owner: &str) -> Value {
    let tool = &req.params["toolCall"];
    json!({
        "event": "permission_request",
        "session": req.session,
        "request": req.handle,
        "owner": owner,
        "title": tool["title"],
        "kind": tool["kind"],
        "tool_call": tool,
        "options": req.params["options"],
    })
}
