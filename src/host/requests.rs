//! Requests the host sends the agent as its client:
//!
//! - opening the session of a headless start: `initialize`, then
//!   `session/new`, `session/resume` or `session/load`, then the mode and
//!   config options the start asked for, before the first prompt;
//! - what bridges ask of the agent through the host: set the mode, a config
//!   option or the model, list the agent's sessions, fork or close one. The
//!   bridge gets its answer when the agent's arrives.

use std::collections::VecDeque;
use std::io::Write;
use std::path::PathBuf;

use serde_json::{Map, Value, json};

use super::acp::Held;
use super::{Host, id_key};
use crate::log::Dir;

pub(super) enum HostRequest {
    Initialize,
    Open(Open),
    Setup(SetupStep),
    Peer { peer: u64, req_id: Option<Value>, op: PeerOp },
}

/// How a headless start gets its session.
pub(super) enum Open {
    New,
    Resume(String),
    Load(String),
}

/// What a headless start applies before the first prompt.
#[derive(Clone)]
pub(super) enum SetupStep {
    Mode(String),
    Config(String, String),
}

impl SetupStep {
    fn describe(&self) -> String {
        match self {
            SetupStep::Mode(mode) => format!("setting mode {mode}"),
            SetupStep::Config(id, value) => format!("setting {id}={value}"),
        }
    }
}

/// What a bridge asked the agent for.
pub(super) enum PeerOp {
    Mode { session: String, mode: String },
    Config { session: String },
    Model { session: String, model: String },
    List,
    Fork { cwd: PathBuf },
    Close { session: String },
}

impl Host {
    /// Started with no editor (`brnr start`, `brnr host`): the host opens the
    /// session itself.
    pub(super) fn begin_headless_start(&mut self) {
        let reason =
            json!({ "event": "owner-changed", "owner": "host", "reason": "started headless" });
        self.sink.note(None, reason);
        let params = json!({
            "protocolVersion": 1,
            "clientCapabilities": {},
            "clientInfo": { "name": "brnr", "version": env!("CARGO_PKG_VERSION") },
        });
        self.host_request("initialize", params, HostRequest::Initialize);
    }

    pub(super) fn host_request(&mut self, method: &str, params: Value, kind: HostRequest) {
        self.next_id += 1;
        let id = Value::String(format!("brnr-{}", self.next_id));
        let key = id_key(&id);
        let cwd = params["cwd"].as_str().map(str::to_owned);
        let session = params["sessionId"].as_str().map(str::to_owned);
        let msg = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        let mut line = serde_json::to_vec(&msg).unwrap();
        line.push(b'\n');
        self.client_requests.insert(key.clone(), session.clone());
        self.host_requests.insert(key.clone(), kind);
        self.record(session.as_deref(), Dir::ControlToAgent, &line);
        if matches!(method, "session/new" | "session/fork") {
            let pending = super::acp::Pending::New {
                cwd,
                request: Some(line.clone()),
                dir: Dir::ControlToAgent,
            };
            self.pending.insert(key, pending);
        }
        self.write_agent(&line);
    }

    pub(super) fn host_request_done(&mut self, request: HostRequest, msg: &Map<String, Value>) {
        if let HostRequest::Peer { peer, req_id, op } = request {
            return self.peer_done(peer, req_id, op, msg);
        }
        if let Some(error) = msg.get("error") {
            let what = match &request {
                HostRequest::Initialize => "initialize".to_owned(),
                HostRequest::Open(Open::New) => "session/new".to_owned(),
                HostRequest::Open(Open::Resume(_)) => "session/resume".to_owned(),
                HostRequest::Open(Open::Load(session)) => {
                    if let Some(i) = self.find(session) {
                        self.sessions.remove(i);
                    }
                    "session/load".to_owned()
                }
                HostRequest::Setup(step) => step.describe(),
                HostRequest::Peer { .. } => unreachable!(),
            };
            let mut text = format!("{what} failed: {}", error_message(error));
            if is_auth_error(error) {
                text.push_str(&self.auth_hint());
            }
            return self.fail_start(&text);
        }
        let result = msg.get("result").cloned().unwrap_or(Value::Null);
        match request {
            HostRequest::Initialize => {
                self.agent_caps = result["agentCapabilities"].clone();
                self.auth_methods = result["authMethods"].clone();
                self.info["capabilities"] = self.capabilities();
                self.info["agent_info"] = result["agentInfo"].clone();
                if let Err(err) = self.check_mcp_servers() {
                    return self.fail_start(&err);
                }
                self.open_first_session();
            }
            HostRequest::Open(open) => {
                let session = match open {
                    Open::New => match result["sessionId"].as_str() {
                        Some(session) => session.to_owned(),
                        None => return self.fail_start("session/new returned no sessionId"),
                    },
                    Open::Resume(session) | Open::Load(session) => session,
                };
                if self.stop_requested {
                    return; // The start already failed (timed out) or was stopped.
                }
                let i = self.open_session(&session, None);
                self.sessions[i].replaying = false;
                self.sessions[i].state.result(&result);
                self.starting = Some(session);
                self.run_setup(i);
            }
            HostRequest::Setup(step) => {
                let Some(i) = self.starting.clone().and_then(|s| self.find(&s)) else { return };
                match step {
                    SetupStep::Mode(mode) => self.sessions[i].state.set_mode(&mode),
                    SetupStep::Config(..) => self.sessions[i].state.result(&result),
                }
                self.run_setup(i);
            }
            HostRequest::Peer { .. } => unreachable!(),
        }
    }

    /// What the agent can do, for status and for brnr to check up front.
    pub(super) fn capabilities(&self) -> Value {
        let caps = &self.agent_caps;
        let session = &caps["sessionCapabilities"];
        json!({
            "resume": !session["resume"].is_null(),
            "load": caps["loadSession"] == true,
            "list": !session["list"].is_null(),
            "fork": !session["fork"].is_null(),
            "close": !session["close"].is_null(),
            "image": caps["promptCapabilities"]["image"] == true,
            "mcp_http": caps["mcpCapabilities"]["http"] == true,
            "mcp_sse": caps["mcpCapabilities"]["sse"] == true,
        })
    }

    fn check_mcp_servers(&self) -> Result<(), String> {
        let caps = self.capabilities();
        for server in &self.mcp_servers {
            let kind = server["type"].as_str().unwrap_or("stdio");
            let supported = match kind {
                "http" => caps["mcp_http"] == true,
                "sse" => caps["mcp_sse"] == true,
                _ => true,
            };
            if !supported {
                let name = server["name"].as_str().unwrap_or("?");
                return Err(format!("MCP server {name}: the agent doesn't support {kind} servers"));
            }
        }
        Ok(())
    }

    fn open_first_session(&mut self) {
        let cwd = self.cwd.to_string_lossy().into_owned();
        let mcp = json!(self.mcp_servers);
        let Some(session) = self.resume.clone() else {
            let params = json!({ "cwd": cwd, "mcpServers": mcp });
            return self.host_request("session/new", params, HostRequest::Open(Open::New));
        };
        let params = json!({ "sessionId": session, "cwd": cwd, "mcpServers": mcp });
        let caps = self.capabilities();
        if caps["resume"] == true {
            self.host_request("session/resume", params, HostRequest::Open(Open::Resume(session)));
        } else if caps["load"] == true {
            // The agent replays the history; it is in the transcript already.
            let i = self.open_session(&session, None);
            self.sessions[i].replaying = true;
            self.host_request("session/load", params, HostRequest::Open(Open::Load(session)));
        } else {
            self.fail_start("the agent can't resume sessions (no session/resume or session/load)");
        }
    }

    /// The next mode or config step of a headless start, or, when there are
    /// none left, the start is done.
    fn run_setup(&mut self, i: usize) {
        let session = self.sessions[i].id.clone();
        let Some(step) = self.setup.pop_front() else { return self.finish_start(i) };
        let (method, params) = match &step {
            SetupStep::Mode(mode) => {
                ("session/set_mode", json!({ "sessionId": session, "modeId": mode }))
            }
            SetupStep::Config(id, value) => (
                "session/set_config_option",
                json!({ "sessionId": session, "configId": id, "value": value }),
            ),
        };
        self.host_request(method, params, HostRequest::Setup(step));
    }

    fn finish_start(&mut self, i: usize) {
        let session = self.sessions[i].id.clone();
        // brnr start hears of the session before the agent gets any work: if
        // it has gone, nobody knows this session exists.
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
        if self.manual {
            eprintln!("brnr host: session {session}");
        }
        if let Some(text) = self.first_prompt.take() {
            let held = Held { id: self.message_id(), text, blocks: Vec::new() };
            self.send_prompt(i, held);
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

    /// The agent needs a login, which a headless host can't do: say how.
    fn auth_hint(&self) -> String {
        let methods: Vec<&str> = self
            .auth_methods
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|m| m["name"].as_str().or(m["id"].as_str()))
            .collect();
        let offered =
            if methods.is_empty() { String::new() } else { format!(" ({})", methods.join(", ")) };
        format!(
            ". The agent needs you to log in{offered}; brnr can't do that without a terminal. \
             Log in with the agent's own CLI first (for example `claude` or `codex login`)"
        )
    }

    // ---- for bridges ---------------------------------------------------

    pub(super) fn peer_op(
        &mut self,
        peer: u64,
        req_id: Option<Value>,
        op: PeerOp,
        method: &str,
        params: Value,
    ) {
        self.host_request(method, params, HostRequest::Peer { peer, req_id, op });
    }

    fn peer_done(
        &mut self,
        peer: u64,
        req_id: Option<Value>,
        op: PeerOp,
        msg: &Map<String, Value>,
    ) {
        let reply = match msg.get("error") {
            Some(error) => json!({ "ok": false, "error": error_message(error) }),
            None => self.peer_result(op, msg.get("result").unwrap_or(&Value::Null)),
        };
        self.reply(peer, req_id, reply);
    }

    fn peer_result(&mut self, op: PeerOp, result: &Value) -> Value {
        match op {
            PeerOp::Mode { session, mode } => {
                if let Some(i) = self.find(&session) {
                    self.sessions[i].state.set_mode(&mode);
                    self.sessions[i].state.result(result);
                }
                json!({ "ok": true, "session": session, "mode": mode })
            }
            PeerOp::Config { session } => {
                let config = self.find(&session).map(|i| {
                    self.sessions[i].state.result(result);
                    self.sessions[i].state.config.clone()
                });
                json!({ "ok": true, "session": session, "config": config.flatten() })
            }
            PeerOp::Model { session, model } => {
                if let Some(i) = self.find(&session)
                    && let Some(models) = &mut self.sessions[i].state.models
                {
                    models["currentModelId"] = json!(model);
                }
                json!({ "ok": true, "session": session, "model": model })
            }
            PeerOp::List => json!({
                "ok": true,
                "sessions": result["sessions"],
                "next_cursor": result["nextCursor"],
            }),
            PeerOp::Fork { cwd } => {
                let Some(session) = result["sessionId"].as_str().map(str::to_owned) else {
                    return json!({ "ok": false, "error": "session/fork returned no sessionId" });
                };
                let i = self.open_session(&session, Some(&cwd.to_string_lossy()));
                self.sessions[i].state.result(result);
                json!({ "ok": true, "session": session })
            }
            PeerOp::Close { session } => {
                if let Some(i) = self.find(&session) {
                    self.flush_agent_message(i);
                    self.sessions.remove(i);
                }
                if self.sessions.is_empty() && !self.editor_attached() {
                    self.begin_stop();
                }
                json!({ "ok": true, "session": session })
            }
        }
    }
}

/// A queue of setup steps from `--mode` and `--set` (and the profile).
pub(super) fn setup_steps(
    mode: Option<String>,
    config: Vec<(String, String)>,
) -> VecDeque<SetupStep> {
    let mut steps: VecDeque<SetupStep> = mode.into_iter().map(SetupStep::Mode).collect();
    steps.extend(config.into_iter().map(|(k, v)| SetupStep::Config(k, v)));
    steps
}

pub(super) fn error_message(error: &Value) -> String {
    error["message"].as_str().map_or_else(|| error.to_string(), str::to_owned)
}

/// ACP's `auth_required` (-32000), or an agent that says as much.
fn is_auth_error(error: &Value) -> bool {
    error["code"] == -32000
        || error["message"].as_str().is_some_and(|m| m.to_lowercase().contains("auth"))
}
