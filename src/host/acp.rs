//! The ACP stream: sessions, turns, injection, and the host as the agent's
//! client when no editor is attached.
//!
//! The host reads ACP line by line from both sides. It changes the stream
//! in these ways; everything else passes through unchanged:
//!
//! - Every request to the agent has an id of the host's (`brnr-<n>`), the
//!   editor's too, and the answer is for whoever sent it: the editor's goes
//!   back to it with its id as it wrote it (ADR 61). In a line, only the id
//!   changes. The editor's `$/cancel_request` names its own requests, and
//!   goes with their ids on the agent's side.
//! - An injected message goes to the agent as a `session/prompt`, never
//!   while a prompt is running: until then it is held. With `--steer` it
//!   goes into the running turn instead, as `_session/steering` (ADR 18 in
//!   docs/adr). The responses are kept from the editor, and the editor is
//!   shown the text as it is sent, as a completed tool call or, if its
//!   profile says so, as `user_message_chunk`s (`echo_message`, ADR 70). On
//!   an editor's session it is the experimental `send` (ADR 4), refused
//!   while a prompt runs.
//! - Held context is appended to the next `session/prompt`, whoever sends
//!   it.
//! - What the side channel does to an editor's session (ADR 4, see
//!   experimental.rs): a permission request or an elicitation the host
//!   answered in the editor's place is withdrawn from it
//!   (`$/cancel_request`), and its late answer dropped; brnr's notes to it
//!   take the echo's form; and a session brnr closed under it has its later
//!   requests answered by the host.
//! - With no editor attached, the host answers what the agent asks of its
//!   client. Permission requests wait for an allow or reject from a bridge
//!   or the CLI, by the option's kind (ADR 63), until `response_timeout`
//!   rejects them once, or cancels their turn; how much the agent asks is
//!   the agent's mode (`--mode`, see ADR 27 in docs/adr). Elicitations
//!   (`elicitation/create`, a form or a URL for the user) wait the same
//!   way for an accept, decline or cancel, until the same timeout cancels
//!   them (ADR 68, see elicitation.rs). Anything else gets "method not
//!   found": `fs` and `terminal` too, which a headless process doesn't
//!   offer (ADR 67 in docs/adr).
//! - While a `session/load`, headless or the editor's, replays a session's
//!   history (ADR 57): if brnr has a transcript of the session, the replayed
//!   updates are neither recorded nor turned into events, as the transcript
//!   has them already, but what they say the session is now (its title,
//!   mode, config options and commands) is kept. A session open in the
//!   process has its transcript, so an editor loading it again records
//!   nothing of the replay again. If it has none, they are recorded and made
//!   into events as live ones would be, the user's messages too, each marked
//!   `replayed`. Either way a `history` event says which, and how many
//!   updates the agent replayed, and the editor gets the replay as the agent
//!   sent it. Loads of an editor's that overlap share one replay, until the
//!   last is answered, and one `history` event, with how many `loads`. A
//!   replay outlasts its session's close while a load of it is unanswered:
//!   counted, not recorded, and the session's again if the load succeeds
//!   (`session_reopened`), when `recorded_updates` says how many the
//!   transcript has.
//! - The editor's `session/load` or `session/resume` of a session another
//!   process holds is answered by the host with an error, naming the process
//!   and how to release it, and never reaches the agent (ADR 3); with
//!   `shared_sessions` in the profile (ADR 42) it goes through, and the
//!   session is served shared (see `Hold`).
//!
//! The editor's `session/delete` passes through as it is; once the agent has
//! deleted the session, the host records a `session_deleted` (ADR 63).
//!
//! The editor's own steers (`_session/steering`) pass through untouched but
//! for their ids, as every request does; once the agent has taken one into
//! the turn, it is a `user_message` by the editor.
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
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Instant, SystemTime};

use serde_json::{Map, Value, json};

use super::elicitation::{self, UrlElicitation};
use super::flow::LINE_BYTES;
use super::requests::{HostRequest, PeerOp, history};
use super::state::SessionState;
use super::{Host, LOG_FLUSH, control, id_key, text_block};
use crate::config::{EchoAs, Feature, Log};
use crate::lock::{self, Lock};
use crate::log::{self, Dir};
use crate::schema::{
    self, ContentBlock, MessageId, NewSessionResponse, PermissionOptionKind, SessionUpdate,
};
use crate::{frame, json, paths};

/// The most URL elicitations the host keeps waiting for their completion.
const URL_ELICITATIONS: usize = 1024;

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

/// A `session/load`'s replay of history, while it lasts (ADR 57).
pub(super) struct Replay {
    /// Recorded and made into events: brnr has no transcript of the session.
    pub(super) record: bool,
    /// The updates the agent has replayed.
    pub(super) updates: u64,
    /// Of those, the ones recorded: all if `record`, but those that came
    /// while the session was closed (`Host::closed_loads`).
    pub(super) recorded_updates: u64,
    /// The ids of the tool calls running before it, while their turn runs:
    /// the others, the history's, aren't running now.
    pub(super) tools: Vec<String>,
    /// The loads it covers: the editor's load of a session while its replay
    /// runs joins it, as the agent's replays can't be told apart (P4).
    pub(super) loads: u32,
    /// Of those, the ones the agent hasn't answered: it ends with the last.
    pub(super) unanswered: u32,
    /// One of them succeeded.
    pub(super) loaded: bool,
    /// The session was opened for them: if none succeeds, and no resume has
    /// opened it meanwhile, it closes again.
    pub(super) opened: bool,
}

pub(super) struct Session {
    pub(super) id: String,
    pub(super) cwd: PathBuf,
    /// The session it was forked from, when it opened here as a fork
    /// (ADR 16); not known for one opened any other way.
    pub(super) forked_from: Option<String>,
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
    pub(super) replay: Option<Replay>,
    /// Since when nothing has been running, held or waiting for an answer
    /// (see `fire_idle_timers`); `idle_done` once its time ran out.
    idle_since: Option<Instant>,
    idle_done: bool,
    /// When it last had an event, or opened.
    pub(super) last_active: SystemTime,
    /// How the last turn ended: `{stop_reason, error, tokens}`, as
    /// `turn_ended`.
    pub(super) last_turn: Option<Value>,
    /// How this process holds it (ADR 3).
    pub(super) hold: Hold,
    /// A close asked for, until the agent has closed it: meanwhile the
    /// session takes no more requests (see `close`).
    pub(super) closing: Option<Close>,
    /// Opened for a bridge's `new` or `resume`, which hasn't committed yet
    /// (see requests.rs): it takes no requests, and its idle time doesn't
    /// count.
    pub(super) opening: bool,
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

/// A session closed here, whose lock goes, and whose close is answered,
/// once the logger has written its records, `session_closed` last, or at
/// `until`, whichever comes first: a process that resumes it next appends
/// after them (ADR 48). The event loop never waits for it (P1).
pub(super) struct LettingGo {
    session: String,
    until: Instant,
    /// Taken by whichever comes first: the logger's thread or the event
    /// loop's timer. This process opening the session again takes back only
    /// its lock: the close is still answered once it is written.
    held: Arc<Mutex<Option<Release>>>,
}

/// What goes as a closed session is let go of: its lock, then the answers
/// to the close, each a line for its peer.
struct Release {
    lock: Option<Lock>,
    answers: Vec<(control::Queue, String)>,
}

impl LettingGo {
    fn take(&self) -> Option<Release> {
        self.held.lock().unwrap_or_else(PoisonError::into_inner).take()
    }

    fn done(&self) -> bool {
        self.held.lock().unwrap_or_else(PoisonError::into_inner).is_none()
    }

    /// The session's lock, if it is still this process's.
    fn reclaim(&self) -> Option<Lock> {
        self.held.lock().unwrap_or_else(PoisonError::into_inner).as_mut()?.lock.take()
    }

    fn locked(&self) -> bool {
        self.held
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .is_some_and(|r| r.lock.is_some())
    }
}

impl Release {
    /// Lets go of the lock, if the session isn't open here again, then
    /// answers, so whoever hears it finds the session free.
    fn go(self) {
        drop(self.lock);
        for (queue, line) in self.answers {
            queue.post(line);
        }
    }
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
    /// How deeply what it keeps of what the host read nests (see
    /// `Host::holds`). Every field is named, so that one added is thought
    /// about here.
    pub(super) fn holds(&self) -> usize {
        let Session {
            held,
            steering,
            ended,
            last_turn,
            state,
            closing,
            id: _,
            cwd: _,
            forked_from: _,
            prompts: _,
            interrupts: _,
            context: _,
            agent_text: _,
            agent_text_kind: _,
            agent_message_id: _,
            turn_started: _,
            replay: _,
            idle_since: _,
            idle_done: _,
            last_active: _,
            hold: _,
            opening: _,
        } = self;
        let blocks = held.iter().chain(steering).flat_map(|h| &h.blocks);
        let req_id = match closing {
            Some(Close::AfterTurn { req_id, .. }) => req_id.as_ref(),
            Some(Close::Sent) | None => None,
        };
        json::nesting(blocks.chain(ended).chain(last_turn).chain(req_id)).max(state.holds())
    }

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
    /// `session/new`, or the host's `session/fork` of `forked_from`. Its
    /// request is logged once the response says which session it belongs to.
    New {
        cwd: Option<String>,
        request: Option<Vec<u8>>,
        dir: Dir,
        forked_from: Option<String>,
    },
    /// The editor's `session/load` (`load`) or `session/resume`.
    Attach {
        session: String,
        cwd: Option<String>,
        load: bool,
    },
    Close {
        session: String,
    },
    /// The editor's `session/delete`, passed through as it is (ADR 63).
    Delete {
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

/// A request to the agent it hasn't answered: who asked, so who its answer
/// is for, and the session it is about. Its id on the agent's side is the
/// host's own, whoever asked (ADR 61).
pub(super) struct ClientRequest {
    pub(super) by: Requester,
    pub(super) session: Option<String>,
}

pub(super) enum Requester {
    /// The editor, with its id: as it wrote it (`raw`), which its answer goes
    /// back with, and as a key, for its `$/cancel_request`.
    Editor { raw: Vec<u8>, key: String },
    /// The host, as the agent's client or for a bridge.
    Host(HostRequest),
    /// A message the host sent as a prompt: its answer ends the turn.
    Prompt,
}

/// A request from the agent to its client.
pub(super) struct AgentRequest {
    pub(super) key: String,
    pub(super) id: Value,
    method: String,
    pub(super) session: Option<String>,
    pub(super) params: Value,
    /// `p<n>` for a permission request, `e<n>` for an elicitation: what
    /// bridges and brnr call it. None for a request brnr doesn't hold.
    pub(super) handle: Option<String>,
    /// When `response_timeout` answers it.
    pub(super) deadline: Option<Instant>,
}

impl AgentRequest {
    /// A `session/request_permission` (ADR 27).
    pub(super) fn is_permission(&self) -> bool {
        self.method == "session/request_permission"
    }

    /// An `elicitation/create` (ADR 68).
    pub(super) fn is_elicitation(&self) -> bool {
        self.method == "elicitation/create"
    }

    /// Its handle, if brnr holds it.
    pub(super) fn held(&self) -> Option<&str> {
        self.handle.as_deref()
    }

    /// The title brnr tells the editor by (see experimental.rs): the tool
    /// call's, or the elicitation's message.
    pub(super) fn title(&self) -> String {
        let title = if self.is_elicitation() {
            self.params["message"].as_str()
        } else {
            self.params["toolCall"]["title"].as_str()
        };
        title.unwrap_or("the request").to_owned()
    }

    /// How deeply what it keeps of the request nests (see `Host::holds`).
    pub(super) fn holds(&self) -> usize {
        let AgentRequest { id, params, key: _, method: _, session: _, handle: _, deadline: _ } =
            self;
        json::nesting([id, params])
    }
}

/// How a permission request is answered from outside (ADR 63): `allow` or
/// `reject`, once or, with `always`, always.
#[derive(Clone, Copy)]
pub(super) enum Choice {
    Allow,
    Reject,
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
                    // It goes with an id of the host's, which nobody else's
                    // can be: the editor's as it wrote it comes back on the
                    // answer (ADR 61). Only the id changes in the line.
                    let wire = self.wire_id();
                    let key = id_key(&wire);
                    let spans = json::members(&line, "id");
                    let raw = match spans.last() {
                        Some(span) => line[span.clone()].to_vec(),
                        None => serde_json::to_vec(&id).unwrap(),
                    };
                    msg.insert("id".to_owned(), wire.clone());
                    line = match self.editor_request(&method, &key, &mut msg, &session) {
                        Some(rewritten) => rewritten,
                        None if !spans.is_empty() => {
                            json::replace(&line, &spans, &serde_json::to_vec(&wire).unwrap())
                        }
                        None => serde_json::to_vec(&msg).unwrap(),
                    };
                    let by = Requester::Editor { raw, key: id_key(&id) };
                    let request = ClientRequest { by, session: session.clone() };
                    self.client_requests.insert(key.clone(), request);
                    if method == "session/new" {
                        deferred = Some(key);
                    }
                }
                (Some(method), None) if method == "$/cancel_request" => {
                    if let Some(cancels) = self.editor_cancel(&msg) {
                        for cancel in cancels {
                            let mut line = serde_json::to_vec(&cancel).unwrap();
                            line.push(b'\n');
                            self.record(session.as_deref(), Dir::EditorToAgent, &line);
                            self.write_agent(&line);
                        }
                        return;
                    }
                }
                (None, Some(id)) => {
                    let key = id_key(&id);
                    if self.agent_requests.iter().any(|r| r.key == key) {
                        session = self.agent_request_answered(&key, &msg, "editor");
                    } else if self.late_answer(&key, &id, &msg) {
                        // Answered by the host already (a cancel, an allow
                        // or reject): the agent must not get a second answer.
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
        // out (ADR 25 in docs/adr); the agent gets the line as it came, but
        // for a request's id (ADR 61) and the named changes.
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

    /// The editor's `$/cancel_request` of its request `requestId`: the
    /// cancellations to send the agent instead, each with the id that request
    /// has there (one each, if the editor gave several requests that id). An
    /// id the host gave a request on the agent's side, which the editor
    /// never had, isn't one of its own to cancel: nothing goes, and the host
    /// log says so. Any other goes as it came (`None`): the id of a request
    /// the host couldn't read (ADR 26), or of none.
    fn editor_cancel(&mut self, msg: &Map<String, Value>) -> Option<Vec<Value>> {
        let named = msg.get("params")?.get("requestId")?;
        let key = id_key(named);
        let mut wires: Vec<&String> = (self.client_requests.iter())
            .filter(|(_, r)| matches!(&r.by, Requester::Editor { key: k, .. } if *k == key))
            .map(|(wire, _)| wire)
            .collect();
        if wires.is_empty() {
            if !self.client_requests.contains_key(&key) {
                return None;
            }
            let event = json!({ "event": "editor-cancel-dropped", "request_id": named });
            self.sink.note(None, event);
            return Some(Vec::new());
        }
        wires.sort();
        let cancels = wires.into_iter().map(|wire| {
            let mut cancel = Value::Object(msg.clone());
            cancel["params"]["requestId"] = serde_json::from_str(wire).unwrap();
            cancel
        });
        Some(cancels.collect())
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
            // Its client capabilities, `fs` and `terminal` too, reach the
            // agent as the editor sent them (ADR 67 in docs/adr).
            ("initialize", _) => {
                self.pending.insert(key.to_owned(), Pending::Initialize);
            }
            ("session/new", _) => {
                let pending =
                    Pending::New { cwd, request: None, dir: Dir::EditorToAgent, forked_from: None };
                self.pending.insert(key.to_owned(), pending);
            }
            ("session/load" | "session/resume", Some(session)) => {
                let load = method == "session/load";
                if load {
                    self.editor_load(session, cwd.as_deref());
                }
                let attach = Pending::Attach { session: session.clone(), cwd, load };
                self.pending.insert(key.to_owned(), attach);
            }
            ("session/close", Some(session)) => {
                // Dropped now: held until the agent answers, they would go out as
                // the running turn ends.
                if let Some(i) = self.find(session) {
                    self.drop_all(i, "close");
                }
                self.pending.insert(key.to_owned(), Pending::Close { session: session.clone() });
            }
            ("session/delete", Some(session)) => {
                self.pending.insert(key.to_owned(), Pending::Delete { session: session.clone() });
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

    /// The editor's `session/load` of `session`, whose history the agent
    /// replays before it answers: recorded only if brnr has no transcript of
    /// the session, in any folder, as a headless start's and a bridge's are
    /// (ADR 57): an events file holding only openings, which a load that
    /// failed leaves, is none ([`log::has_records`]). One not open here
    /// opens now, so that the replay is the session's; one open here has its
    /// transcript already, from this process, whether or not the logger has
    /// written it yet, unless the process logs nothing; one this process
    /// served before and has closed since has one if the process recorded
    /// anything of it beyond its openings, a failed load's replay too, as
    /// the disk will once the logger has written it; one another process
    /// holds has that one's. A load while the session's replay runs joins
    /// that replay, as which load an update is for can't be told (P4): what
    /// either replays is taken as the replay is, recorded or not, and one
    /// `history` event covers them all.
    fn editor_load(&mut self, session: &str, cwd: Option<&str>) {
        // One closed while a load of it is unanswered opens with its replay.
        let open = match self.find(session) {
            None if self.closed_loads.contains_key(session) => {
                Some(self.open_session(session, cwd))
            }
            open => open,
        };
        if let Some(replay) = open.and_then(|i| self.sessions[i].replay.as_mut()) {
            replay.loads += 1;
            replay.unanswered += 1;
            return;
        }
        let record = match open.map(|i| &self.sessions[i].hold) {
            Some(Hold::Owner(_) | Hold::Unlocked(_)) => self.logging == Log::Off,
            Some(Hold::Shared(_)) | None => !self.has_transcript(session),
        };
        let i = self.open_session(session, cwd);
        self.begin_replay(i, record);
        if let Some(replay) = &mut self.sessions[i].replay {
            replay.opened = open.is_none();
        }
    }

    /// The agent's answer to one of the editor's loads of `session`, a
    /// failure too. The replay ends with the last of the loads it covers:
    /// with its `history` event if any of them succeeded; if none did, a
    /// session opened for them closes again (ADR 57).
    fn load_answered(&mut self, session: &str, cwd: Option<&str>, result: Option<&Value>) {
        let i = match (self.find(session), result) {
            (Some(i), _) => i,
            // Closed meanwhile, as the editor asked, and the agent answered
            // the close before the load: its last answer wins, and says so.
            // The replay goes on, its own again (see `open_session`).
            (None, Some(_)) => {
                let i = self.open_session(session, cwd);
                self.emit(
                    json!({ "event": "session_reopened", "session": session, "by": "editor" }),
                );
                i
            }
            (None, None) => return self.closed_load_failed(session),
        };
        if let Some(result) = result {
            self.sessions[i].state.result(result);
        }
        let Some(replay) = &mut self.sessions[i].replay else { return };
        replay.unanswered = replay.unanswered.saturating_sub(1);
        replay.loaded |= result.is_some();
        if replay.unanswered > 0 {
            return;
        }
        let (loaded, opened) = (replay.loaded, replay.opened);
        // The last replayed message, while it is still marked as replayed,
        // whether the load succeeded or not (P3).
        self.flush_agent_message(i);
        if loaded {
            self.end_replay(i);
        } else if opened {
            // Its lock goes with it, and its files are closed.
            self.sink.close_session(&self.sessions.remove(i).id);
        } else {
            self.drop_replay(i); // Loaded again, or resumed, and failed.
        }
    }

    /// A load of `session`, closed meanwhile, failed. Its replay goes with
    /// the last of its loads; if one of them succeeded before the close,
    /// its `history` says so, with the session closed (ADR 57).
    fn closed_load_failed(&mut self, session: &str) {
        let Some(replay) = self.closed_loads.get_mut(session) else { return };
        replay.unanswered = replay.unanswered.saturating_sub(1);
        if replay.unanswered > 0 {
            return;
        }
        if let Some(replay) = self.closed_loads.remove(session)
            && replay.loaded
        {
            self.emit(history(session, &replay));
        }
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
        self.write_editor(None, &msg);
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
        self.echo_message(session, "Context via brnr", &attached);
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
                let mut session = param_session(&msg);
                let mut completed = None;
                if method.as_deref() == Some("elicitation/complete") {
                    let event = self.elicitation_complete(&msg);
                    session = event.as_ref().and_then(|e| e["session"].as_str()).map(str::to_owned);
                    completed = event;
                }
                let update = method.as_deref() == Some("session/update");
                let i = session.as_deref().and_then(|s| self.find(s));
                if let Some(i) = i
                    && let Some(replay) = &mut self.sessions[i].replay
                {
                    replay.updates += u64::from(update);
                    if !replay.record {
                        // History the transcript has already. An editor
                        // still gets it, as the agent sent it (ADR 2).
                        if update {
                            self.replayed_state(i, &msg["params"]["update"]);
                        }
                        return self.send_link(frame::DATA, line);
                    }
                    replay.recorded_updates += u64::from(update);
                } else if let Some(replay) =
                    session.as_deref().and_then(|s| self.closed_loads.get_mut(s))
                {
                    // Its session closed while it was loading: counted, with
                    // no files to record it in (ADR 57).
                    replay.updates += u64::from(update);
                }
                if update && let Some(session) = &session {
                    self.track_update(session, &msg["params"]["update"]);
                }
                self.record(session.as_deref(), Dir::AgentToEditor, line);
                self.send_link(frame::DATA, line);
                if let Some(event) = completed {
                    self.emit(event);
                }
            }
        }
    }

    fn agent_response(&mut self, key: &str, msg: &Map<String, Value>, line: &[u8]) {
        let request = self.client_requests.remove(key);
        let mut session = request.as_ref().and_then(|r| r.session.clone());
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
                    // Its tool calls end with it, however it ended: one the
                    // agent left unfinished, cancelled or failed, isn't
                    // running now (ADR 34). Nor are those a replay found.
                    s.state.tools.clear();
                    if let Some(replay) = &mut s.replay {
                        replay.tools.clear();
                    }
                }
            }
            let injected = prompt.as_ref().is_some_and(|p| p.injected);
            let messages = prompt.map(|p| p.messages).unwrap_or_default();
            session = Some(sid.clone());
            turn = Some((sid, injected, messages));
        }
        // Whose answer it is (ADR 61): the editor's goes to it with its id as
        // it wrote it, and nothing else in the line changed. One to an id the
        // host never sent (a request in a line it couldn't read, ADR 26) goes
        // on as it came.
        let (ours, forward) = match request.map(|r| r.by) {
            Some(Requester::Editor { raw, .. }) => {
                (None, Some(json::replace(line, &json::members(line, "id"), &raw)))
            }
            Some(Requester::Host(request)) => (Some(request), None),
            Some(Requester::Prompt) => (None, None),
            None => (None, Some(line.to_vec())),
        };
        let dir = if forward.is_some() { Dir::AgentToEditor } else { Dir::AgentToControl };
        self.record(session.as_deref(), dir, line);
        if let Some(line) = forward {
            self.send_link(frame::DATA, &line);
        }
        if let Some(request) = ours {
            self.host_request_done(request, msg);
        }
        if let Some((sid, injected, messages)) = turn {
            let stop_reason = msg.get("result").and_then(|r| r.get("stopReason"));
            let tokens = turn_tokens(msg.get("result"));
            if let Some(i) = self.find(&sid) {
                self.flush_agent_message(i);
                self.sessions[i].last_turn = Some(json!({
                    "stop_reason": stop_reason,
                    "error": msg.get("error"),
                    "tokens": tokens,
                }));
            }
            let event = json!({
                "event": "turn_ended",
                "session": sid,
                "by": if injected { "control" } else { "editor" },
                "prompt": key,
                "messages": messages,
                "stop_reason": stop_reason,
                "error": msg.get("error"),
                "tokens": tokens,
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
            Pending::New { cwd, request, dir, forked_from } => {
                let session = result.and_then(new_session);
                if let Some(session) = &session {
                    let i = self.open_fork(session, cwd.as_deref(), forked_from.as_deref());
                    self.sessions[i].state.result(result.unwrap_or(&Value::Null));
                }
                if let Some(request) = request {
                    // A copy, in the session's file, of what the host log has with its
                    // position: none here (ADR 69).
                    self.sink.msg(session.as_deref(), dir, &request, None);
                }
                session
            }
            Pending::Attach { session, cwd, load } => {
                if load {
                    self.load_answered(&session, cwd.as_deref(), result);
                } else if let Some(result) = result {
                    let i = self.open_session(&session, cwd.as_deref());
                    self.sessions[i].state.result(result);
                    // Open now, whatever a load still to be answered gets.
                    if let Some(replay) = &mut self.sessions[i].replay {
                        replay.opened = false;
                    }
                } else {
                    self.claimed.remove(&session); // Its lock goes (see `attach`).
                }
                Some(session)
            }
            Pending::Close { session } => {
                if result.is_some()
                    && let Some(i) = self.find(&session)
                {
                    self.close_session(i, "editor", None);
                }
                Some(session)
            }
            Pending::Delete { session } => {
                if result.is_some() {
                    self.deleted(&session);
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
        if req.is_permission() {
            self.next_permission += 1;
            req.handle = Some(format!("p{}", self.next_permission));
        } else if req.is_elicitation() {
            self.next_elicitation += 1;
            req.handle = Some(format!("e{}", self.next_elicitation));
            self.remember_url(&req);
        }
        if !self.editor_attached() {
            return self.answer_as_client(req);
        }
        self.send_link(frame::DATA, line);
        let event = request_event(&req, "editor");
        self.agent_requests.push(req);
        if let Some(event) = event {
            self.emit(event);
        }
    }

    /// Keeps a URL elicitation's id, so the agent's `elicitation/complete`
    /// for it can be told to whom it belonged (ADR 68). Up to
    /// [`URL_ELICITATIONS`]: past that, an agent that never completes them
    /// would have the host keep them without limit, and the completion of
    /// one not kept is told without its handle.
    fn remember_url(&mut self, req: &AgentRequest) {
        if self.url_elicitations.len() >= URL_ELICITATIONS {
            return;
        }
        if let (Some(id), Some(handle)) = (elicitation::elicitation_id(req), req.held()) {
            let held = UrlElicitation { session: req.session.clone(), handle: handle.to_owned() };
            self.url_elicitations.insert(id, held);
        }
    }

    /// The agent's `elicitation/complete`: a URL's flow is done (ADR 68),
    /// as `elicitation_completed`. It names the elicitation, not a session;
    /// the session is the request's, if brnr kept it.
    fn elicitation_complete(&mut self, msg: &Map<String, Value>) -> Option<Value> {
        let id = msg.get("params")?.get("elicitationId")?.as_str()?;
        let (session, handle) = match self.url_elicitations.remove(id) {
            Some(held) => (held.session, Some(held.handle)),
            None => (None, None),
        };
        Some(json!({
            "event": "elicitation_completed",
            "session": session,
            "request": handle,
            "elicitation_id": id,
        }))
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
        let result = msg.get("result").cloned().unwrap_or(Value::Null);
        if req.is_permission() {
            self.emit(resolved_event(&req, &result["outcome"], None, by));
        } else if req.is_elicitation() {
            if result["action"] != "accept"
                && let Some(id) = elicitation::elicitation_id(&req)
            {
                self.url_elicitations.remove(&id);
            }
            self.emit(elicitation::resolved_event(&req, &result, by));
        }
        req.session
    }

    // ---- the host as the agent's client --------------------------------

    fn answer_as_client(&mut self, mut req: AgentRequest) {
        match req.method.as_str() {
            "session/request_permission" => {
                // None, never, for a timeout too far off to say.
                req.deadline = self.response_timeout.and_then(|t| Instant::now().checked_add(t));
                let event = permission_event(&req, "headless");
                self.agent_requests.push(req);
                self.emit(event);
            }
            // Held as a permission request is, until someone answers it
            // or the same timeout cancels it (ADR 68).
            "elicitation/create" => {
                req.deadline = self.response_timeout.and_then(|t| Instant::now().checked_add(t));
                let event = elicitation::request_event(&req, "headless");
                self.agent_requests.push(req);
                self.emit(event);
            }
            method => {
                let message = format!("{method} is not supported without an editor");
                self.respond(&req, json!({ "error": { "code": -32601, "message": message } }));
            }
        }
    }

    /// Answers permission request `handle` with the option of one kind
    /// (ADR 63): `allow_once`, `allow_always` with `always`, `reject_once`
    /// or `reject_always`. No other kind stands in for it, and a reject is
    /// never `cancelled`: without that kind, or with two options of it, it
    /// fails, listing the options (P4). `option` names the option instead:
    /// one of a kind ACP has must be on the choice's side, and with `always`
    /// the always kind; one of a kind brnr doesn't know goes with either. On
    /// an editor's session, where this is the experimental `permission` (see
    /// experimental.rs), the request is then withdrawn from the editor.
    pub(super) fn resolve_permission(
        &mut self,
        handle: &str,
        choice: Choice,
        always: bool,
        option: Option<&str>,
        by: &str,
    ) -> Result<Value, String> {
        let pos = self
            .agent_requests
            .iter()
            .position(|r| r.is_permission() && r.held() == Some(handle))
            .ok_or_else(|| format!("no pending request {handle}"))?;
        if self.status.is_some() || self.agent_in.is_none() {
            return Err("the agent is no longer accepting input".into());
        }
        let options =
            self.agent_requests[pos].params["options"].as_array().cloned().unwrap_or_default();
        let allow = matches!(choice, Choice::Allow);
        let wanted = kind_of(choice, always);
        let listed = || {
            let all: Vec<String> = options.iter().map(option_text).collect();
            format!("options: {}", all.join(", "))
        };
        let chosen = match option {
            Some(option) => {
                let Some(chosen) = options.iter().find(|o| o["optionId"] == option) else {
                    return Err(format!("{handle} has no option {option} ({})", listed()));
                };
                let kind = chosen["kind"].as_str().unwrap_or_default();
                match known_kind(chosen) {
                    Some(k) if allows(k) != allow => {
                        let verb = if allow { "reject" } else { "allow" };
                        return Err(format!(
                            "{handle}: {option} ({kind}) is for brnr permission {verb}"
                        ));
                    }
                    Some(_) if always && kind != wanted => {
                        return Err(format!(
                            "{handle}: {option} is {kind}, not {wanted} as --always asks"
                        ));
                    }
                    _ => chosen,
                }
            }
            None => {
                let of_kind: Vec<&Value> = options.iter().filter(|o| o["kind"] == wanted).collect();
                match of_kind[..] {
                    [chosen] => chosen,
                    [] => {
                        return Err(format!(
                            "{handle} has no {wanted} option ({}); --option <id> picks one",
                            listed()
                        ));
                    }
                    _ => {
                        return Err(format!(
                            "{handle} has {} {wanted} options ({}); --option <id> picks one",
                            of_kind.len(),
                            listed()
                        ));
                    }
                }
            }
        };
        let outcome = json!({ "outcome": "selected", "optionId": chosen["optionId"] });
        let req = self.agent_requests.remove(pos);
        self.respond(&req, json!({ "result": { "outcome": outcome } }));
        let how = if allow { "allowed" } else { "rejected" };
        self.withdraw(&req, how, by);
        self.emit(resolved_event(&req, &outcome, Some(how), by));
        Ok(outcome)
    }

    /// Answers elicitation `handle` with `action`, ACP's `accept` (its
    /// `content` checked against the form's schema first, see
    /// elicitation.rs), `decline` or `cancel`, by `by` (ADR 68). Nothing is
    /// sent for content that doesn't fit (P4). On an editor's session,
    /// where this is the experimental `elicitation` (see experimental.rs),
    /// the request is then withdrawn from the editor.
    pub(super) fn resolve_elicitation(
        &mut self,
        handle: &str,
        action: &str,
        content: &Value,
        by: &str,
    ) -> Result<Value, String> {
        let pos = self
            .agent_requests
            .iter()
            .position(|r| r.is_elicitation() && r.held() == Some(handle))
            .ok_or_else(|| format!("no pending elicitation {handle}"))?;
        if self.status.is_some() || self.agent_in.is_none() {
            return Err("the agent is no longer accepting input".into());
        }
        let result = elicitation::response(&self.agent_requests[pos], action, content)?;
        let req = self.agent_requests.remove(pos);
        if action != "accept"
            && let Some(id) = elicitation::elicitation_id(&req)
        {
            self.url_elicitations.remove(&id);
        }
        self.respond(&req, json!({ "result": result }));
        let how = match action {
            "accept" => "accepted",
            "decline" => "declined",
            _ => "cancelled",
        };
        self.withdraw(&req, how, by);
        self.emit(elicitation::resolved_event(&req, &result, by));
        Ok(result)
    }

    /// ACP: a client that cancels a turn answers that session's pending
    /// permission requests with `cancelled`, `by` `cancel` or `timeout`. Its
    /// pending elicitations are answered `cancel` the same way (ADR 68):
    /// the turn they were asked in is gone. If the editor is showing one,
    /// it is withdrawn, and its late answer dropped (see experimental.rs).
    fn cancel_requests(&mut self, session: &str, by: &str) {
        let (cancel, keep): (Vec<_>, Vec<_>) = take(&mut self.agent_requests)
            .into_iter()
            .partition(|r| r.handle.is_some() && r.session.as_deref() == Some(session));
        self.agent_requests = keep;
        for req in cancel {
            if req.is_elicitation() {
                let result = json!({ "action": "cancel" });
                if let Some(id) = elicitation::elicitation_id(&req) {
                    self.url_elicitations.remove(&id);
                }
                self.respond(&req, json!({ "result": result }));
                self.withdraw(&req, "cancelled", by);
                self.emit(elicitation::resolved_event(&req, &result, by));
                continue;
            }
            let outcome = json!({ "outcome": "cancelled" });
            self.respond(&req, json!({ "result": { "outcome": outcome } }));
            self.withdraw(&req, "cancelled", by);
            self.emit(resolved_event(&req, &outcome, Some("cancelled"), by));
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
            // Live, a user's message is the prompt's; replayed, only the
            // agent has it (ADR 57).
            SessionUpdate::UserMessageChunk(chunk) if self.sessions[i].replay.is_some() => {
                ("user_message", chunk)
            }
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

    /// Closes session `i`, `by` `close` (`brnr session close`, `--take-over`)
    /// or `idle`: a running turn is cancelled first, its pending approvals
    /// answered `cancelled`, and `session/close` goes once it has ended
    /// (ADR 16). `peer` hears when the agent has closed it (peer 0: nobody).
    pub(super) fn close(&mut self, i: usize, peer: u64, req_id: Option<Value>, by: &'static str) {
        // Dropped now: held until the agent answers, they would go out as the
        // running turn ends.
        self.drop_all(i, "close");
        if !self.is_idle(i) {
            let session = self.sessions[i].id.clone();
            self.cancel(&session, "cancel");
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
        self.peer_op(
            peer,
            req_id,
            PeerOp::Close { session, by, failed: None },
            "session/close",
            params,
        );
    }

    /// The agent has closed session `i`, `by` `close`, `idle` or `editor`:
    /// what it still holds is dropped, then `session_closed` (ADR 20). Its
    /// lock goes, and the peer in `answer` is answered with its reply, once
    /// that is written, LOG_FLUSH at most (see `LettingGo`).
    pub(super) fn close_session(&mut self, i: usize, by: &str, answer: Option<(u64, Value)>) {
        self.flush_agent_message(i);
        self.drop_all(i, "close");
        let Session { id: session, hold, replay, .. } = self.sessions.remove(i);
        self.emit(json!({ "event": "session_closed", "session": session, "by": by }));
        // A load of it still to be answered may open it again (ADR 57).
        if let Some(replay) = replay.filter(|r| r.unanswered > 0 && self.editor_attached()) {
            self.closed_loads.insert(session.clone(), replay);
        }
        // After its last record, so nothing of it is lost (ADR 22).
        self.sink.close_session(&session);
        self.let_go(session, hold, answer);
    }

    /// Lets go of `session`, gone from `sessions`, and answers the peer in
    /// `answer` with its reply, once the logger has written what this
    /// process recorded of it, LOG_FLUSH at most (see `LettingGo`): a close's
    /// records, or a failed open's (a load's replay, ADR 57), come before
    /// the next process's (ADR 48).
    pub(super) fn let_go(&mut self, session: String, hold: Hold, answer: Option<(u64, Value)>) {
        let lock = match hold {
            Hold::Owner(lock) => lock,
            Hold::Shared(_) | Hold::Unlocked(_) => None,
        };
        let answers =
            answer.and_then(|(peer, reply)| Some((self.peer_queue(peer)?, reply.to_string())));
        let release = Release { lock, answers: answers.into_iter().collect() };
        let held = Arc::new(Mutex::new(Some(release)));
        let written = held.clone();
        self.sink.when_written(move |_| {
            // Taken out first, so the lock isn't held while it goes.
            let release = written.lock().unwrap_or_else(PoisonError::into_inner).take();
            if let Some(release) = release {
                release.go();
            }
        });
        let until = Instant::now() + LOG_FLUSH;
        self.letting_go.push(LettingGo { session, until, held });
    }

    /// Closed sessions whose time to be written is up are let go of as they
    /// are; past that, records of theirs can come after a resume's (ADR 48).
    pub(super) fn fire_letting_go(&mut self, now: Instant) {
        self.letting_go.retain(|l| {
            if now < l.until {
                return !l.done();
            }
            if let Some(release) = l.take() {
                release.go();
            }
            false
        });
    }

    /// When the next closed session is let go of, for the event loop.
    pub(super) fn next_letting_go(&self) -> Option<Instant> {
        self.letting_go.iter().map(|l| l.until).min()
    }

    /// Exiting, once the logger has had its time: every closed session is
    /// let go of, and its close answered.
    pub(super) fn let_go_of_closed(&mut self) {
        for release in take(&mut self.letting_go).iter().filter_map(LettingGo::take) {
            release.go();
        }
    }

    /// `session`'s lock, if it closed here and isn't let go of yet: opened
    /// here again, it is this process's still, and its records come after
    /// the close's from the same logger. The close is answered as before,
    /// once its records are written (ADR 48).
    fn reclaim(&mut self, session: &str) -> Option<Lock> {
        self.letting_go.iter().filter(|l| l.session == session).find_map(LettingGo::reclaim)
    }

    /// Whether `session` closed here and its lock is still this process's.
    fn letting_go_of(&self, session: &str) -> bool {
        self.letting_go.iter().any(|l| l.session == session && l.locked())
    }

    /// The sessions closed here whose locks are still this process's, until
    /// their records are written: `status` says so, so that brnr can tell
    /// one closing from one opening (ADR 48).
    pub(super) fn closing(&self) -> Vec<&str> {
        self.letting_go.iter().filter(|l| l.locked()).map(|l| l.session.as_str()).collect()
    }

    /// The agent has deleted `session`, as the editor asked: a
    /// `session_deleted`, and brnr's transcript stays (ADR 63). One open here
    /// is gone from the agent, so it closes, as if the editor had closed it.
    /// One no process has open has the event appended to its transcripts
    /// too; one another process holds has it in the host log only, as that
    /// process writes its transcript.
    fn deleted(&mut self, session: &str) {
        let event = json!({ "event": "session_deleted", "session": session, "by": "editor" });
        let event = self.emit(event);
        match self.find(session) {
            Some(i) => self.close_session(i, "editor", None),
            // Closed here, and not let go of yet: still this process's to write.
            None if self.letting_go_of(session) || lock::holder(session).is_none() => {
                self.sink.append(session, event);
            }
            None => {}
        }
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
    /// process stops with its last session, one a bridge is opening
    /// counting (`opening_in_flight`). Idle time counts from the
    /// commit too, so a session started without a prompt doesn't run
    /// forever; one with a prompt is busy from then.
    pub(super) fn fire_idle_timers(&mut self, now: Instant) {
        let Some(limit) = self.stop_when_idle else { return };
        if self.editor_attached() || !self.start_done || self.stop_requested {
            return;
        }
        for i in 0..self.sessions.len() {
            let idle = self.is_idle(i) && !self.sessions[i].opening;
            let s = &mut self.sessions[i];
            if !idle {
                (s.idle_since, s.idle_done) = (None, false);
            } else if s.idle_since.is_none() {
                s.idle_since = Some(now);
            }
        }
        let Some(i) = (0..self.sessions.len()).find(|&i| {
            let until = self.sessions[i].idle_since.and_then(|t| t.checked_add(limit)); // None: never.
            self.idle_counts(i) && until.is_some_and(|t| now >= t)
        }) else {
            return;
        };
        self.sessions[i].idle_done = true;
        let session = self.sessions[i].id.clone();
        self.sink.note(Some(&session), json!({ "event": "idle-timeout" }));
        // A session a bridge is opening keeps the process going.
        if self.sessions.len() == 1 && !self.opening_in_flight() {
            self.begin_stop();
        } else if self.caps.close {
            // As `brnr session close` would, with nobody to answer (peer 0).
            self.close(i, 0, None, "idle");
        }
    }

    /// Whether session `i`'s idle time can still run out, for
    /// `fire_idle_timers` and `next_idle_deadline` alike: it is idle, its
    /// time hasn't run out already, and it is neither being opened nor
    /// closing (a close waits for the agent's answer, whatever its time).
    fn idle_counts(&self, i: usize) -> bool {
        let s = &self.sessions[i];
        !s.idle_done && !s.opening && s.closing.is_none() && self.is_idle(i)
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
            .filter(|&i| self.idle_counts(i))
            .filter_map(|i| match self.sessions[i].idle_since {
                None => Some(Instant::now()),
                Some(t) => t.checked_add(limit), // None: never.
            })
            .min()
    }

    /// A new id for a request to the agent, `brnr-<n>`: the host gives every
    /// request its own, the editor's too, so no two can be confused (ADR 61).
    pub(super) fn wire_id(&mut self) -> Value {
        self.next_id += 1;
        Value::String(format!("brnr-{}", self.next_id))
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
        let id = self.wire_id();
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
        let request = ClientRequest { by: Requester::Prompt, session: Some(session.clone()) };
        self.client_requests.insert(key.clone(), request);
        let text = prompt_text(Some(&json!(blocks)));
        self.echo_message(&session, "Message via brnr", &blocks);
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
        self.echo_message(&session, "Message via brnr", &blocks);
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

    /// Answers permission requests nobody answered within
    /// `response_timeout` with their `reject_once` option (ADR 63). One
    /// without one (or with two: which, brnr can't say) has its turn
    /// cancelled, which answers it and every request pending in its session
    /// `cancelled`, as ACP requires. Never `reject_always`, which nobody
    /// chose, and never an allow. An elicitation nobody answered is
    /// answered `cancel`, which ACP has for no choice made (ADR 68): never
    /// `decline`, which is the user's no, nor `accept`.
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
            // Gone already: answered `cancelled` with an earlier one's turn.
            let Some(req) =
                self.agent_requests.iter_mut().find(|r| r.handle.as_ref() == Some(&handle))
            else {
                continue;
            };
            req.deadline = None;
            let elicitation = req.is_elicitation();
            let options = req.params["options"].as_array().into_iter().flatten();
            let once = options.filter(|o| o["kind"] == "reject_once").count();
            let session = req.session.clone();
            let result = if elicitation {
                self.resolve_elicitation(&handle, "cancel", &Value::Null, "timeout")
            } else if once == 1 {
                self.resolve_permission(&handle, Choice::Reject, false, None, "timeout")
            } else if self.status.is_some() || self.agent_in.is_none() {
                Err("the agent is no longer accepting input".to_owned())
            } else if let Some(session) = session {
                let event = json!({ "event": "cancel", "by": "timeout", "request": handle });
                self.sink.note(Some(&session), event);
                self.cancel(&session, "timeout");
                Ok(Value::Null)
            } else {
                Err("no reject_once option, and no session to cancel".to_owned())
            };
            if let Err(err) = result {
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

    /// Cancels `session`'s turn (`session/cancel`), answering its pending
    /// permission requests `cancelled` and its elicitations `cancel`, `by`
    /// `cancel` or `timeout`.
    pub(super) fn cancel(&mut self, session: &str, by: &str) {
        let msg = json!({
            "jsonrpc": "2.0",
            "method": "session/cancel",
            "params": { "sessionId": session },
        });
        let mut line = serde_json::to_vec(&msg).unwrap();
        line.push(b'\n');
        self.record(Some(session), Dir::ControlToAgent, &line);
        self.write_agent(&line);
        self.cancel_requests(session, by);
    }

    /// Shows the editor an injected message, titled `title`, in the form its
    /// profile's `echo_as` names (ADR 70): by default a completed tool call
    /// (`echo`), or one `user_message_chunk` per block, untitled, for an
    /// editor that renders those out of turn.
    pub(super) fn echo_message(&mut self, session: &str, title: &str, blocks: &[Value]) {
        if self.echo_as == EchoAs::ToolCall {
            return self.echo(session, title, blocks);
        }
        if let Some(i) = self.find(session) {
            self.flush_agent_message(i);
        }
        for block in blocks {
            let update = json!({ "sessionUpdate": "user_message_chunk", "content": block });
            self.update_editor(session, update);
        }
    }

    /// Shows the editor brnr's text as a completed tool call: the one update
    /// an editor renders as a block of its own wherever the turn is. A
    /// `user_message_chunk` out of turn is not rendered by every editor (see
    /// ADR 5 in docs/adr). brnr's notes to the editor take this form whatever
    /// `echo_as` says, as they are brnr's words, not the user's (see
    /// experimental.rs).
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
        if self.editor_attached() {
            self.write_editor(session, msg);
        }
    }

    /// A line of brnr's own on the editor's stdout, which is the agent's
    /// (ADR 2): none once the agent's stdout has ended, after which the
    /// editor has seen it end (ADR 62). The host log says what wasn't sent
    /// (P3).
    fn write_editor(&mut self, session: Option<&str>, msg: &Value) {
        if !self.stdout_open {
            let event = json!({
                "event": "not-sent-to-editor",
                "reason": "the agent's stdout has ended",
                "session_id": session,
                "method": msg.get("method"),
                "id": msg.get("id"),
            });
            self.sink.note(None, event);
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
        self.open_fork(session, cwd, None)
    }

    /// As [`Self::open_session`], for a session forked from `forked_from`,
    /// which its transcript's `session-opened` and its status name (ADR 16).
    pub(super) fn open_fork(
        &mut self,
        session: &str,
        cwd: Option<&str>,
        forked_from: Option<&str>,
    ) -> usize {
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
            Hold::Owner(_) => self.sink.open_session(session, &cwd, forked_from),
            // One the headless process won't serve gets no transcript.
            Hold::Unlocked(_) if self.editor_attached() => {
                self.sink.open_session(session, &cwd, forked_from)
            }
            Hold::Unlocked(_) => {}
        }
        self.sessions.push(Session {
            id: session.to_owned(),
            cwd,
            forked_from: forked_from.map(str::to_owned),
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
            // One a load still to be answered had, closed under it (ADR 57).
            replay: self.closed_loads.remove(session),
            idle_since: None,
            idle_done: false,
            last_active: SystemTime::now(),
            last_turn: None,
            hold,
            closing: None,
            opening: false,
        });
        self.sessions.len() - 1
    }

    /// Whether brnr has a transcript of `session`, in any folder, counting
    /// what this process has recorded of it that the disk may not have yet;
    /// an events file holding only openings, which a load that failed
    /// leaves, is none (ADR 57).
    pub(super) fn has_transcript(&self, session: &str) -> bool {
        self.sink.recorded(session) || log::transcripts(session).iter().any(|p| log::has_records(p))
    }

    /// `session`'s lock, for a session this process is to serve. One another
    /// process holds is shared, as `attach` lets it be; one that can't be
    /// locked is `Unlocked`, and the host log says why. Whoever opened it
    /// decides what becomes of either (ADR 3, ADR 50).
    fn take_lock(&mut self, session: &str) -> Hold {
        if let Some(lock) = self.reclaim(session) {
            return Hold::Owner(Some(lock));
        }
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
        let lock = self.reclaim(session).map_or_else(|| lock::take(session), Ok);
        let lock = lock.map_err(|err| match err {
            lock::Error::Held(pid) => format!("{session} is running in process {pid}"),
            lock::Error::Io(why) => format!("{session} can't be locked: {why}"),
        })?;
        self.claimed.insert(session.to_owned(), Hold::Owner(Some(lock)));
        Ok(())
    }
}

/// What the editor is told of a session process `pid` holds: how to
/// release it. One started with another runtime directory is reached
/// through that (ADR 65), where its metadata is, read only if the
/// directory is private, as this one's must be; the directory is said
/// whether an editor holds the session or not.
fn held_elsewhere(session: &str, pid: u32) -> String {
    let elsewhere = lock::holder_dir(session);
    let dir = elsewhere.clone().unwrap_or_else(paths::runtime_dir);
    let meta = paths::check_private(&dir)
        .ok()
        .and_then(|()| fs::read(dir.join(format!("{pid}.json"))).ok());
    let meta: Value = meta.and_then(|m| serde_json::from_slice(&m).ok()).unwrap_or_default();
    if meta["proxy_pid"].is_number() {
        let there = elsewhere.map_or_else(String::new, |d| {
            format!(
                ", with runtime directory {} ({} reaches it)",
                d.display(),
                paths::brnr_dir_is(&d)
            )
        });
        format!(
            "brnr: session {session} is open in another editor (brnr process {pid}){there}; close \
             it there first"
        )
    } else {
        let through = elsewhere.map(|d| format!("{} ", paths::brnr_dir_is(&d))).unwrap_or_default();
        format!(
            "brnr: session {session} is running in brnr process {pid}; release it first with \
             `{through}brnr session close {session}`"
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

/// The tokens a `session/prompt` answer says its turn used (ADR 71): its
/// `usage`, unstable ACP and so read as a `Value` (ADR 43), as `{total,
/// input, output, thought, cached_read, cached_write}`. Each is the agent's
/// count, or null where it gave none that is a count (a `u64`); all of it is
/// null with no `usage` object. Nothing is worked out from the rest (P4).
fn turn_tokens(result: Option<&Value>) -> Value {
    let Some(usage) = result.and_then(|r| r.get("usage")).and_then(Value::as_object) else {
        return Value::Null;
    };
    let count = |key: &str| usage.get(key).and_then(Value::as_u64);
    json!({
        "total": count("totalTokens"),
        "input": count("inputTokens"),
        "output": count("outputTokens"),
        "thought": count("thoughtTokens"),
        "cached_read": count("cachedReadTokens"),
        "cached_write": count("cachedWriteTokens"),
    })
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

/// A permission option's kind, if ACP's schema has it (v2 allows `_…` kinds
/// of an agent's own).
fn known_kind(option: &Value) -> Option<PermissionOptionKind> {
    schema::read::<PermissionOptionKind>(&option["kind"])
}

/// The kind `choice` answers with, once or `always`.
fn kind_of(choice: Choice, always: bool) -> &'static str {
    match (choice, always) {
        (Choice::Allow, false) => "allow_once",
        (Choice::Allow, true) => "allow_always",
        (Choice::Reject, false) => "reject_once",
        (Choice::Reject, true) => "reject_always",
    }
}

/// An option as errors list it: `allow (allow_once)`.
fn option_text(option: &Value) -> String {
    let id = option["optionId"].as_str().unwrap_or("?");
    format!("{id} ({})", option["kind"].as_str().unwrap_or("?"))
}

/// `permission_resolved` for `req`, answered with `outcome` (ACP's) by
/// `by`: `answer` says `allowed`, `rejected` or `cancelled`, which brnr
/// knows when it answered, and `option_kind` the chosen option's kind. An
/// editor's answer is told by the kind, so an option of a kind brnr doesn't
/// know has no `answer` (null).
fn resolved_event(req: &AgentRequest, outcome: &Value, how: Option<&str>, by: &str) -> Value {
    let chosen = req.params["options"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|o| outcome["outcome"] == "selected" && o["optionId"] == outcome["optionId"]);
    let option_kind = chosen.map_or(Value::Null, |o| o["kind"].clone());
    let answer = how.map(str::to_owned).or_else(|| match chosen.and_then(known_kind) {
        _ if outcome["outcome"] == "cancelled" => Some("cancelled".to_owned()),
        Some(k) if allows(k) => Some("allowed".to_owned()),
        Some(_) => Some("rejected".to_owned()),
        None => None,
    });
    json!({
        "event": "permission_resolved",
        "session": req.session,
        "request": req.handle,
        "answer": answer,
        "option_kind": option_kind,
        "outcome": outcome,
        "by": by,
    })
}

/// The event for a request brnr holds, `permission_request` or
/// `elicitation_request`; none for one it doesn't.
fn request_event(req: &AgentRequest, owner: &str) -> Option<Value> {
    if req.is_permission() {
        Some(permission_event(req, owner))
    } else if req.is_elicitation() {
        Some(elicitation::request_event(req, owner))
    } else {
        None
    }
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
