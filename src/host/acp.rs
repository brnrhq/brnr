//! The ACP stream: sessions, turns, injection, and the host as the agent's
//! client when no editor is attached.
//!
//! The host reads ACP line by line from both sides. It changes the stream
//! in four ways; everything else passes through unchanged:
//!
//! - The editor's `initialize` loses the `fs` and `terminal` client
//!   capabilities ([`DROPPED_CAPABILITIES`]).
//! - An injected prompt goes to the agent as a `session/prompt` with a host
//!   id (`brnr-<n>`). Its response is kept from the editor, and the
//!   editor is shown the text as a `user_message_chunk` as it is sent. ACP
//!   says nothing about when an agent takes up a prompt sent mid-turn
//!   (claude-agent-acp folds it into the running turn at its next step), so
//!   the moment it is sent is the only point the host can show.
//! - Held context is appended to the next `session/prompt`, whoever sends
//!   it.
//! - With no editor attached, the host answers what the agent asks of its
//!   client. Permission requests follow the `permissions` policy, and with
//!   `ask` they wait for an approve or deny from a bridge. Elicitation is
//!   declined, and anything else gets "method not found".

use std::collections::VecDeque;
use std::io::Write;
use std::mem::take;
use std::path::PathBuf;

use serde_json::{Map, Value, json};

use super::{Host, Permissions, id_key, text_block};
use crate::frame;
use crate::log::Dir;

/// Client capabilities removed from the editor's `initialize`. ACP v2 drops
/// them, so the host won't implement them for when the editor is gone, and
/// the agent must not come to rely on them while the editor is there.
const DROPPED_CAPABILITIES: &[&str] = &["fs", "terminal"];

/// Session updates that end the agent message being assembled for the
/// `agent_message` event. Bookkeeping updates (usage, commands, mode) don't.
const ENDS_AGENT_MESSAGE: &[&str] =
    &["user_message_chunk", "agent_thought_chunk", "tool_call", "tool_call_update", "plan"];

/// A `session/prompt` the agent hasn't answered yet.
pub(super) struct Prompt {
    id: String,
    injected: bool,
}

pub(super) struct Session {
    pub(super) id: String,
    pub(super) cwd: PathBuf,
    /// Prompts sent and not yet answered, in the order they were sent.
    /// Non-empty means a turn is running.
    pub(super) prompts: VecDeque<Prompt>,
    /// Injected messages waiting for the session to go idle.
    pub(super) held: VecDeque<String>,
    /// How many of `held`, from the front, are interrupts: a new interrupt
    /// goes after them, so interrupts keep the order they were sent in.
    pub(super) interrupts: usize,
    /// Context to append to the next prompt.
    pub(super) context: Vec<String>,
    /// The agent message so far, for the `agent_message` event.
    agent_text: String,
    agent_message_id: Option<Value>,
}

/// A request whose response creates or ends a session.
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
}

/// What the host asked the agent, as its client, when started headless.
pub(super) enum HostRequest {
    Initialize,
    NewSession,
}

/// A request from the agent to its client.
pub(super) struct AgentRequest {
    key: String,
    id: Value,
    method: String,
    pub(super) session: Option<String>,
    pub(super) params: Value,
    /// `p<n>` for a permission request: what bridges and brnr call it.
    pub(super) handle: Option<String>,
}

#[derive(Clone, Copy)]
pub(super) enum Choice {
    Allow,
    Deny,
}

impl Host {
    // ---- editor → agent ------------------------------------------------

    pub(super) fn editor_bytes(&mut self, bytes: &[u8]) {
        self.editor_buf.extend_from_slice(bytes);
        while let Some(i) = self.editor_buf.iter().position(|&b| b == b'\n') {
            let mut line: Vec<u8> = self.editor_buf.drain(..=i).collect();
            line.pop();
            self.editor_line(line);
        }
    }

    fn editor_line(&mut self, mut line: Vec<u8>) {
        let mut session = None;
        let mut deferred = None;
        if let Ok(Value::Object(mut msg)) = serde_json::from_slice::<Value>(&line) {
            session = param_session(&msg);
            let method = msg.get("method").and_then(Value::as_str).map(str::to_owned);
            match (method, msg.get("id").cloned()) {
                (Some(method), Some(id)) => {
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
                    if !self.agent_requests.iter().any(|r| r.key == key) {
                        // Already answered by the host (a cancel), or never
                        // asked: the agent must not get a second answer.
                        let event = json!({ "event": "editor-response-dropped", "id": id });
                        self.sink.note(session.as_deref(), event);
                        return;
                    }
                    session = self.agent_request_answered(&key, &msg, "editor");
                }
                _ => {}
            }
        }
        line.push(b'\n');
        // A session/new goes in the host log now, so one the agent never
        // answers is still on record, and in the session's file once the
        // response names it.
        let session = if deferred.is_some() { None } else { session };
        self.record(session.as_deref(), Dir::EditorToAgent, &line);
        if let Some(Pending::New { request, .. }) =
            deferred.and_then(|key| self.pending.get_mut(&key))
        {
            *request = Some(line.clone());
        }
        self.write_agent(&line);
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
            ("initialize", _) => return self.drop_capabilities(msg),
            ("session/new", _) => {
                let pending = Pending::New { cwd, request: None, dir: Dir::EditorToAgent };
                self.pending.insert(key.to_owned(), pending);
            }
            ("session/load" | "session/resume", Some(session)) => {
                self.pending
                    .insert(key.to_owned(), Pending::Attach { session: session.clone(), cwd });
            }
            ("session/close", Some(session)) => {
                self.pending.insert(key.to_owned(), Pending::Close { session: session.clone() });
            }
            ("session/prompt", Some(session)) => return self.editor_prompt(session, key, msg),
            _ => {}
        }
        None
    }

    fn drop_capabilities(&mut self, msg: &mut Map<String, Value>) -> Option<Vec<u8>> {
        let caps = msg.get_mut("params")?.get_mut("clientCapabilities")?.as_object_mut()?;
        let dropped: Map<String, Value> = DROPPED_CAPABILITIES
            .iter()
            .filter_map(|&k| Some((k.to_owned(), caps.remove(k)?)))
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
        self.sessions[i].prompts.push_back(Prompt { id: key.to_owned(), injected: false });
        self.prompt_session.insert(key.to_owned(), session.to_owned());
        let rewritten = self.attach_context(i, session, msg);
        let text = prompt_text(msg.get("params").and_then(|p| p.get("prompt")));
        self.flush_agent_message(i);
        self.emit(
            json!({ "event": "user_message", "session": session, "by": "editor", "text": text }),
        );
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
        blocks.extend(context.iter().map(|t| text_block(t)));
        let event =
            json!({ "event": "context-attached", "to": "editor-prompt", "count": context.len() });
        self.sink.note(Some(session), event);
        for text in &context {
            self.echo(session, text);
        }
        Some(serde_json::to_vec(&msg).unwrap())
    }

    // ---- agent → editor ------------------------------------------------

    pub(super) fn agent_line(&mut self, line: &[u8]) {
        let body = line.strip_suffix(b"\n").unwrap_or(line);
        let Ok(Value::Object(msg)) = serde_json::from_slice::<Value>(body) else {
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
            let mut injected = false;
            if let Some(i) = self.find(&sid) {
                let prompts = &mut self.sessions[i].prompts;
                if let Some(pos) = prompts.iter().position(|p| p.id == key) {
                    injected = prompts.remove(pos).unwrap().injected;
                }
            }
            session = Some(sid.clone());
            turn = Some((sid, injected));
        }
        let forward = ours.is_none() && !turn.as_ref().is_some_and(|(_, injected)| *injected);
        let dir = if forward { Dir::AgentToEditor } else { Dir::AgentToControl };
        self.record(session.as_deref(), dir, line);
        if forward {
            self.send_link(frame::DATA, line);
        }
        if let Some(request) = ours {
            self.host_request_done(request, msg);
        }
        if let Some((sid, injected)) = turn {
            if let Some(i) = self.find(&sid) {
                self.flush_agent_message(i);
            }
            self.emit(json!({
                "event": "turn_ended",
                "session": sid,
                "by": if injected { "control" } else { "editor" },
                "stop_reason": msg.get("result").and_then(|r| r.get("stopReason")),
                "error": msg.get("error"),
            }));
            self.next_turn(&sid);
        }
    }

    /// Returns the session the response was about.
    fn session_changed(&mut self, pending: Pending, result: Option<&Value>) -> Option<String> {
        match pending {
            Pending::New { cwd, request, dir } => {
                let session = result.and_then(|r| r["sessionId"].as_str()).map(str::to_owned);
                if let Some(session) = &session {
                    self.open_session(session, cwd.as_deref());
                }
                if let Some(request) = request {
                    self.sink.msg(session.as_deref(), dir, &request);
                }
                session
            }
            Pending::Attach { session, cwd } => {
                if result.is_some() {
                    self.open_session(&session, cwd.as_deref());
                }
                Some(session)
            }
            Pending::Close { session } => {
                if result.is_some()
                    && let Some(i) = self.find(&session)
                {
                    self.flush_agent_message(i);
                    self.sessions.remove(i);
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

    /// The editor has gone: whatever it left unanswered is now the host's.
    pub(super) fn take_over_agent_requests(&mut self) {
        for req in take(&mut self.agent_requests) {
            self.answer_as_client(req);
        }
    }

    fn answer_as_client(&mut self, req: AgentRequest) {
        match req.method.as_str() {
            "session/request_permission" => {
                let handle = req.handle.clone().unwrap();
                let event = permission_event(&req, "host");
                self.agent_requests.push(req);
                self.emit(event);
                let policy = match self.permissions {
                    Permissions::Ask => return,
                    Permissions::AutoAllow => Choice::Allow,
                    Permissions::AutoDeny => Choice::Deny,
                };
                if let Err(err) = self.resolve_permission(&handle, policy, None, "policy") {
                    self.sink.note(
                        None,
                        json!({ "event": "policy-failed", "request": handle, "error": err }),
                    );
                }
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

    /// Answers permission request `handle`: with `option` if given, else the
    /// first allow (or reject) option. Denying a request that offers no
    /// reject option cancels it.
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
        if self.editor_attached() {
            return Err("the editor owns this session; answer it there".into());
        }
        if self.status.is_some() || self.agent_in.is_none() {
            return Err("the agent is no longer accepting input".into());
        }
        let options =
            self.agent_requests[pos].params["options"].as_array().cloned().unwrap_or_default();
        let outcome = match option {
            Some(option) => {
                if !options.iter().any(|o| o["optionId"] == option) {
                    let ids: Vec<&str> =
                        options.iter().filter_map(|o| o["optionId"].as_str()).collect();
                    return Err(format!(
                        "{handle} has no option {option} (options: {})",
                        ids.join(", ")
                    ));
                }
                json!({ "outcome": "selected", "optionId": option })
            }
            None => {
                let kinds = match choice {
                    Choice::Allow => ["allow_once", "allow_always"],
                    Choice::Deny => ["reject_once", "reject_always"],
                };
                match kinds.iter().find_map(|k| options.iter().find(|o| o["kind"] == *k)) {
                    Some(o) => json!({ "outcome": "selected", "optionId": o["optionId"] }),
                    None if matches!(choice, Choice::Deny) => json!({ "outcome": "cancelled" }),
                    None => return Err(format!("{handle} offers no allow option")),
                }
            }
        };
        let req = self.agent_requests.remove(pos);
        self.respond(&req, json!({ "result": { "outcome": outcome } }));
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
    /// its late answer is dropped (see `editor_line`).
    fn cancel_permissions(&mut self, session: &str) {
        let (cancel, keep): (Vec<_>, Vec<_>) = take(&mut self.agent_requests)
            .into_iter()
            .partition(|r| r.handle.is_some() && r.session.as_deref() == Some(session));
        self.agent_requests = keep;
        for req in cancel {
            self.respond(&req, json!({ "result": { "outcome": { "outcome": "cancelled" } } }));
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

    // ---- headless start ------------------------------------------------

    /// Started with no editor (`brnr start`, `brnr host`): the host opens the
    /// session itself.
    pub(super) fn begin_headless_start(&mut self) {
        let reason =
            json!({ "event": "owner-changed", "owner": "host", "reason": "started headless" });
        self.sink.note(None, reason);
        let params = json!({ "protocolVersion": 1, "clientCapabilities": {} });
        self.host_request("initialize", params, HostRequest::Initialize);
    }

    fn host_request(&mut self, method: &str, params: Value, kind: HostRequest) {
        self.next_id += 1;
        let id = Value::String(format!("brnr-{}", self.next_id));
        let key = id_key(&id);
        let cwd = params["cwd"].as_str().map(str::to_owned);
        let msg = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        let mut line = serde_json::to_vec(&msg).unwrap();
        line.push(b'\n');
        self.client_requests.insert(key.clone(), None);
        self.host_requests.insert(key.clone(), kind);
        self.record(None, Dir::ControlToAgent, &line);
        if method == "session/new" {
            let pending =
                Pending::New { cwd, request: Some(line.clone()), dir: Dir::ControlToAgent };
            self.pending.insert(key, pending);
        }
        self.write_agent(&line);
    }

    fn host_request_done(&mut self, request: HostRequest, msg: &Map<String, Value>) {
        if let Some(error) = msg.get("error") {
            let what = match request {
                HostRequest::Initialize => "initialize",
                HostRequest::NewSession => "session/new",
            };
            let message =
                error["message"].as_str().map_or_else(|| error.to_string(), str::to_owned);
            return self.fail_start(&format!("{what} failed: {message}"));
        }
        match request {
            HostRequest::Initialize => {
                let params = json!({ "cwd": self.cwd.to_string_lossy(), "mcpServers": [] });
                self.host_request("session/new", params, HostRequest::NewSession);
            }
            HostRequest::NewSession => {
                let Some(session) = msg["result"]["sessionId"].as_str().map(str::to_owned) else {
                    return self.fail_start("session/new returned no sessionId");
                };
                if self.stop_requested {
                    return; // The start already failed (timed out) or was stopped.
                }
                let i = self.open_session(&session, None);
                // brnr start hears of the session before the agent gets any
                // work: if it has gone, nobody knows this session exists.
                let ready = json!({
                    "ok": true,
                    "id": self.info["id"],
                    "host_id": self.host_id,
                    "session": session,
                });
                if let Some(mut ready_fd) = self.ready.take() {
                    self.start_deadline = None;
                    if writeln!(ready_fd, "{ready}").is_err() {
                        self.sink.note(None, json!({ "event": "start-abandoned" }));
                        return self.begin_stop();
                    }
                }
                if let Some(prompt) = self.first_prompt.take() {
                    self.send_prompt(i, prompt);
                }
                if self.manual {
                    eprintln!("brnr host: session {session}");
                }
            }
        }
    }

    pub(super) fn fail_start(&mut self, error: &str) {
        self.sink.note(None, json!({ "event": "start-failed", "error": error }));
        self.startup_failed(error);
        self.begin_stop();
    }

    /// Reports `error` to brnr start if it is still waiting.
    pub(super) fn startup_failed(&mut self, error: &str) {
        if self.manual && self.sessions.is_empty() && !self.startup_reported {
            self.startup_reported = true;
            eprintln!("brnr host: {error}");
        }
        if let Some(mut ready) = self.ready.take() {
            let _ = writeln!(ready, "{}", json!({ "ok": false, "error": error }));
        }
    }

    // ---- turns and injection -------------------------------------------

    /// Collects agent message text for the `agent_message` event.
    fn track_update(&mut self, session: &str, update: &Value) {
        let Some(i) = self.find(session) else { return };
        let kind = update["sessionUpdate"].as_str().unwrap_or_default();
        if kind == "agent_message_chunk" {
            let id = update.get("messageId").cloned();
            if self.sessions[i].agent_message_id != id {
                self.flush_agent_message(i);
            }
            let s = &mut self.sessions[i];
            s.agent_message_id = id;
            if let Some(text) = update["content"]["text"].as_str() {
                s.agent_text.push_str(text);
            }
        } else if ENDS_AGENT_MESSAGE.contains(&kind) {
            self.flush_agent_message(i);
        }
    }

    pub(super) fn flush_agent_message(&mut self, i: usize) {
        let text = take(&mut self.sessions[i].agent_text);
        if !text.is_empty() {
            let session = self.sessions[i].id.clone();
            self.emit(json!({ "event": "agent_message", "session": session, "text": text }));
        }
    }

    /// After a prompt is answered: once the session is idle, send the next
    /// held message.
    fn next_turn(&mut self, session: &str) {
        let Some(i) = self.find(session) else { return };
        let s = &mut self.sessions[i];
        if s.prompts.is_empty()
            && let Some(text) = s.held.pop_front()
        {
            s.interrupts = s.interrupts.saturating_sub(1);
            self.send_prompt(i, text);
        }
    }

    pub(super) fn send_prompt(&mut self, i: usize, text: String) {
        self.next_id += 1;
        let id = Value::String(format!("brnr-{}", self.next_id));
        let key = id_key(&id);
        let session = self.sessions[i].id.clone();
        let mut texts = vec![text];
        texts.extend(take(&mut self.sessions[i].context));
        let msg = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "session/prompt",
            "params": {
                "sessionId": session,
                "prompt": texts.iter().map(|t| text_block(t)).collect::<Vec<_>>(),
            },
        });
        self.sessions[i].prompts.push_back(Prompt { id: key.clone(), injected: true });
        self.prompt_session.insert(key.clone(), session.clone());
        self.client_requests.insert(key, Some(session.clone()));
        for text in &texts {
            self.echo(&session, text);
        }
        let event = json!({ "event": "user_message", "session": session, "by": "control", "text": texts.join("\n") });
        self.emit(event);
        let mut line = serde_json::to_vec(&msg).unwrap();
        line.push(b'\n');
        self.record(Some(&session), Dir::ControlToAgent, &line);
        self.write_agent(&line);
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

    /// Shows the editor an injected message as the user's.
    fn echo(&mut self, session: &str, text: &str) {
        if let Some(i) = self.find(session) {
            self.flush_agent_message(i);
        }
        let msg = json!({
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": {
                "sessionId": session,
                "update": { "sessionUpdate": "user_message_chunk", "content": text_block(text) },
            },
        });
        let mut line = serde_json::to_vec(&msg).unwrap();
        line.push(b'\n');
        if self.editor_attached() {
            self.record(Some(session), Dir::ControlToEditor, &line);
            self.send_link(frame::DATA, &line);
        }
    }

    // ---- sessions ------------------------------------------------------

    pub(super) fn find(&self, session: &str) -> Option<usize> {
        self.sessions.iter().position(|s| s.id == session)
    }

    /// Index of `session`, started (with its log file) if this is the first
    /// we hear of it. Without a cwd, the host's own is used.
    pub(super) fn open_session(&mut self, session: &str, cwd: Option<&str>) -> usize {
        if let Some(i) = self.find(session) {
            return i;
        }
        let cwd = cwd.map(PathBuf::from).unwrap_or_else(|| self.cwd.clone());
        self.sink.open_session(session, &cwd);
        self.sessions.push(Session {
            id: session.to_owned(),
            cwd,
            prompts: VecDeque::new(),
            held: VecDeque::new(),
            context: Vec::new(),
            interrupts: 0,
            agent_text: String::new(),
            agent_message_id: None,
        });
        self.sessions.len() - 1
    }
}

/// A prompt's content as text: its text blocks, with other blocks shown as
/// `[image]`, `[resource <uri>]` and so on.
fn prompt_text(blocks: Option<&Value>) -> String {
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

fn param_session(msg: &Map<String, Value>) -> Option<String> {
    msg.get("params")?.get("sessionId")?.as_str().map(str::to_owned)
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
