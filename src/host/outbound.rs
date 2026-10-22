//! The host's requests to the agent (ADR 2.4): every one the agent hasn't
//! answered, whoever asked (the editor; the host, for a start, a bridge or a
//! message it sent as a prompt), by the id it has on the agent's side, with
//! who asked and what its answer gets.
//!
//! It does no I/O. A request goes in under an id it gives
//! ([`Outbound::next_id`], [`Outbound::insert`]) and comes out once, as the
//! agent answers it ([`Outbound::answered`]); the host acts on what comes
//! out (`Host::agent_response`, acp.rs). The rules it keeps:
//!
//! - Ids are `brnr-<n>`, from one counter that brnr's echoes share
//!   (`brnr-echo-<n>`, ADR 7.2): none is given twice in a process, so no two
//!   requests waiting have the same one, whatever ids the editor writes.
//! - An answer is its request's once: a second with that id, or one with
//!   an id the host never gave (a request in a line it couldn't read,
//!   ADR 2.5), is nobody's here, and goes on as it came.
//! - The editor's `$/cancel_request` names its own ids, not the host's
//!   ([`Outbound::editors`]).

use std::collections::HashMap;

use serde_json::Value;

use super::id_key;
use super::requests::{HostRequest, Open};
use crate::log::Dir;

/// The requests to the agent it hasn't answered, by their ids there.
#[derive(Default)]
pub(super) struct Outbound {
    asked: HashMap<String, Asked>,
    /// The last id given.
    last: u64,
}

/// A request to the agent it hasn't answered.
pub(super) struct Asked {
    /// Whose its answer is.
    pub(super) by: By,
    /// The session it names (`sessionId`), whose files its answer is
    /// recorded in.
    pub(super) session: Option<String>,
    /// A `session/new` or `session/fork`: the session its answer names
    /// opens, before the answer is recorded.
    pub(super) opens: Option<Opens>,
    /// What its answer gets.
    pub(super) then: Then,
}

/// Who asked: the answer goes to that one only (ADR 2.4).
pub(super) enum By {
    /// The editor, with its id: as it wrote it (`raw`), which its answer goes
    /// back with, and as a key, for its `$/cancel_request`.
    Editor { raw: Vec<u8>, key: String },
    /// The host: the answer is its alone.
    Host,
}

/// A `session/new`, or a `session/fork` of `forked_from`, the editor's or
/// the host's. Its request is logged in the session's file once the answer
/// says which session it opened.
pub(super) struct Opens {
    pub(super) cwd: Option<String>,
    /// The request as recorded, once it is.
    pub(super) request: Option<Vec<u8>>,
    pub(super) dir: Dir,
    pub(super) forked_from: Option<String>,
}

impl Opens {
    /// What request `method` with `params`, sent `dir`, opens, if it opens a
    /// session.
    pub(super) fn of(method: &str, params: &Value, dir: Dir) -> Option<Opens> {
        if !matches!(method, "session/new" | "session/fork") {
            return None;
        }
        let cwd = params["cwd"].as_str().map(str::to_owned);
        // A fork's source is the session it names (ADR 3.12).
        let forked_from = (method == "session/fork")
            .then(|| params["sessionId"].as_str().map(str::to_owned))
            .flatten();
        Some(Opens { cwd, request: None, dir, forked_from })
    }
}

/// What an answer gets, besides going to whoever asked.
pub(super) enum Then {
    /// Nothing more.
    Relay,
    /// One of the editor's whose answer changes what the host knows: noted
    /// as it goes by, before it is recorded and sent on (see acp.rs).
    Note(Note),
    /// A `session/prompt` of this session, the editor's or a message the
    /// host sent: once it is recorded and sent on, the answer ends the turn
    /// (see turns.rs).
    Turn(String),
    /// One of the host's own, acted on once its answer is recorded (see
    /// requests.rs).
    Host(HostRequest),
}

/// What the host notes of the answer to one of the editor's requests.
pub(super) enum Note {
    /// `initialize`: what the agent can do.
    Initialize,
    /// `session/load` (`load`) or `session/resume`.
    Attach { session: String, cwd: Option<String>, load: bool },
    /// `session/close`.
    Close { session: String },
    /// `session/delete`, passed through as it is (ADR 8.1).
    Delete { session: String },
    /// `_session/steering`, passed through as it is: its text is a
    /// `user_message` once the agent has taken it into the turn.
    Steering { session: String, text: String },
}

impl Outbound {
    /// A new id for a request to the agent, `brnr-<n>`.
    pub(super) fn next_id(&mut self) -> Value {
        self.last += 1;
        Value::String(format!("brnr-{}", self.last))
    }

    /// A new id for one of brnr's echoes to the editor, `brnr-echo-<n>`: the
    /// requests' counter, so that it is no request's.
    pub(super) fn echo_id(&mut self) -> String {
        self.last += 1;
        format!("brnr-echo-{}", self.last)
    }

    /// The request sent with `id`, one [`Outbound::next_id`] gave, until the
    /// agent answers it. Returns its key.
    pub(super) fn insert(&mut self, id: &Value, asked: Asked) -> String {
        let key = id_key(id);
        let earlier = self.asked.insert(key.clone(), asked);
        debug_assert!(earlier.is_none(), "id {key} given twice");
        key
    }

    /// The request the agent answered with id `key`, out of the table:
    /// `None` if there is none (any more).
    pub(super) fn answered(&mut self, key: &str) -> Option<Asked> {
        self.asked.remove(key)
    }

    /// Whether a request with id `key` is waiting.
    pub(super) fn waiting(&self, key: &str) -> bool {
        self.asked.contains_key(key)
    }

    /// The ids, on the agent's side, of the editor's requests waiting that
    /// it gave id `key`, in order.
    pub(super) fn editors(&self, key: &str) -> Vec<String> {
        let mut wires: Vec<String> = (self.asked.iter())
            .filter(|(_, a)| matches!(&a.by, By::Editor { key: k, .. } if k == key))
            .map(|(wire, _)| wire.clone())
            .collect();
        wires.sort();
        wires
    }

    /// Whether a bridge's `new` or `resume` is waiting for the agent's
    /// answer: a session not among the host's yet, which keeps a headless
    /// process from stopping as if it had none (see `Host::stop_if_empty`,
    /// `Host::fire_idle_timers`).
    pub(super) fn opening(&self) -> bool {
        self.asked.values().any(|a| {
            matches!(a.then, Then::Host(HostRequest::Opening(Open::New | Open::Resume(_), _)))
        })
    }

    /// Whether the editor's `session/resume` of `session` waits for the
    /// agent's answer.
    pub(super) fn resuming(&self, session: &str) -> bool {
        self.asked.values().any(|a| {
            matches!(&a.then, Then::Note(Note::Attach { session: s, load: false, .. }) if s == session)
        })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::super::fuzz::Harness;
    use super::*;
    use crate::frame;

    fn asked(by: By, then: Then) -> Asked {
        Asked { by, session: None, opens: None, then }
    }

    #[test]
    fn adr_0002_an_answer_is_its_requests_once() {
        let mut table = Outbound::default();
        let id = table.next_id();
        let key = table.insert(&id, asked(By::Host, Then::Turn("s".into())));
        assert!(table.waiting(&key));
        assert!(
            matches!(table.answered(&key), Some(Asked { then: Then::Turn(s), .. }) if s == "s")
        );
        assert!(table.answered(&key).is_none());
        assert!(!table.waiting(&key));
        // An id it never gave.
        assert!(table.answered("\"brnr-9\"").is_none());
        assert!(table.answered("\"7\"").is_none());
    }

    #[test]
    fn adr_0002_ids_are_never_given_twice() {
        let mut table = Outbound::default();
        let a = table.next_id();
        let echo = table.echo_id();
        let b = table.next_id();
        assert_eq!((a, echo.as_str(), b), (json!("brnr-1"), "brnr-echo-2", json!("brnr-3")));
        // Two of the editor's with one id of its own are two requests here.
        let editor = || By::Editor { raw: b"7".to_vec(), key: "7".into() };
        let (c, d) = (table.next_id(), table.next_id());
        let (c, d) = (
            table.insert(&c, asked(editor(), Then::Relay)),
            table.insert(&d, asked(editor(), Then::Relay)),
        );
        assert_ne!(c, d);
        assert_eq!(table.editors("7"), [c.clone(), d]);
        // An id the host gave isn't the editor's.
        assert!(table.editors(&c).is_empty() && table.waiting(&c));
    }

    #[test]
    fn adr_0002_a_second_answer_reaches_nobody() {
        let mut h = Harness::headless(false, None);
        h.sent();
        let initialized = br#"{"jsonrpc":"2.0","id":"brnr-1","result":{"protocolVersion":1}}"#;
        h.agent_line(initialized);
        assert!(h.sent().agent[0].windows(11).any(|w| w == b"session/new"));
        // Its follow-up ran once: no second session/new.
        h.agent_line(initialized);
        assert!(h.sent().agent.is_empty());
    }

    #[test]
    fn adr_0002_an_answer_to_an_unknown_id_goes_on_as_it_came() {
        let mut h = Harness::editor(false, false);
        let line = b"{\"jsonrpc\":\"2.0\",\"id\":\"brnr-9\",\"result\":{}}\n";
        h.agent_line(line);
        assert_eq!(h.sent().editor, [(frame::DATA, line.to_vec())]);
    }

    #[test]
    fn adr_0002_an_answer_for_a_closed_session_goes_to_whoever_asked() {
        let mut h = Harness::editor(false, false);
        h.claim("s1");
        h.editor_bytes(b"{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"session/prompt\",\"params\":{\"sessionId\":\"s1\",\"prompt\":[]}}\n");
        h.editor_bytes(b"{\"jsonrpc\":\"2.0\",\"id\":8,\"method\":\"session/close\",\"params\":{\"sessionId\":\"s1\"}}\n");
        h.sent();
        h.agent_line(b"{\"jsonrpc\":\"2.0\",\"id\":\"brnr-2\",\"result\":{}}\n");
        assert!(h.host().find("s1").is_none());
        // The prompt's answer, its session gone: the editor's, with its id.
        h.agent_line(
            b"{\"jsonrpc\":\"2.0\",\"id\":\"brnr-1\",\"result\":{\"stopReason\":\"cancelled\"}}\n",
        );
        let sent = h.sent().editor;
        let want = b"{\"jsonrpc\":\"2.0\",\"id\":7,\"result\":{\"stopReason\":\"cancelled\"}}\n";
        assert!(sent.iter().any(|(_, line)| line == want), "{sent:?}");
        assert!(!h.host().outbound.waiting("\"brnr-1\""));
    }
}
