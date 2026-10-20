//! Approvals and elicitations (ADR 4.5): the agent's requests to its
//! client, kept until they are answered, and the one way each ends, whoever
//! answers it: the editor, a bridge or the CLI, a cancel of its turn, or
//! `response_timeout`.
//!
//! It does no I/O. A request goes in as the agent asks it
//! ([`Approvals::ask`]), an answer as a call, and what the host is to do
//! comes back as a [`Resolved`], in order: the reply to the agent, the
//! request withdrawn from the editor, the event. The rules it keeps:
//!
//! - A permission request is held with a handle `p<n>`, an elicitation
//!   with `e<n>`, counted from 1 and never used again in the process. The
//!   agent's other requests are kept only while the editor has them, so
//!   that its answer is recorded in their session.
//! - An answer from outside the editor names the request and its session,
//!   and is refused once the agent no longer takes input: nothing is sent.
//! - A permission request is answered with the option of one kind, or one
//!   named (ADR 8.1, see [`choose`]). A timeout answers `reject_once`, or
//!   has the turn cancelled, never an allow; an elicitation's answers
//!   `cancel`.
//! - A URL elicitation's id is kept until the agent completes its flow, for
//!   `elicitation_completed`: dropped as the request ends, but for an
//!   accept, whose flow goes on.
//! - One the editor was shown that the host answered is withdrawn from it,
//!   and the editor's late answer dropped, so the agent never gets two
//!   (ADR 2.5, 7.1).
//!
//! Nothing ends a request the agent stops waiting for, its turn ended or
//! its `$/cancel_request` (#162): that would be one more way in here.

use std::collections::HashMap;
use std::time::Instant;

use serde_json::{Map, Value, json};

use super::{elicitation, id_key};
use crate::json;
use crate::schema::{self, PermissionOptionKind};

/// The most URL elicitations kept waiting for their completion: past that,
/// an agent that never completes them would have the host keep them without
/// limit, and the completion of one not kept is told without its handle.
const URL_ELICITATIONS: usize = 1024;

/// Why an answer from outside the editor isn't sent: the agent has exited,
/// or its stdin is closed.
const NOT_ACCEPTING: &str = "the agent is no longer accepting input";

/// A request from the agent to its client.
pub(super) struct AgentRequest {
    pub(super) key: String,
    pub(super) id: Value,
    method: String,
    pub(super) session: Option<String>,
    pub(super) params: Value,
    /// `p<n>` for a permission request, `e<n>` for an elicitation: what
    /// bridges and brnr call it. None for a request brnr doesn't hold.
    handle: Option<String>,
    /// When `response_timeout` answers it.
    deadline: Option<Instant>,
}

/// The requests brnr holds, with ACP's words for each (ADR 4.5).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Kind {
    /// `session/request_permission`.
    Permission,
    /// `elicitation/create`.
    Elicitation,
}

impl AgentRequest {
    pub(super) fn new(id: Value, method: String, session: Option<String>, params: Value) -> Self {
        AgentRequest { key: id_key(&id), id, method, session, params, handle: None, deadline: None }
    }

    /// Which of the requests brnr holds it is, if it is one.
    pub(super) fn kind(&self) -> Option<Kind> {
        match self.method.as_str() {
            "session/request_permission" => Some(Kind::Permission),
            "elicitation/create" => Some(Kind::Elicitation),
            _ => None,
        }
    }

    pub(super) fn is_permission(&self) -> bool {
        self.kind() == Some(Kind::Permission)
    }

    pub(super) fn is_elicitation(&self) -> bool {
        self.kind() == Some(Kind::Elicitation)
    }

    /// Its handle, if brnr holds it.
    pub(super) fn held(&self) -> Option<&str> {
        self.handle.as_deref()
    }

    /// When `response_timeout` answers it, if it does.
    pub(super) fn deadline(&self) -> Option<Instant> {
        self.deadline
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

/// How a permission request is answered from outside (ADR 8.1): `allow` or
/// `reject`, once or, with `always`, always.
#[derive(Clone, Copy)]
pub(super) enum Choice {
    Allow,
    Reject,
}

/// The host's answer to a request it holds, as [`choose`] and
/// `elicitation::response` make it.
pub(super) enum Answer {
    /// A permission request's ACP `outcome`, which brnr tells as `how`:
    /// `allowed`, `rejected` or `cancelled`.
    Outcome(Value, &'static str),
    /// An elicitation's ACP result: its `action`, and an accept's `content`.
    Result(Value),
}

/// What the host is as a request ends.
#[derive(Clone, Copy)]
pub(super) struct Client {
    /// An editor is attached: it was shown the request.
    pub(super) editor: bool,
    /// The agent still takes input: an answer from outside the editor can
    /// reach it.
    pub(super) accepting: bool,
}

/// Who answered a request, as its `permission_resolved` or
/// `elicitation_resolved` says (ADR 5.9): `by`, `editor` or `control`; the
/// connection that asked (`peer`), where there is one; and why, where it
/// wasn't asked: its turn cancelled (`cancel`), or its time up (`timeout`).
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Who {
    pub(super) by: &'static str,
    pub(super) peer: Option<String>,
    pub(super) reason: Option<&'static str>,
}

impl Who {
    pub(super) const EDITOR: Who = Who { by: "editor", peer: None, reason: None };

    /// A bridge's or the CLI's answer, through connection `peer`.
    pub(super) fn peer(peer: Option<String>) -> Who {
        Who { by: "control", peer, reason: None }
    }

    /// brnr's own, for `reason`: `cancel` or `timeout`.
    pub(super) fn because(reason: &'static str) -> Who {
        Who { by: "control", peer: None, reason: Some(reason) }
    }

    /// Its fields, into `event`.
    pub(super) fn add_to(&self, event: &mut Value) {
        event["by"] = json!(self.by);
        event["peer"] = json!(self.peer);
        event["reason"] = json!(self.reason);
    }
}

/// A request ended, and what the host is to do for it, in this order.
pub(super) struct Resolved {
    pub(super) req: AgentRequest,
    /// The answer, ACP's: a permission request's outcome, an elicitation's
    /// result.
    pub(super) answer: Value,
    /// The host's reply to the agent; none for the editor's answer, which
    /// goes on as the editor wrote it.
    pub(super) reply: Option<Value>,
    /// The host answered one the editor was shown: it is withdrawn from the
    /// editor, told `how` and by whom (see experimental.rs).
    pub(super) withdraw: Option<(&'static str, Who)>,
    /// `permission_resolved` or `elicitation_resolved`; none for a request
    /// brnr doesn't hold.
    pub(super) event: Option<Value>,
}

/// What the editor's answer to one of the agent's requests is.
pub(super) enum EditorAnswer {
    /// The answer to one waiting: it goes on to the agent.
    Resolved(Box<Resolved>),
    /// One the host answered already: dropped, as the agent must not get a
    /// second answer.
    Late(Answered),
    /// Neither: an answer to a request the host couldn't read, or to none.
    /// It goes on, and the agent ignores what it never asked (ADR 2.5).
    Unknown,
}

/// What a request whose time is up comes to (ADR 4.5, 8.1).
pub(super) enum Timeout {
    Resolved(Box<Resolved>),
    /// A permission request without one `reject_once` option: its turn in
    /// `session` is cancelled, which answers it `cancelled` (see
    /// [`Approvals::cancel`]).
    Cancel {
        session: String,
    },
    /// It couldn't be answered, saying why.
    Failed(String),
}

/// A request the host answered while the editor may answer it too: what
/// the editor is told if it does.
pub(super) struct Answered {
    pub(super) session: Option<String>,
    /// The tool call's title, or the elicitation's message, as the request
    /// has it.
    pub(super) title: String,
    /// `allowed`, `rejected` or `cancelled`; `accepted` or `declined`.
    pub(super) how: &'static str,
    /// Who answered it.
    pub(super) by: Who,
}

/// What is kept of a URL elicitation held or accepted, until the agent says
/// its flow is complete.
struct UrlElicitation {
    session: Option<String>,
    handle: String,
}

/// Who answered a request, as [`Approvals::end`] takes it.
enum End {
    Host(Answer),
    /// The editor, with the `result` it sent.
    Editor(Value),
}

/// The agent's requests to its client that are unanswered, and what is
/// kept of those answered.
#[derive(Default)]
pub(super) struct Approvals {
    /// In the order the agent asked.
    requests: Vec<AgentRequest>,
    /// Requests the host answered that the editor was shown, by key, until
    /// the editor answers them too.
    answered: HashMap<String, Answered>,
    /// The last handle's number, of each kind.
    permissions: u64,
    elicitations: u64,
    /// URL elicitations held or accepted, by the agent's `elicitationId`.
    urls: HashMap<String, UrlElicitation>,
}

impl Approvals {
    /// The agent asked `req` of its client: kept until it is answered. One
    /// brnr holds gets its handle, and `deadline`, and its event comes back,
    /// `permission_request` or `elicitation_request` with `owner`.
    pub(super) fn ask(
        &mut self,
        mut req: AgentRequest,
        deadline: Option<Instant>,
        owner: &str,
    ) -> Option<Value> {
        match req.kind() {
            Some(Kind::Permission) => {
                self.permissions += 1;
                req.handle = Some(format!("p{}", self.permissions));
            }
            Some(Kind::Elicitation) => {
                self.elicitations += 1;
                req.handle = Some(format!("e{}", self.elicitations));
                self.remember_url(&req);
            }
            None => {}
        }
        req.deadline = deadline.filter(|_| req.handle.is_some());
        let event = request_event(&req, owner);
        self.requests.push(req);
        event
    }

    /// Keeps a URL elicitation's id, so the agent's `elicitation/complete`
    /// for it can be told to whom it belonged, up to [`URL_ELICITATIONS`].
    fn remember_url(&mut self, req: &AgentRequest) {
        if self.urls.len() >= URL_ELICITATIONS {
            return;
        }
        if let (Some(id), Some(handle)) = (elicitation::elicitation_id(req), req.held()) {
            let held = UrlElicitation { session: req.session.clone(), handle: handle.to_owned() };
            self.urls.insert(id, held);
        }
    }

    /// The agent's `elicitation/complete` of URL elicitation `id`: the
    /// session and the handle it belonged to, if kept.
    pub(super) fn complete(&mut self, id: &str) -> (Option<String>, Option<String>) {
        match self.urls.remove(id) {
            Some(url) => (url.session, Some(url.handle)),
            None => (None, None),
        }
    }

    /// The editor answered `key`, with `msg`.
    pub(super) fn editor_answer(&mut self, key: &str, msg: &Map<String, Value>) -> EditorAnswer {
        if let Some(pos) = self.requests.iter().position(|r| r.key == key) {
            let result = msg.get("result").cloned().unwrap_or(Value::Null);
            let resolved = self.end(pos, End::Editor(result), &Who::EDITOR, false);
            return EditorAnswer::Resolved(Box::new(resolved));
        }
        match self.answered.remove(key) {
            Some(answered) => EditorAnswer::Late(answered),
            None => EditorAnswer::Unknown,
        }
    }

    /// Answers request `handle` of `kind` in `session` (none: one outside
    /// any) with what `answer` makes of it, by `by`: a bridge or the CLI, or
    /// the timeout. Nothing is answered if `answer` fails.
    pub(super) fn answer(
        &mut self,
        kind: Kind,
        handle: &str,
        session: Option<&str>,
        by: &Who,
        client: Client,
        answer: impl FnOnce(&AgentRequest) -> Result<Answer, String>,
    ) -> Result<Resolved, String> {
        let pos = (self.requests.iter())
            .position(|r| {
                r.kind() == Some(kind)
                    && r.held() == Some(handle)
                    && r.session.as_deref() == session
            })
            .ok_or_else(|| {
                let what = if kind == Kind::Permission { "request" } else { "elicitation" };
                match session {
                    Some(session) => format!("no pending {what} {handle} in session {session}"),
                    None => format!("no pending {what} {handle} outside a session"),
                }
            })?;
        if !client.accepting {
            return Err(NOT_ACCEPTING.to_owned());
        }
        let answer = answer(&self.requests[pos])?;
        Ok(self.end(pos, End::Host(answer), by, client.editor))
    }

    /// ACP: a client that cancels a turn answers that session's pending
    /// permission requests `cancelled`; its elicitations are answered
    /// `cancel` the same way (ADR 4.5), the turn they were asked in gone.
    /// `by` is the cancel's or the timeout's.
    pub(super) fn cancel(&mut self, session: &str, by: &Who, client: Client) -> Vec<Resolved> {
        let mut ended = Vec::new();
        while let Some(pos) = (self.requests.iter())
            .position(|r| r.handle.is_some() && r.session.as_deref() == Some(session))
        {
            let answer = if self.requests[pos].is_elicitation() {
                Answer::Result(json!({ "action": "cancel" }))
            } else {
                Answer::Outcome(json!({ "outcome": "cancelled" }), "cancelled")
            };
            ended.push(self.end(pos, End::Host(answer), by, client.editor));
        }
        ended
    }

    /// The handles of requests whose time is up at `now`, for
    /// [`Self::time_out`] to answer in turn.
    pub(super) fn expired(&self, now: Instant) -> Vec<String> {
        (self.requests.iter())
            .filter(|r| r.deadline.is_some_and(|d| now >= d))
            .filter_map(|r| r.handle.clone())
            .collect()
    }

    /// Answers `handle`, whose time is up, as ADR 4.5 has it: a permission
    /// request with its `reject_once` option, or, without one (or with two:
    /// which, brnr can't say), with its turn cancelled; an elicitation
    /// `cancel`, which ACP has for no choice made, never `decline`, the
    /// user's no. Its time is up once, whatever comes of it. None if it is
    /// gone already: answered `cancelled` with an earlier one's turn.
    pub(super) fn time_out(&mut self, handle: &str, client: Client) -> Option<Timeout> {
        let req = self.requests.iter_mut().find(|r| r.held() == Some(handle))?;
        req.deadline = None;
        let (kind, session) = (req.kind()?, req.session.clone());
        let options = req.params["options"].as_array().into_iter().flatten();
        let once = options.filter(|o| o["kind"] == "reject_once").count();
        let by = &Who::because("timeout");
        let resolved = match kind {
            Kind::Elicitation => self.answer(kind, handle, session.as_deref(), by, client, |r| {
                elicitation::response(r, "cancel", &Value::Null).map(Answer::Result)
            }),
            Kind::Permission if once == 1 => {
                self.answer(kind, handle, session.as_deref(), by, client, |r| {
                    choose(r, Choice::Reject, false, None)
                })
            }
            _ if !client.accepting => Err(NOT_ACCEPTING.to_owned()),
            _ => match session {
                Some(session) => return Some(Timeout::Cancel { session }),
                None => Err("no reject_once option, and no session to cancel".to_owned()),
            },
        };
        Some(resolved.map_or_else(Timeout::Failed, |r| Timeout::Resolved(Box::new(r))))
    }

    /// The one way a request ends: gone from the table, its URL elicitation
    /// with it unless accepted, and if the host answered one the editor was
    /// shown, kept as answered, for the editor's late answer.
    fn end(&mut self, pos: usize, end: End, by: &Who, editor: bool) -> Resolved {
        let req = self.requests.remove(pos);
        let (answer, reply, how) = match end {
            End::Editor(result) if req.is_permission() => (result["outcome"].clone(), None, None),
            End::Editor(result) => (result, None, None),
            End::Host(Answer::Outcome(outcome, how)) => {
                let reply = json!({ "result": { "outcome": outcome } });
                (outcome, Some(reply), Some(how))
            }
            End::Host(Answer::Result(result)) => {
                let how = match result["action"].as_str() {
                    Some("accept") => "accepted",
                    Some("decline") => "declined",
                    _ => "cancelled",
                };
                (result.clone(), Some(json!({ "result": result })), Some(how))
            }
        };
        if req.is_elicitation()
            && answer["action"] != "accept"
            && let Some(id) = elicitation::elicitation_id(&req)
        {
            self.urls.remove(&id);
        }
        let withdraw = how.filter(|_| editor).map(|how| {
            let answered =
                Answered { session: req.session.clone(), title: req.title(), how, by: by.clone() };
            self.answered.insert(req.key.clone(), answered);
            (how, by.clone())
        });
        let event = match req.kind() {
            Some(Kind::Permission) => Some(resolved_event(&req, &answer, how, by)),
            Some(Kind::Elicitation) => Some(elicitation::resolved_event(&req, &answer, by)),
            None => None,
        };
        Resolved { req, answer, reply, withdraw, event }
    }

    /// The requests brnr holds, in the order the agent asked.
    pub(super) fn held(&self) -> impl Iterator<Item = &AgentRequest> {
        self.requests.iter().filter(|r| r.handle.is_some())
    }

    /// Whether a request brnr holds waits in `session`.
    pub(super) fn waiting(&self, session: &str) -> bool {
        self.held().any(|r| r.session.as_deref() == Some(session))
    }

    /// The earliest deadline, for the event loop's wake-up.
    pub(super) fn next_deadline(&self) -> Option<Instant> {
        self.requests.iter().filter_map(|r| r.deadline).min()
    }

    /// How deeply what it keeps of the agent's requests nests (see
    /// `Host::holds`).
    pub(super) fn holds(&self) -> usize {
        let Approvals { requests, answered: _, permissions: _, elicitations: _, urls: _ } = self;
        requests.iter().map(AgentRequest::holds).max().unwrap_or(0)
    }
}

/// The answer to permission request `req` with the option of one kind
/// (ADR 8.1): `allow_once`, `allow_always` with `always`, `reject_once` or
/// `reject_always`. No other kind stands in for it, and a reject is never
/// `cancelled`: without that kind, or with two options of it, it fails,
/// listing the options (P4). `option` names the option instead: one of a
/// kind ACP has must be on the choice's side, and with `always` the always
/// kind; one of a kind brnr doesn't know goes with either.
pub(super) fn choose(
    req: &AgentRequest,
    choice: Choice,
    always: bool,
    option: Option<&str>,
) -> Result<Answer, String> {
    let handle = req.held().unwrap_or("?");
    let options = req.params["options"].as_array().map_or(&[][..], Vec::as_slice);
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
    Ok(Answer::Outcome(outcome, if allow { "allowed" } else { "rejected" }))
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
fn resolved_event(req: &AgentRequest, outcome: &Value, how: Option<&str>, by: &Who) -> Value {
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
    let mut event = json!({
        "event": "permission_resolved",
        "session": req.session,
        "request": req.handle,
        "answer": answer,
        "option_kind": option_kind,
        "outcome": outcome,
    });
    by.add_to(&mut event);
    event
}

/// The event for a request brnr holds, `permission_request` or
/// `elicitation_request`; none for one it doesn't.
fn request_event(req: &AgentRequest, owner: &str) -> Option<Value> {
    match req.kind()? {
        Kind::Permission => {
            let tool = &req.params["toolCall"];
            Some(json!({
                "event": "permission_request",
                "session": req.session,
                "request": req.handle,
                "owner": owner,
                "title": tool["title"],
                "kind": tool["kind"],
                "tool_call": tool,
                "options": req.params["options"],
            }))
        }
        Kind::Elicitation => Some(elicitation::request_event(req, owner)),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    const HEADLESS: Client = Client { editor: false, accepting: true };
    const EDITOR: Client = Client { editor: true, accepting: true };

    fn permission(id: u64, session: &str, kinds: &[&str]) -> AgentRequest {
        let options: Vec<Value> =
            kinds.iter().map(|k| json!({ "optionId": k, "name": k, "kind": k })).collect();
        let params = json!({
            "sessionId": session,
            "toolCall": { "toolCallId": "t1", "title": "Edit" },
            "options": options,
        });
        AgentRequest::new(
            json!(id),
            "session/request_permission".into(),
            Some(session.into()),
            params,
        )
    }

    fn url(id: u64, session: &str, elicitation: &str) -> AgentRequest {
        let params = json!({
            "sessionId": session, "mode": "url", "message": "Log in",
            "url": "https://example.com", "elicitationId": elicitation,
        });
        AgentRequest::new(json!(id), "elicitation/create".into(), Some(session.into()), params)
    }

    fn b1() -> Who {
        Who::peer(Some("b1".into()))
    }

    const CANCEL: &Who = &Who { by: "control", peer: None, reason: Some("cancel") };

    fn handles(t: &Approvals) -> Vec<&str> {
        t.held().filter_map(AgentRequest::held).collect()
    }

    fn allow(t: &mut Approvals, handle: &str, client: Client) -> Result<Resolved, String> {
        t.answer(Kind::Permission, handle, Some("s"), &b1(), client, |r| {
            choose(r, Choice::Allow, false, None)
        })
    }

    fn elicit(t: &mut Approvals, handle: &str, action: &str) -> Result<Resolved, String> {
        t.answer(Kind::Elicitation, handle, Some("s"), &b1(), HEADLESS, |r| {
            elicitation::response(r, action, &Value::Null).map(Answer::Result)
        })
    }

    fn editor(t: &mut Approvals, id: u64, result: Value) -> EditorAnswer {
        let msg = json!({ "jsonrpc": "2.0", "id": id, "result": result });
        t.editor_answer(&id_key(&json!(id)), msg.as_object().unwrap())
    }

    #[test]
    fn adr_0004_handles_are_never_used_again() {
        let mut t = Approvals::default();
        t.ask(permission(1, "s", &["allow_once"]), None, "headless");
        t.ask(url(2, "s", "u1"), None, "headless");
        let other = AgentRequest::new(json!(3), "fs/read_text_file".into(), None, Value::Null);
        assert!(t.ask(other, None, "editor").is_none(), "no event for one brnr doesn't hold");
        assert_eq!(handles(&t), ["p1", "e1"]);
        allow(&mut t, "p1", HEADLESS).unwrap();
        t.cancel("s", CANCEL, HEADLESS);
        assert!(handles(&t).is_empty());
        let event = t.ask(permission(4, "s", &["allow_once"]), None, "headless").unwrap();
        assert_eq!(event["request"], "p2");
        t.ask(url(5, "s", "u2"), None, "headless");
        assert_eq!(handles(&t), ["p2", "e2"]);
    }

    #[test]
    fn adr_0004_every_way_a_request_ends_leaves_nothing_behind() {
        type Ending = fn(&mut Approvals);
        let endings: [(&str, Ending, bool); 7] = [
            ("accepted", |t| drop(elicit(t, "e1", "accept").unwrap()), true),
            ("declined", |t| drop(elicit(t, "e1", "decline").unwrap()), false),
            ("cancelled", |t| drop(elicit(t, "e1", "cancel").unwrap()), false),
            ("by the turn", |t| drop(t.cancel("s", CANCEL, HEADLESS)), false),
            ("timed out", |t| drop(t.time_out("e1", HEADLESS).unwrap()), false),
            ("editor accepted", |t| drop(editor(t, 1, json!({ "action": "accept" }))), true),
            ("editor declined", |t| drop(editor(t, 1, json!({ "action": "decline" }))), false),
        ];
        for (how, end, kept) in endings {
            let mut t = Approvals::default();
            t.ask(url(1, "s", "u1"), None, "headless");
            end(&mut t);
            assert!(handles(&t).is_empty() && !t.waiting("s"), "{how}: still held");
            // Accepted, the URL's flow goes on until the agent completes it.
            assert_eq!(t.urls.contains_key("u1"), kept, "{how}");
            let completed = t.complete("u1");
            assert_eq!(completed.1.is_some(), kept, "{how}");
            assert!(t.urls.is_empty(), "{how}: kept after its completion");
        }
        // A permission request, by each who can answer it.
        let mut t = Approvals::default();
        for (id, session) in [(1, "s"), (2, "s"), (3, "s"), (4, "other")] {
            t.ask(permission(id, session, &["allow_once", "reject_once"]), None, "editor");
        }
        let cancelled = json!({ "outcome": { "outcome": "cancelled" } });
        let EditorAnswer::Resolved(r) = editor(&mut t, 1, cancelled) else { panic!() };
        // It goes on to the agent as the editor wrote it.
        assert!(r.reply.is_none() && r.withdraw.is_none());
        assert_eq!(r.event.unwrap()["answer"], "cancelled");
        allow(&mut t, "p2", EDITOR).unwrap();
        assert_eq!(t.cancel("s", CANCEL, EDITOR).len(), 1);
        assert_eq!(handles(&t), ["p4"], "a cancel ends its own session's only");
        assert!(matches!(t.time_out("p4", HEADLESS), Some(Timeout::Resolved(_))));
        assert!(handles(&t).is_empty());
        assert_eq!(t.next_deadline(), None);
    }

    #[test]
    fn adr_0002_a_late_answer_from_the_editor_is_dropped() {
        let mut t = Approvals::default();
        t.ask(permission(1, "s", &["allow_once"]), None, "editor");
        t.ask(url(2, "s", "u1"), None, "editor");
        let r = allow(&mut t, "p1", EDITOR).unwrap();
        assert_eq!(r.withdraw, Some(("allowed", b1())));
        assert_eq!(r.reply.unwrap()["result"]["outcome"]["optionId"], "allow_once");
        let r = t.cancel("s", CANCEL, EDITOR).pop().unwrap();
        assert_eq!(r.withdraw, Some(("cancelled", CANCEL.clone())));
        assert_eq!(r.event.unwrap()["action"], "cancel");
        for (id, how, by) in [(1, "allowed", b1()), (2, "cancelled", CANCEL.clone())] {
            let EditorAnswer::Late(answered) = editor(&mut t, id, json!({})) else {
                panic!("{id}: not dropped")
            };
            assert_eq!((answered.how, answered.by), (how, by));
            // Once: what the editor sends after that goes on.
            assert!(matches!(editor(&mut t, id, json!({})), EditorAnswer::Unknown));
        }
        // Headless, nobody else was shown it: nothing is kept.
        t.ask(permission(3, "s", &["allow_once"]), None, "headless");
        assert!(allow(&mut t, "p2", HEADLESS).unwrap().withdraw.is_none());
        assert!(matches!(editor(&mut t, 3, json!({})), EditorAnswer::Unknown));
    }

    #[test]
    fn adr_0004_an_answer_that_cant_be_sent_leaves_the_request_waiting() {
        let mut t = Approvals::default();
        t.ask(permission(1, "s", &["allow_once", "allow_always"]), None, "headless");
        let gone = Client { editor: false, accepting: false };
        assert_eq!(
            allow(&mut t, "p1", gone).err().unwrap(),
            "the agent is no longer accepting input"
        );
        let other = t.answer(Kind::Permission, "p1", Some("t"), &b1(), HEADLESS, |r| {
            choose(r, Choice::Allow, false, None)
        });
        assert_eq!(other.err().unwrap(), "no pending request p1 in session t");
        let e = elicit(&mut t, "p1", "cancel").err().unwrap();
        assert_eq!(e, "no pending elicitation p1 in session s");
        let reject = t.answer(Kind::Permission, "p1", Some("s"), &b1(), HEADLESS, |r| {
            choose(r, Choice::Reject, false, None)
        });
        assert!(reject.err().unwrap().starts_with("p1 has no reject_once option"));
        assert_eq!(handles(&t), ["p1"]);
    }

    #[test]
    fn adr_0004_a_timeout_rejects_once_or_cancels_the_turn() {
        let mut t = Approvals::default();
        let now = Instant::now();
        t.ask(permission(1, "s", &["allow_once", "reject_once"]), Some(now), "headless");
        t.ask(permission(2, "s", &["allow_once", "reject_always"]), Some(now), "headless");
        t.ask(url(3, "s", "u1"), Some(now + Duration::from_secs(9)), "headless");
        assert_eq!(t.expired(now), ["p1", "p2"]);
        let Some(Timeout::Resolved(r)) = t.time_out("p1", HEADLESS) else { panic!() };
        assert_eq!(r.event.unwrap()["answer"], "rejected");
        assert!(
            matches!(t.time_out("p2", HEADLESS), Some(Timeout::Cancel { session }) if session == "s")
        );
        // Its time is up once: the cancel answers it.
        assert_eq!(t.expired(now), Vec::<String>::new());
        assert_eq!(t.cancel("s", &Who::because("timeout"), HEADLESS).len(), 2);
        assert!(t.time_out("p2", HEADLESS).is_none(), "gone with its turn");
        let gone = Client { editor: false, accepting: false };
        t.ask(permission(4, "s", &["allow_once"]), Some(now), "headless");
        assert!(matches!(t.time_out("p3", gone), Some(Timeout::Failed(_))));
        assert_eq!(t.next_deadline(), None);
    }
}
