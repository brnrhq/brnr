//! The ACP stream: sessions, turns, injection, and the host as the agent's
//! client when no editor is attached.
//!
//! The host reads ACP line by line from both sides. It changes the stream
//! in these ways; everything else passes through unchanged:
//!
//! - Every request to the agent has an id of the host's (`brnr-<n>`), the
//!   editor's too, and the answer is for whoever sent it: the editor's goes
//!   back to it with its id as it wrote it (ADR 2.4). In a line, only the id
//!   changes. The editor's `$/cancel_request` names its own requests, and
//!   goes with their ids on the agent's side.
//! - The editor's `initialize` loses the `fs` and `terminal` client
//!   capabilities, as ACP v2 has neither, but those the profile names
//!   (`client_capabilities`), and none in strict mode (ADR 2.7 in docs/adr).
//! - An injected message goes to the agent as a `session/prompt`, never
//!   while a prompt is running: until then it is held. With `--steer` it
//!   goes into the running turn instead, as `_session/steering` (ADR 4.2 in
//!   docs/adr; turns.rs has the order). The responses are kept from the editor, and the editor is
//!   shown the text as it is sent, as a completed tool call or, if its
//!   profile says so, as `user_message_chunk`s (`echo_message`, ADR 7.2). On
//!   an editor's session it is the experimental `send` (ADR 7.1), refused
//!   while a prompt runs.
//! - Held context is appended to the next `session/prompt`, whoever sends
//!   it.
//! - What the side channel does to an editor's session (ADR 7.1, see
//!   experimental.rs): a permission request or an elicitation the host
//!   answered in the editor's place is withdrawn from it
//!   (`$/cancel_request`), and its late answer dropped; brnr's notes to it
//!   take the echo's form; and a session brnr closed under it has its later
//!   requests answered by the host.
//! - With no editor attached, the host answers what the agent asks of its
//!   client. Permission requests wait for an allow or reject from a bridge
//!   or the CLI, by the option's kind (ADR 8.1), until `response_timeout`
//!   rejects them once, or cancels their turn; how much the agent asks is
//!   the agent's mode (`--mode`, see ADR 4.5 in docs/adr). Elicitations
//!   (`elicitation/create`, a form or a URL for the user) wait the same
//!   way for an accept, decline or cancel, until the same timeout cancels
//!   them (ADR 4.5, see elicitation.rs). Anything else gets "method not
//!   found": `fs` and `terminal` too, which a headless process doesn't
//!   offer (ADR 2.7 in docs/adr).
//! - The agent's `$/cancel_request` for a permission request or an
//!   elicitation it asked ends it, `by: agent` (ADR 4.5): headless, the
//!   host answers it `cancelled` or `cancel`; an editor gets the
//!   notification as it came, and its answer goes on to the agent.
//! - While a `session/load`, headless or the editor's, replays a session's
//!   history (ADR 5.6): if brnr has a transcript of the session, the replayed
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
//! - The editor's `session/load`, `session/resume` or `session/prompt` of a
//!   session another process holds is answered by the host with an error,
//!   naming the process and how to release it, and never reaches the agent
//!   (ADR 3.9); with `shared_sessions` in the profile (ADR 3.10) it goes
//!   through, and the session is served shared (see `Hold`, sessions.rs).
//!   A session the editor's prompt opens has no transcript until the agent
//!   says anything of it, or the editor loads it, and closes again once the
//!   agent has failed everything the editor asked of it having said nothing
//!   of it (`Session::pending`, ADR 5.6).
//!
//! The editor's `session/delete` passes through as it is; once the agent has
//! deleted the session, the host records a `session_deleted` (ADR 8.1).
//!
//! The editor's own steers (`_session/steering`) pass through untouched but
//! for their ids, as every request does; once the agent has taken one into
//! the turn, it is a `user_message` by the editor.
//!
//! Lines are read as json.rs reads them: any JSON text, a lone surrogate as
//! U+FFFD. One that isn't JSON at all passes through untracked (ADR 2.5 in
//! docs/adr): an editor's answer to an id the host doesn't know goes on to
//! the agent, but for its late answers to requests the host answered itself.
//! What the host acts on in a line is read with ACP's schema types, and
//! what they don't take as far as the host can (ADR 2.3, see schema.rs).

use std::fs;
use std::mem::take;
use std::path::PathBuf;
use std::time::{Instant, SystemTime};

use serde_json::{Map, Value, json};

use super::approvals::{AgentRequest, Client, EditorAnswer, Resolved, Timeout, Who};
use super::flow::LINE_BYTES;
use super::lines::Piece;
use super::outbound::{Asked, By, Note, Opens, Then};
use super::requests::{HostRequest, PeerOp};
use super::sessions::{Ending, Hold};
use super::state::SessionState;
use super::turns::{self, Activity, Do, Held, Turns};
use super::{Host, id_key, text_block};
use crate::config::{ClientCapability, EchoAs, Feature, Log};
use crate::error::{Code, Error};
use crate::lock;
use crate::log::{self, Dir};
use crate::schema::{self, ContentBlock, MessageId, NewSessionResponse, SessionUpdate};
use crate::{frame, json, paths, transcript};

/// A `session/load`'s replay of history, while it lasts (ADR 5.6).
pub(super) struct Replay {
    /// Recorded and made into events: brnr has no transcript of the session.
    pub(super) record: bool,
    /// The updates the agent has replayed.
    pub(super) updates: u64,
    /// Of those, the ones recorded: all if `record`, but those that came
    /// while the session was closed (see sessions.rs).
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
    /// (ADR 3.12); not known for one opened any other way.
    pub(super) forked_from: Option<String>,
    /// Its turns, and the messages and context held for them (see
    /// turns.rs).
    pub(super) turns: Turns,
    /// The agent message (or thought) so far, for the `agent_message` and
    /// `agent_thought` events.
    agent_text: String,
    agent_text_kind: &'static str,
    agent_message_id: Option<MessageId>,
    /// What the agent has said about the session (see state.rs).
    pub(super) state: SessionState,
    /// `session/load` is replaying history (see the module docs).
    pub(super) replay: Option<Replay>,
    /// Since when nothing has been running, held or waiting for an answer
    /// (see `fire_idle_timers`); `idle_done` once its time ran out.
    idle_since: Option<Instant>,
    idle_done: bool,
    /// When it last had an event, or opened.
    pub(super) last_active: SystemTime,
    /// How this process holds it (ADR 3.9).
    pub(super) hold: Hold,
    /// A close asked for, until the agent has closed it: meanwhile the
    /// session takes no more requests (see `close`).
    pub(super) closing: Option<Close>,
    /// Opened for a bridge's `new` or `resume`, which hasn't committed yet
    /// (see requests.rs): it takes no requests, and its idle time doesn't
    /// count.
    pub(super) opening: bool,
    /// Opened for the editor's prompt, and the agent has said nothing of it
    /// yet: it closes again once the agent has failed everything the editor
    /// asked of it, its last failure closing it (see `close_if_failed`,
    /// `open_prompted`, ADR 5.6).
    pub(super) pending: bool,
    /// Its files aren't open yet: it is pending, and no load of it has
    /// opened them. What is recorded of it goes to the host log meanwhile.
    pub(super) unopened: bool,
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
    /// `session`, as it opens, held as `hold`, with the replay a load of it
    /// still has, if any (see sessions.rs).
    pub(super) fn new(
        session: &str,
        cwd: PathBuf,
        forked_from: Option<&str>,
        hold: Hold,
        replay: Option<Replay>,
    ) -> Session {
        Session {
            id: session.to_owned(),
            cwd,
            forked_from: forked_from.map(str::to_owned),
            turns: Turns::new(session),
            agent_text: String::new(),
            agent_text_kind: "agent_message",
            agent_message_id: None,
            state: SessionState::default(),
            replay,
            idle_since: None,
            idle_done: false,
            last_active: SystemTime::now(),
            hold,
            closing: None,
            opening: false,
            pending: false,
            unopened: false,
        }
    }

    pub(super) fn shared(&self) -> bool {
        matches!(self.hold, Hold::Shared(_))
    }

    /// Why this process isn't the session's owner, if it isn't, to follow
    /// "which": a headless process doesn't serve such a session (ADR 3.9).
    /// Another process's is `session_active`; one that can't be locked has
    /// no code of its own.
    pub(super) fn not_owned(&self) -> Option<Error> {
        match &self.hold {
            Hold::Owner(_) => None,
            Hold::Shared(pid) => {
                Some(Error::new(Code::session_active, format!("is running in process {pid}")))
            }
            Hold::Unlocked(why) => Some(Error::from(format!("can't be locked: {why}"))),
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

impl Host {
    // ---- editor → agent ------------------------------------------------

    /// What the editor writes, as lines. One longer than `LINE_BYTES` isn't
    /// read: it goes to the agent as it comes (ADR 10.3).
    pub(super) fn editor_bytes(&mut self, bytes: &[u8]) {
        for piece in self.editor_lines.push(bytes) {
            match piece {
                Piece::Line(mut line) => {
                    line.pop(); // Its newline: the line is read without it.
                    self.editor_line(line);
                }
                Piece::Part { bytes, first } => {
                    if first {
                        self.too_long("editor");
                    }
                    self.write_agent(&bytes);
                }
            }
        }
    }

    /// A line past `LINE_BYTES` from `from` (ADR 10.3): with an editor it goes
    /// on unread, headless (from the agent) it is dropped.
    fn too_long(&mut self, from: &str) {
        let relayed = from == "editor" || self.link.is_some();
        self.emit_to_sessions(json!({
            "event": "line_too_long", "from": from, "limit": LINE_BYTES, "relayed": relayed,
        }));
    }

    /// Whether `line` from `from` nests past `json::DEPTH`, and if it does,
    /// says so (ADR 2.5): as a line too long, it goes on unread with an
    /// editor, and headless (from the agent) it is dropped. Either way it
    /// isn't recorded: what would be redacted in it can't be found.
    fn too_deep(&mut self, from: &str, line: &[u8]) -> bool {
        if json::depth(line) <= json::DEPTH {
            return false;
        }
        let relayed = from == "editor" || self.link.is_some();
        self.emit_to_sessions(json!({
            "event": "line_too_deep", "from": from, "max_depth": json::DEPTH, "relayed": relayed,
        }));
        true
    }

    /// A part of a line from the agent past `LINE_BYTES`, `first` where it
    /// passed it: as `too_long` says.
    pub(super) fn agent_part(&mut self, bytes: Vec<u8>, first: bool) {
        if first {
            self.too_long("agent");
        }
        self.send_link_owned(frame::DATA, bytes);
    }

    fn editor_line(&mut self, mut line: Vec<u8>) {
        if self.too_deep("editor", &line) {
            line.push(b'\n');
            return self.write_agent(&line);
        }
        let msg = json::parse(&line, json::DEPTH);
        self.editor_message(line, msg);
    }

    fn editor_message(&mut self, mut line: Vec<u8>, msg: Option<Value>) {
        let mut session = None;
        let mut asking = None;
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
                        let error = Error::new(Code::session_closed, error);
                        return self.refuse(id, sid, &method, &error);
                    }
                    // A prompt of a session not open here opens it, as a
                    // load does: after `closed_error`, which says why one
                    // brnr closed under the editor is gone (ADR 3.9, 7.1).
                    if method == "session/prompt"
                        && let Some(sid) = &session
                        && let Err(error) = self.attach(sid)
                    {
                        return self.refuse(id, sid, &method, &error);
                    }
                    // It goes with an id of the host's, which nobody else's
                    // can be: the editor's as it wrote it comes back on the
                    // answer (ADR 2.4). Only the id changes in the line.
                    let wire = self.outbound.next_id();
                    let key = id_key(&wire);
                    let spans = json::members(&line, "id");
                    let raw = match spans.last() {
                        Some(span) => line[span.clone()].to_vec(),
                        None => serde_json::to_vec(&id).unwrap(),
                    };
                    msg.insert("id".to_owned(), wire.clone());
                    let (rewritten, then) = self.editor_request(&method, &key, &mut msg, &session);
                    line = match rewritten {
                        Some(rewritten) => rewritten,
                        None if !spans.is_empty() => {
                            json::replace(&line, &spans, &serde_json::to_vec(&wire).unwrap())
                        }
                        None => serde_json::to_vec(&msg).unwrap(),
                    };
                    let by = By::Editor { raw, key: id_key(&id) };
                    let params = msg.get("params").unwrap_or(&Value::Null);
                    let opens = Opens::of(&method, params, Dir::EditorToAgent);
                    asking = Some((wire, Asked { by, session: session.clone(), opens, then }));
                    if method == "session/new" {
                        session = None;
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
                (None, Some(id)) => match self.approvals.editor_answer(&id_key(&id), &msg) {
                    EditorAnswer::Resolved(resolved) => {
                        session = resolved.req.session.clone();
                        self.resolved(*resolved);
                    }
                    // Answered by the host already (a cancel, an allow or
                    // reject): the agent must not get a second answer.
                    EditorAnswer::Late(answered) => return self.late_answer(&id, answered, &msg),
                    // The agent cancelled it, and the editor answers as ACP
                    // has it: on to the agent, as it is (ADR 4.5).
                    EditorAnswer::GivenUp(given_up) => session = given_up,
                    // It answers a request the host couldn't read, or none:
                    // it goes on, and the agent ignores an answer to
                    // something it never asked (ADR 2.5 in docs/adr).
                    EditorAnswer::Unknown => {}
                },
                _ => {}
            }
            recorded = log::redacted(&msg);
        }
        line.push(b'\n');
        // A session/new goes in the host log now, so one the agent never
        // answers is still on record, and a session/fork in its source's
        // file, as `brnr session fork`'s does; either goes in the session's
        // file too once the response names it. What is recorded keeps the
        // MCP servers' secrets out (ADR 9.1 in docs/adr); the agent gets the
        // line as it came, but for a request's id (ADR 2.4) and the named
        // changes.
        let recorded = recorded.as_deref().unwrap_or(&line);
        self.record(session.as_deref(), Dir::EditorToAgent, recorded);
        if let Some((wire, mut asked)) = asking {
            if let Some(opens) = &mut asked.opens {
                opens.request = Some(recorded.to_vec());
            }
            self.outbound.insert(&wire, asked);
        }
        self.write_agent(&line);
    }

    /// The editor's `$/cancel_request` of its request `requestId`: the
    /// cancellations to send the agent instead, each with the id that request
    /// has there (one each, if the editor gave several requests that id). An
    /// id the host gave a request on the agent's side, which the editor
    /// never had, isn't one of its own to cancel: nothing goes, and the host
    /// log says so. Any other goes as it came (`None`): the id of a request
    /// the host couldn't read (ADR 2.5), or of none.
    fn editor_cancel(&mut self, msg: &Map<String, Value>) -> Option<Vec<Value>> {
        let named = msg.get("params")?.get("requestId")?;
        let key = id_key(named);
        let wires = self.outbound.editors(&key);
        if wires.is_empty() {
            if !self.outbound.waiting(&key) {
                return None;
            }
            let event = json!({ "event": "editor_cancel_dropped", "rpc_id": named });
            self.sink.note(None, event);
            return Some(Vec::new());
        }
        let cancels = wires.into_iter().map(|wire| {
            let mut cancel = Value::Object(msg.clone());
            cancel["params"]["requestId"] = serde_json::from_str(&wire).unwrap();
            cancel
        });
        Some(cancels.collect())
    }

    /// What the editor sent after its last line, as it closed its stdin: it
    /// goes to the agent as it is, and is recorded as a line would be; one
    /// nested past `json::DEPTH` isn't, and is said, as `editor_line` has it.
    pub(super) fn record_editor_rest(&mut self, rest: &[u8]) {
        if self.too_deep("editor", rest) {
            return;
        }
        let msg = json::parse::<Value>(rest, json::DEPTH);
        let recorded = msg.as_ref().and_then(Value::as_object).and_then(log::redacted);
        self.record(None, Dir::EditorToAgent, recorded.as_deref().unwrap_or(rest));
    }

    /// Notes requests that change session state: what their answer gets,
    /// and the message re-encoded if the host changed it.
    fn editor_request(
        &mut self,
        method: &str,
        key: &str,
        msg: &mut Map<String, Value>,
        session: &Option<String>,
    ) -> (Option<Vec<u8>>, Then) {
        let cwd = msg.get("params").and_then(|p| p["cwd"].as_str()).map(str::to_owned);
        let note = match (method, session) {
            ("initialize", _) => {
                return (self.drop_capabilities(msg), Then::Note(Note::Initialize));
            }
            ("session/load" | "session/resume", Some(session)) => {
                let load = method == "session/load";
                if load {
                    self.editor_load(session, cwd.as_deref());
                }
                Note::Attach { session: session.clone(), cwd, load }
            }
            ("session/close", Some(session)) => {
                // Dropped now: held until the agent answers, they would go out as
                // the running turn ends.
                if let Some(i) = self.find(session) {
                    self.drop_all(i, "close");
                }
                Note::Close { session: session.clone() }
            }
            ("session/delete", Some(session)) => Note::Delete { session: session.clone() },
            ("session/prompt", Some(session)) => {
                let rewritten = self.editor_prompt(session, key, msg);
                return (rewritten, Then::Turn(session.clone()));
            }
            ("_session/steering", Some(session)) => {
                let text = turns::prompt_text(msg.get("params").and_then(|p| p.get("prompt")));
                Note::Steering { session: session.clone(), text }
            }
            _ => return (None, Then::Relay),
        };
        (None, Then::Note(note))
    }

    /// The editor's `session/load`, `session/resume` or `session/prompt` of
    /// `session`: its lock is taken before the agent hears of it. One another
    /// process holds is refused, saying how to release it (ADR 3.9), unless
    /// the profile shares sessions (`shared_sessions`, ADR 3.10).
    fn attach(&mut self, session: &str) -> Result<(), Error> {
        if self.sessions.knows(session) {
            return Ok(()); // Ours already.
        }
        let hold = self.sessions.take_lock(session, &self.sink);
        if let Hold::Shared(pid) = hold
            && !self.features.contains(&Feature::SharedSessions)
        {
            return Err(Error::new(Code::session_active, held_elsewhere(session, pid)));
        }
        self.sessions.claim(session, hold);
        Ok(())
    }

    /// The editor's `session/load` of `session`, whose history the agent
    /// replays before it answers: recorded only if brnr has no transcript of
    /// the session, in any folder, as a headless start's and a bridge's are
    /// (ADR 5.6): an events file holding only openings, which a load that
    /// failed leaves, is none ([`transcript::has_records`]). One not open here
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
            None if self.sessions.closed_load(session).is_some() => {
                Some(self.open_session(session, cwd))
            }
            open => open,
        };
        if let Some(replay) = open.and_then(|i| self.sessions[i].replay.as_mut()) {
            replay.loads += 1;
            replay.unanswered += 1;
            return;
        }
        // One the editor's prompt opened has no files yet (see
        // `open_prompted`): its replay is recorded as if not open here.
        let record = match open.map(|i| (&self.sessions[i].hold, self.sessions[i].unopened)) {
            Some((Hold::Owner(_) | Hold::Unlocked(_), false)) => self.logging == Log::Off,
            _ => !self.has_transcript(session),
        };
        let i = self.open_session(session, cwd);
        // Its files open, so that its replay is recorded; it is still
        // pending, and the prompt's, if the agent says nothing of it.
        self.open_files(i, cwd);
        self.begin_replay(i, record);
        if let Some(replay) = &mut self.sessions[i].replay {
            replay.opened = open.is_none();
        }
    }

    /// The agent's answer to one of the editor's loads of `session`, a
    /// failure too. The replay ends with the last of the loads it covers:
    /// with its `history` event if any of them succeeded; if none did, a
    /// session opened for them closes again (ADR 5.6).
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
            self.confirm(i, cwd);
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
            self.close_session(i, Ending::OpenFailed, None);
        } else {
            // Loaded again, or resumed, or prompted, and failed.
            self.drop_replay(i);
            self.close_if_failed(i);
        }
    }

    /// Answers the editor's request `id` with an error in the agent's place:
    /// the agent never hears of it. The editor gets a JSON-RPC error, as ACP
    /// has it; the host log has brnr's (ADR 8.7).
    fn refuse(&mut self, id: Value, session: &str, method: &str, error: &Error) {
        let event = json!({
            "event": "editor_request_refused",
            "session": session,
            "rpc_id": id,
            "method": method,
            "error": error.json(),
        });
        self.sink.note(None, event);
        let rpc = json!({ "code": -32603, "message": error.message });
        let msg = json!({ "jsonrpc": "2.0", "id": id, "error": rpc });
        self.write_editor(None, &msg);
    }

    /// Removes the editor's `fs` and `terminal` client capabilities from its
    /// `initialize`, as ACP v2 does, but those the profile names
    /// (`client_capabilities`), and none in strict mode: stable ACP v1 has
    /// them (ADR 2.7 in docs/adr). The host log says what it dropped.
    fn drop_capabilities(&mut self, msg: &mut Map<String, Value>) -> Option<Vec<u8>> {
        if self.strict {
            return None;
        }
        let caps = msg.get_mut("params")?.get_mut("clientCapabilities")?.as_object_mut()?;
        let dropped: Map<String, Value> = (ClientCapability::ALL.into_iter())
            .filter(|c| !self.client_capabilities.contains(c))
            .filter_map(|c| Some((c.name().to_owned(), caps.shift_remove(c.name())?)))
            .collect();
        if dropped.is_empty() {
            return None;
        }
        self.sink.note(None, json!({ "event": "capabilities_dropped", "capabilities": dropped }));
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
        let i = self.open_prompted(session);
        let rewritten = self.attach_context(i, session, msg);
        let prompt = msg.get("params").and_then(|p| p.get("prompt"));
        let sent = self.sessions[i].turns.editor_prompt(key, prompt);
        self.act(i, vec![sent]);
        rewritten
    }

    /// Appends held context to an editor prompt and echoes it.
    fn attach_context(
        &mut self,
        i: usize,
        session: &str,
        msg: &mut Map<String, Value>,
    ) -> Option<Vec<u8>> {
        let Some(Value::Array(blocks)) = msg.get_mut("params").and_then(|p| p.get_mut("prompt"))
        else {
            return None;
        };
        let context = self.sessions[i].turns.take_context();
        if context.is_empty() {
            return None;
        }
        let attached: Vec<Value> = context.iter().map(|t| text_block(t)).collect();
        blocks.extend(attached.iter().cloned());
        let event =
            json!({ "event": "context_attached", "to": "editor_prompt", "count": context.len() });
        self.sink.note(Some(session), event);
        self.echo_message(session, "Context via brnr", &attached);
        Some(serde_json::to_vec(&msg).unwrap())
    }

    // ---- agent → editor ------------------------------------------------

    pub(super) fn agent_line(&mut self, line: &[u8]) {
        let body = line.strip_suffix(b"\n").unwrap_or(line);
        if self.too_deep("agent", body) {
            return self.send_link(frame::DATA, line);
        }
        let msg = json::parse(body, json::DEPTH);
        self.agent_message(line, msg);
    }

    fn agent_message(&mut self, line: &[u8], msg: Option<Value>) {
        let Some(Value::Object(msg)) = msg else {
            self.record(None, Dir::AgentToEditor, line);
            self.send_link(frame::DATA, line);
            return;
        };
        let method = msg.get("method").and_then(Value::as_str).map(str::to_owned);
        // The agent has a session it names, before the line is recorded
        // (see `confirm`).
        if method.is_some()
            && let Some(i) = param_session(&msg).and_then(|s| self.find(&s))
        {
            self.confirm(i, None);
        }
        match (method, msg.get("id").cloned()) {
            (None, Some(id)) => self.agent_response(&id_key(&id), &msg, line),
            (Some(method), Some(id)) => self.agent_request(method, id, &msg, line),
            (Some(method), None) if method == "$/cancel_request" => self.agent_cancel(&msg, line),
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
                        // still gets it, as the agent sent it (ADR 3.1).
                        if update {
                            self.replayed_state(i, &msg["params"]["update"]);
                        }
                        return self.send_link(frame::DATA, line);
                    }
                    replay.recorded_updates += u64::from(update);
                } else if let Some(replay) =
                    session.as_deref().and_then(|s| self.sessions.closed_load(s))
                {
                    // Its session closed while it was loading: counted, with
                    // no files to record it in (ADR 5.6).
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

    /// The agent's `$/cancel_request`: a permission request or an
    /// elicitation it no longer waits for ends, `by: agent` (ADR 4.5, see
    /// approvals.rs). The notification is recorded in the request's
    /// session, live whatever a load replays, and goes on to an editor as it
    /// came; then the host's answer, headless, and the event.
    fn agent_cancel(&mut self, msg: &Map<String, Value>, line: &[u8]) {
        let named = msg.get("params").and_then(|p| p.get("requestId"));
        let given_up = named.and_then(|id| self.approvals.give_up(&id_key(id), self.client()));
        let session = given_up.as_ref().and_then(|r| r.req.session.clone());
        // Cancelled after its session closed: nothing is kept past that (ADR 10.3).
        if let Some(s) = &session
            && self.find(s).is_none()
        {
            self.approvals.closed(s);
        }
        self.record(session.or_else(|| param_session(msg)).as_deref(), Dir::AgentToEditor, line);
        self.send_link(frame::DATA, line);
        if let Some(resolved) = given_up {
            self.resolved(resolved);
        }
    }

    /// The agent answered request `key`: whatever asked gets it, and what
    /// it was asked for follows from it (see outbound.rs).
    fn agent_response(&mut self, key: &str, msg: &Map<String, Value>, line: &[u8]) {
        // One to an id the host never sent (a request in a line it couldn't
        // read, ADR 2.5), or answered already, goes on as it came.
        let Some(Asked { by, mut session, opens, then }) = self.outbound.answered(key) else {
            self.record(None, Dir::AgentToEditor, line);
            return self.send_link(frame::DATA, line);
        };
        let result = msg.get("result");
        if let Some(opens) = opens {
            session = self.opened(opens, result).or(session);
        }
        match then {
            Then::Relay => self.relay(by, session.as_deref(), line),
            Then::Note(note) => {
                let session = self.noted(note, result).or(session);
                self.relay(by, session.as_deref(), line);
            }
            Then::Turn(session) => {
                // A turn it served: the session is the agent's.
                if let Some(i) = result.and_then(|_| self.find(&session)) {
                    self.confirm(i, None);
                }
                self.relay(by, Some(&session), line);
                self.turn_answered(&session, key, msg);
            }
            Then::Host(request) => {
                self.relay(by, session.as_deref(), line);
                self.host_request_done(request, msg);
            }
        }
    }

    /// An answer, recorded in `session`'s files, for whoever asked
    /// (ADR 2.4): the editor's goes to it with its id as it wrote it, and
    /// nothing else in the line changed; the host's is its alone.
    fn relay(&mut self, by: By, session: Option<&str>, line: &[u8]) {
        match by {
            By::Editor { raw, .. } => {
                self.record(session, Dir::AgentToEditor, line);
                self.send_link(frame::DATA, &json::replace(line, &json::members(line, "id"), &raw));
            }
            By::Host => self.record(session, Dir::AgentToControl, line),
        }
    }

    /// The answer to a `session/prompt` of `session`, the editor's or the
    /// host's, `key`: its turn ends (see turns.rs).
    fn turn_answered(&mut self, session: &str, key: &str, msg: &Map<String, Value>) {
        let stop_reason = msg.get("result").and_then(|r| r.get("stopReason"));
        let tokens = turn_tokens(msg.get("result"));
        // The agent's error, whole, under brnr's (ADR 8.7).
        let error = msg.get("error").map(|e| Error::agent(schema::error_message(e), e).json());
        let ended = json!({ "stop_reason": stop_reason, "error": error, "tokens": tokens });
        let Some(i) = self.find(session) else {
            self.emit(turns::turn_ended(session, key, None, ended));
            return;
        };
        self.flush_agent_message(i);
        let todo = self.sessions[i].turns.answered(key, ended);
        let s = &mut self.sessions[i];
        if s.turns.activity() != Activity::Busy {
            // Its tool calls end with it, however it ended: one the agent
            // left unfinished, cancelled or failed, isn't running now
            // (ADR 8.2). Nor are those a replay found.
            s.state.tools.clear();
            if let Some(replay) = &mut s.replay {
                replay.tools.clear();
            }
        }
        self.act(i, todo);
        if self.close_if_failed(i) {
            return;
        }
        self.close_after_turn(i);
    }

    /// The answer to a `session/new` or `session/fork`, `result` if it
    /// succeeded: the session it names opens, and has the request in its
    /// file. Returns the session.
    fn opened(&mut self, opens: Opens, result: Option<&Value>) -> Option<String> {
        let Opens { cwd, request, dir, forked_from } = opens;
        let session = result.and_then(new_session);
        if let Some(session) = &session {
            let i = self.open_fork(session, cwd.as_deref(), forked_from.as_deref());
            self.sessions[i].state.result(result.unwrap_or(&Value::Null));
        }
        if let Some(request) = request {
            // A copy, in the session's file, of what the host log has with its
            // position: none here (ADR 10.2).
            self.sink.msg(session.as_deref(), dir, &request, None);
        }
        session
    }

    /// What the answer to one of the editor's requests changes, `result` if
    /// it succeeded. Returns the session it was about.
    fn noted(&mut self, note: Note, result: Option<&Value>) -> Option<String> {
        match note {
            Note::Attach { session, cwd, load } => {
                if load {
                    self.load_answered(&session, cwd.as_deref(), result);
                } else if let Some(result) = result {
                    let i = self.open_session(&session, cwd.as_deref());
                    self.confirm(i, cwd.as_deref());
                    self.sessions[i].state.result(result);
                    // Open now, whatever a load still to be answered gets.
                    if let Some(replay) = &mut self.sessions[i].replay {
                        replay.opened = false;
                    }
                } else if !self.claim_failed(&session, None)
                    && let Some(i) = self.find(&session)
                {
                    // Claimed, its lock (see `attach`) goes once what was
                    // recorded under it is written: a close's here, if the
                    // resume took it back from one (ADR 5.5). One open
                    // already stays open, unless the editor's prompt opened
                    // it and this was the last the editor asked of it.
                    self.close_if_failed(i);
                }
                Some(session)
            }
            Note::Close { session } => {
                if result.is_some()
                    && let Some(i) = self.find(&session)
                {
                    self.close_session(i, Ending::Closed("editor"), None);
                }
                Some(session)
            }
            Note::Delete { session } => {
                if result.is_some() {
                    self.deleted(&session);
                }
                Some(session)
            }
            Note::Initialize => {
                if let Some(result) = result {
                    self.initialized(result);
                }
                None
            }
            Note::Steering { session, text } => {
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
        let unsupported = format!("{method} is not supported without an editor");
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        let req = AgentRequest::new(id, method, session, params);
        let event = if self.editor_attached() {
            self.send_link(frame::DATA, line);
            self.approvals.ask(req, None, "editor")
        } else if req.kind().is_some() {
            // Held until someone answers it, or `response_timeout` does
            // (ADR 4.5): none, never, for a timeout too far off to say.
            let deadline = self.response_timeout.and_then(|t| Instant::now().checked_add(t));
            self.approvals.ask(req, deadline, "headless")
        } else {
            let error = json!({ "code": -32601, "message": unsupported });
            return self.respond(&req, json!({ "error": error }));
        };
        if let Some(event) = event {
            self.emit(event);
        }
    }

    /// The agent's `elicitation/complete`: a URL's flow is done (ADR 4.5),
    /// as `elicitation_completed`. It names the elicitation, not a session;
    /// the session is the request's, if brnr kept it.
    fn elicitation_complete(&mut self, msg: &Map<String, Value>) -> Option<Value> {
        let id = msg.get("params")?.get("elicitationId")?.as_str()?;
        let (session, handle) = self.approvals.complete(id);
        Some(json!({
            "event": "elicitation_completed",
            "session": session,
            "request": handle,
            "elicitation_id": id,
        }))
    }

    // ---- the host as the agent's client --------------------------------

    /// What the host is as one of the agent's requests is answered.
    pub(super) fn client(&self) -> Client {
        let accepting = self.status.is_none() && self.agent_in.is_some();
        Client { editor: self.editor_attached(), accepting }
    }

    /// One of the agent's requests has ended, whoever answered it (see
    /// approvals.rs): the host's reply goes to the agent, the request is
    /// withdrawn from the editor if it was shown it (see experimental.rs),
    /// then the event.
    pub(super) fn resolved(&mut self, resolved: Resolved) {
        let Resolved { req, answer: _, reply, withdraw, event } = resolved;
        if let Some(reply) = reply {
            self.respond(&req, reply);
        }
        if let Some((how, by)) = withdraw {
            self.withdraw(&req, how, &by);
        }
        if let Some(event) = event {
            self.emit(event);
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
            // agent has it (ADR 5.6).
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

    /// After a prompt or a steer is answered: once no turn runs, a close
    /// that waited for it goes.
    fn close_after_turn(&mut self, i: usize) {
        let s = &mut self.sessions[i];
        if s.turns.activity() != Activity::Busy
            && matches!(s.closing, Some(Close::AfterTurn { .. }))
            && let Some(Close::AfterTurn { peer, req_id, by }) = s.closing.take()
        {
            self.send_close(i, peer, req_id, by);
        }
    }

    /// What a session that is closing or whose process is exiting held,
    /// messages and context alike, is dropped, each with its event.
    pub(super) fn drop_all(&mut self, i: usize, by: &str) {
        let todo = self.sessions[i].turns.drop_all(by);
        self.act(i, todo);
    }

    /// Closes session `i`, `by` `close` (`brnr session close`, `--take-over`)
    /// or `idle`: a running turn is cancelled first, its pending approvals
    /// answered `cancelled`, and `session/close` goes once it has ended
    /// (ADR 3.12). `peer` hears when the agent has closed it (peer 0: nobody).
    pub(super) fn close(&mut self, i: usize, peer: u64, req_id: Option<Value>, by: &'static str) {
        // Dropped now: held until the agent answers, they would go out as the
        // running turn ends.
        self.drop_all(i, "close");
        if !self.is_idle(i) {
            let session = self.sessions[i].id.clone();
            self.cancel(&session, &Who::because("cancel"));
        }
        if self.sessions[i].turns.activity() != Activity::Busy {
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

    /// The agent has deleted `session`, as the editor asked: a
    /// `session_deleted`, and brnr's transcript stays (ADR 8.1). One open here
    /// is gone from the agent, so it closes, as if the editor had closed it.
    /// One no process has open has the event appended to its transcripts
    /// too; one another process holds has it in the host log only, as that
    /// process writes its transcript.
    fn deleted(&mut self, session: &str) {
        let event = json!({ "event": "session_deleted", "session": session, "by": "editor" });
        let event = self.emit(event);
        match self.find(session) {
            Some(i) => self.close_session(i, Ending::Closed("editor"), None),
            // Closed here, and not let go of yet: still this process's to write.
            None if self.sessions.letting_go_of(session) || lock::holder(session).is_none() => {
                self.sink.append(session, event);
            }
            None => {}
        }
    }

    /// Whether session `i` has nothing running, held or waiting for an
    /// answer.
    pub(super) fn is_idle(&self, i: usize) -> bool {
        let s = &self.sessions[i];
        s.turns.activity() == Activity::Idle && !self.approvals.waiting(&s.id)
    }

    /// `stop_when_idle`: a headless session idle that long closes, and the
    /// process stops with its last session, one a bridge is opening
    /// counting (`Outbound::opening`). Idle time counts from the
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
        self.sink.note(Some(&session), json!({ "event": "idle_timed_out" }));
        // A session a bridge is opening keeps the process going.
        if self.sessions.len() == 1 && !self.outbound.opening() {
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

    /// A new `m<n>` message id.
    pub(super) fn message_id(&mut self) -> String {
        self.next_message += 1;
        format!("m{}", self.next_message)
    }

    /// Does what session `i`'s turns say to, in order (see turns.rs).
    pub(super) fn act(&mut self, i: usize, todo: Vec<Do>) {
        let session = self.sessions[i].id.clone();
        for step in todo {
            match step {
                Do::Prompt(held) => self.send_prompt(i, held),
                // Asking the agent to answer `promptRequired` rather than
                // start a turn of its own if none is running
                // (claude-agent-acp's `steer()` has the contract). Held
                // context waits for the next prompt.
                Do::Steer { message, prompt } => {
                    let params = json!({
                        "sessionId": session,
                        "prompt": prompt,
                        "_meta": { "steering": { "idleBehavior": "promptRequired" } },
                    });
                    let steer = HostRequest::Steer { session: session.clone(), message };
                    self.host_request("_session/steering", params, steer);
                }
                Do::Cancel => self.cancel(&session, &Who::because("cancel")),
                Do::Sent { event, echo } => {
                    self.flush_agent_message(i);
                    if let Some(blocks) = echo {
                        self.echo_message(&session, "Message via brnr", &blocks);
                    }
                    self.emit(event);
                }
                Do::Emit(event) => {
                    self.emit(event);
                }
                Do::Note(event) => self.sink.note(Some(&session), event),
            }
        }
    }

    /// Message `held` as session `i`'s next `session/prompt`, held context
    /// appended (see turns.rs): the editor is shown it as it goes.
    pub(super) fn send_prompt(&mut self, i: usize, held: Held) {
        let id = self.outbound.next_id();
        let key = id_key(&id);
        let session = self.sessions[i].id.clone();
        let (blocks, sent) = self.sessions[i].turns.start(key, held);
        let msg = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "session/prompt",
            "params": { "sessionId": session, "prompt": blocks },
        });
        let then = Then::Turn(session.clone());
        let asked = Asked { by: By::Host, session: Some(session.clone()), opens: None, then };
        self.outbound.insert(&id, asked);
        self.act(i, vec![sent]);
        let mut line = serde_json::to_vec(&msg).unwrap();
        line.push(b'\n');
        self.record(Some(&session), Dir::ControlToAgent, &line);
        self.write_agent(&line);
    }

    /// The agent's answer to steering `message` into `session`'s turn (see
    /// turns.rs).
    pub(super) fn steer_answered(
        &mut self,
        session: &str,
        message: &str,
        msg: &Map<String, Value>,
    ) {
        let Some(i) = self.find(session) else { return };
        let todo = self.sessions[i].turns.steer_answered(message, msg);
        self.act(i, todo);
        self.close_after_turn(i);
    }

    /// The editor's own steer, which the agent took into the running turn of
    /// `session`: a `user_message` of that turn, by the editor, as brnr's own
    /// steers are. The steer itself went through untouched.
    fn editor_steered(&mut self, session: &str, text: String) {
        let Some(i) = self.find(session) else { return };
        let sent = self.sessions[i].turns.editor_steered(text);
        self.act(i, vec![sent]);
    }

    /// Answers the requests nobody answered within `response_timeout`, as
    /// ADR 4.5 has it (see `Approvals::time_out`): one whose turn is to be
    /// cancelled is answered with it, as is every request pending in its
    /// session, `cancelled`, as ACP requires.
    pub(super) fn fire_permission_timers(&mut self, now: Instant) {
        if self.editor_attached() {
            return;
        }
        for handle in self.approvals.expired(now) {
            match self.approvals.time_out(&handle, self.client()) {
                None => {}
                Some(Timeout::Resolved(resolved)) => self.resolved(*resolved),
                Some(Timeout::Cancel { session }) => {
                    let (by, reason) = ("control", "timeout");
                    let event = json!({ "event": "cancel_requested", "by": by, "reason": reason, "request": handle });
                    self.sink.note(Some(&session), event);
                    self.cancel(&session, &Who::because("timeout"));
                }
                Some(Timeout::Failed(err)) => {
                    let event = json!({ "event": "timeout_failed", "request": handle, "error": err.json() });
                    self.sink.note(None, event);
                }
            }
        }
    }

    /// The earliest permission deadline, for the event loop's wake-up.
    pub(super) fn next_permission_deadline(&self) -> Option<Instant> {
        if self.editor_attached() {
            return None;
        }
        self.approvals.next_deadline()
    }

    /// Cancels `session`'s turn (`session/cancel`), answering its pending
    /// permission requests `cancelled` and its elicitations `cancel`, by
    /// `by`: the cancel's or the timeout's.
    pub(super) fn cancel(&mut self, session: &str, by: &Who) {
        let msg = json!({
            "jsonrpc": "2.0",
            "method": "session/cancel",
            "params": { "sessionId": session },
        });
        let mut line = serde_json::to_vec(&msg).unwrap();
        line.push(b'\n');
        self.record(Some(session), Dir::ControlToAgent, &line);
        self.write_agent(&line);
        // ACP: a client that cancels a turn answers what the agent asked in it.
        for resolved in self.approvals.cancel(session, by, self.client()) {
            self.resolved(resolved);
        }
    }

    /// Shows the editor an injected message, titled `title`, in the form its
    /// profile's `echo_as` names (ADR 7.2): by default a completed tool call
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
    /// ADR 7.2 in docs/adr). brnr's notes to the editor take this form whatever
    /// `echo_as` says, as they are brnr's words, not the user's (see
    /// experimental.rs).
    pub(super) fn echo(&mut self, session: &str, title: &str, blocks: &[Value]) {
        if let Some(i) = self.find(session) {
            self.flush_agent_message(i);
        }
        if !self.editor_attached() {
            return;
        }
        let echo = self.outbound.echo_id();
        let content: Vec<Value> =
            blocks.iter().map(|b| json!({ "type": "content", "content": b })).collect();
        self.update_editor(
            session,
            json!({
                "sessionUpdate": "tool_call",
                "toolCallId": echo,
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
    /// (ADR 3.1): none once the agent's stdout has ended, after which the
    /// editor has seen it end (ADR 2.6). The host log says what wasn't sent
    /// (P3).
    fn write_editor(&mut self, session: Option<&str>, msg: &Value) {
        if !self.stdout_open {
            let event = json!({
                "event": "editor_message_dropped",
                "reason": "agent_stdout_ended",
                "session": session,
                "method": msg.get("method"),
                "rpc_id": msg.get("id"),
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

    /// Whether brnr has a transcript of `session`, in any folder, counting
    /// what this process has recorded of it that the disk may not have yet;
    /// an events file holding only openings, which a load that failed
    /// leaves, is none (ADR 5.6).
    pub(super) fn has_transcript(&self, session: &str) -> bool {
        self.sink.recorded(session) || transcript::kept(session)
    }
}

/// What the editor is told of a session process `pid` holds: how to
/// release it. One started with another runtime directory is reached
/// through that (ADR 3.9), where its metadata is, read only if the
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

/// The tokens a `session/prompt` answer says its turn used (ADR 4.4): its
/// `usage`, unstable ACP and so read as a `Value` (ADR 2.3), as `{total,
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
