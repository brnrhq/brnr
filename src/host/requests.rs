//! Requests the host sends the agent as its client:
//!
//! - opening the session of a headless start: `initialize`, `authenticate`
//!   if the start names a login method (ADR 30), then `session/new`,
//!   `session/resume` or `session/load` (a resumed session's lock taken
//!   first, ADR 3), then the mode and config options the start asked for,
//!   its flags over its profile's (ADR 58); then the start commits (see
//!   start.rs), and the prompt goes;
//! - what bridges ask of the agent through the host: set the mode, a config
//!   option or the model, fork or close a session. The bridge gets its answer
//!   when the agent's arrives;
//! - steering a message into a running turn (`_session/steering`, see
//!   acp.rs).

use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;

use serde::Serialize;
use serde_json::{Map, Value, json};

use super::acp::{Held, Replay, new_session};
use super::state::SessionState;
use super::{Host, id_key};
use crate::log::{self, Dir};
use crate::request::Settings;
use crate::schema::{self, AgentCapabilities, Error, ErrorCode, error_message};

pub(super) enum HostRequest {
    Initialize,
    Authenticate(String),
    Open(Open),
    Setup(SetupStep),
    Peer {
        peer: u64,
        req_id: Option<Value>,
        op: PeerOp,
    },
    /// Message `message`, steered into `session`'s running turn.
    Steer {
        session: String,
        message: String,
    },
}

/// How a headless start gets its session.
pub(super) enum Open {
    New,
    Resume(String),
    Load(String),
}

/// What a headless start applies before the first prompt. The mode and the
/// model are found as `brnr mode` and `brnr model` find them (ADR 28): the
/// model as the id of its option, and the value.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum SetupStep {
    Mode(String),
    Model(String, String),
    Config(String, String),
}

impl SetupStep {
    fn describe(&self) -> String {
        match self {
            SetupStep::Mode(mode) => format!("setting mode {mode}"),
            SetupStep::Model(_, model) => format!("setting model {model}"),
            SetupStep::Config(id, value) => format!("setting {id}={value}"),
        }
    }
}

/// What a bridge asked the agent for. A close is `by` `close`, or `idle` for
/// `stop_when_idle`, as `session_closed` has it.
pub(super) enum PeerOp {
    Mode { session: String, mode: String },
    Config { session: String },
    Fork { cwd: PathBuf },
    Close { session: String, by: &'static str },
}

impl Host {
    /// Started with no editor (`brnr start`): the host opens the session
    /// itself.
    pub(super) fn begin_headless_start(&mut self) {
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
        // Without the MCP servers' secrets (ADR 25 in docs/adr).
        let recorded = msg.as_object().and_then(log::redacted).unwrap_or_else(|| line.clone());
        self.record(session.as_deref(), Dir::ControlToAgent, &recorded);
        if matches!(method, "session/new" | "session/fork") {
            let pending =
                super::acp::Pending::New { cwd, request: Some(recorded), dir: Dir::ControlToAgent };
            self.pending.insert(key, pending);
        }
        self.write_agent(&line);
    }

    pub(super) fn host_request_done(&mut self, request: HostRequest, msg: &Map<String, Value>) {
        if let HostRequest::Peer { peer, req_id, op } = request {
            return self.peer_done(peer, req_id, op, msg);
        }
        if let HostRequest::Steer { session, message } = request {
            return self.steer_answered(&session, &message, msg);
        }
        if let Some(error) = msg.get("error") {
            let what = match &request {
                HostRequest::Initialize => "initialize".to_owned(),
                HostRequest::Authenticate(method) => format!("authenticate {method}"),
                HostRequest::Open(Open::New) => "session/new".to_owned(),
                HostRequest::Open(Open::Resume(_)) => "session/resume".to_owned(),
                HostRequest::Open(Open::Load(session)) => {
                    if let Some(i) = self.find(session) {
                        self.sessions.remove(i);
                    }
                    "session/load".to_owned()
                }
                HostRequest::Setup(step) => step.describe(),
                HostRequest::Peer { .. } | HostRequest::Steer { .. } => unreachable!(),
            };
            let mut text = format!("{what} failed: {}", error_message(error));
            // The hint is for a login that wasn't asked for.
            if is_auth_error(error) && !matches!(request, HostRequest::Authenticate(_)) {
                text.push_str(&self.auth_hint());
            }
            return self.fail_start(&text);
        }
        let result = msg.get("result").cloned().unwrap_or(Value::Null);
        match request {
            HostRequest::Initialize => {
                self.initialized(&result);
                if let Err(err) = self.check_mcp_servers() {
                    return self.fail_start(&err);
                }
                let checked = self.prompt.as_ref().map_or(Ok(()), |p| self.check_blocks(&p.blocks));
                if let Err(err) = checked {
                    return self.fail_start(&err);
                }
                match self.auth.clone() {
                    Some(method) => self.authenticate(method),
                    None => self.open_first_session(),
                }
            }
            HostRequest::Authenticate(_) => self.open_first_session(),
            HostRequest::Open(open) => {
                let session = match open {
                    Open::New => match new_session(&result) {
                        Some(session) => session,
                        None => return self.fail_start("session/new returned no sessionId"),
                    },
                    Open::Resume(session) | Open::Load(session) => session,
                };
                if self.stop_requested {
                    return; // The start already failed (timed out) or was stopped.
                }
                let i = self.open_session(&session, None);
                // A resumed session's lock was taken before the agent was
                // asked for it; a new one's, as it opened. Without it, the
                // session gets no work, and nobody is to find it (ADR 50).
                if let Some(why) = self.sessions[i].not_owned() {
                    self.sessions.remove(i);
                    return self.fail_start(&format!("the agent opened {session}, which {why}"));
                }
                self.end_replay(i);
                self.sessions[i].state.result(&result);
                self.starting = Some(session);
                let (flags, profile) = std::mem::take(&mut self.settings);
                match setup_steps(&flags, &profile, &self.sessions[i].state) {
                    Ok(steps) => self.setup = steps,
                    Err(error) => return self.fail_start(&error),
                }
                self.run_setup(i);
            }
            HostRequest::Setup(step) => {
                let Some(i) = self.starting.clone().and_then(|s| self.find(&s)) else { return };
                if let SetupStep::Mode(mode) = &step {
                    self.sessions[i].state.set_mode(mode);
                }
                // A config option's answer has them all (a mode or model
                // too); what it changed is a `session_changed`.
                self.apply_result(i, &result);
                self.run_setup(i);
            }
            HostRequest::Peer { .. } | HostRequest::Steer { .. } => unreachable!(),
        }
    }

    /// The agent's answer to `initialize`, the host's own or the editor's:
    /// what it can do, and the login methods it offers.
    pub(super) fn initialized(&mut self, result: &Value) {
        self.caps = Capabilities::of(result);
        let methods = result["authMethods"].as_array().map_or(&[][..], Vec::as_slice);
        self.auth_methods = methods.iter().filter_map(schema::read).collect();
        self.info["capabilities"] = json!(self.caps);
        self.info["agent_info"] = result["agentInfo"].clone();
    }

    fn check_mcp_servers(&self) -> Result<(), String> {
        for server in &self.mcp_servers {
            let kind = server["type"].as_str().unwrap_or("stdio");
            let supported = match kind {
                "http" => self.caps.mcp_http,
                "sse" => self.caps.mcp_sse,
                _ => true,
            };
            if !supported {
                let name = server["name"].as_str().unwrap_or("?");
                return Err(format!("MCP server {name}: the agent doesn't support {kind} servers"));
            }
        }
        Ok(())
    }

    /// The login method the start names, before the session opens. brnr
    /// never picks one (P4); one the agent doesn't offer fails the start up
    /// front (P7).
    fn authenticate(&mut self, method: String) {
        let offered: Vec<String> = self.auth_methods.iter().map(|m| m.id().to_string()).collect();
        if !offered.contains(&method) {
            let offered = if offered.is_empty() { "none".to_owned() } else { offered.join(", ") };
            let error = format!("the agent offers no login method {method} (it offers: {offered})");
            return self.fail_start(&error);
        }
        let params = json!({ "methodId": method });
        self.host_request("authenticate", params, HostRequest::Authenticate(method));
    }

    fn open_first_session(&mut self) {
        let cwd = self.cwd.to_string_lossy().into_owned();
        let mcp = json!(self.mcp_servers);
        let Some(session) = self.resume.clone() else {
            let params = json!({ "cwd": cwd, "mcpServers": mcp });
            return self.host_request("session/new", params, HostRequest::Open(Open::New));
        };
        let params = json!({ "sessionId": session, "cwd": cwd, "mcpServers": mcp });
        let caps = self.caps;
        if caps.resume || caps.load {
            // Taken before the agent hears of it: a session another process
            // holds is refused (ADR 3).
            if let Err(err) = self.own(&session) {
                return self.fail_start(&err);
            }
        }
        if caps.resume {
            self.host_request("session/resume", params, HostRequest::Open(Open::Resume(session)));
        } else if caps.load {
            // The agent replays the history: recorded unless the transcript
            // has it already (ADR 57).
            let i = self.open_session(&session, None);
            self.sessions[i].replay = Some(Replay { record: !self.transcript, updates: 0 });
            self.host_request("session/load", params, HostRequest::Open(Open::Load(session)));
        } else {
            self.fail_start("the agent can't resume sessions (no session/resume or session/load)");
        }
    }

    /// The end of a load's replay, when the agent answers it: the history
    /// event says how many updates it replayed, and whether they were
    /// recorded (ADR 57).
    fn end_replay(&mut self, i: usize) {
        // The last replayed message, while it is still marked as replayed.
        self.flush_agent_message(i);
        let Some(replay) = self.sessions[i].replay.take() else { return };
        if replay.record {
            // A tool call history left unfinished isn't running now.
            self.sessions[i].state.tools.clear();
        }
        let session = self.sessions[i].id.clone();
        self.emit(json!({
            "event": "history",
            "session": session,
            "updates": replay.updates,
            "recorded": replay.record,
        }));
    }

    /// The next mode or config step of a headless start, or, when there are
    /// none left, the start is done.
    fn run_setup(&mut self, i: usize) {
        let session = self.sessions[i].id.clone();
        let Some(step) = self.setup.pop_front() else { return self.finish_start(i) };
        let state = &self.sessions[i].state;
        let option = |category| state.option(category).map(|o| o["id"].clone());
        let set = |id, value| {
            let params = json!({ "sessionId": session, "configId": id, "value": value });
            ("session/set_config_option", params)
        };
        let (method, params) = match &step {
            // An agent with modes only as a config option.
            SetupStep::Mode(mode) if state.modes.is_none() => match option("mode") {
                Some(id) => set(id, mode),
                None => {
                    let error = format!("{}: the agent offers no modes", step.describe());
                    return self.fail_start(&error);
                }
            },
            SetupStep::Mode(mode) => {
                ("session/set_mode", json!({ "sessionId": session, "modeId": mode }))
            }
            SetupStep::Model(id, model) => set(json!(id), model),
            SetupStep::Config(id, value) => set(json!(id), value),
        };
        self.host_request(method, params, HostRequest::Setup(step));
    }

    /// The commit (ADR 7): brnr start hears of the session before the agent
    /// gets any work, and if the report can't be written to it, nobody knows
    /// this session exists. Then the prompt goes.
    fn finish_start(&mut self, i: usize) {
        if self.stop_requested {
            return; // The start already failed (timed out), or was stopped.
        }
        let session = self.sessions[i].id.clone();
        let message = self.prompt.is_some().then(|| self.message_id());
        if !self.report_ready(&session, message.as_deref()) {
            return;
        }
        self.start_deadline = None;
        self.start_done = true;
        if self.foreground {
            self.say(format!("brnr: session {}", crate::render::clean(&session)));
        }
        if let (Some(prompt), Some(id)) = (self.prompt.take(), message) {
            self.send_prompt(i, Held { id, text: prompt.text, blocks: prompt.blocks });
        }
    }

    pub(super) fn fail_start(&mut self, error: &str) {
        self.claimed.clear(); // Nobody is to know of a session it was opening.
        self.sink.note(None, json!({ "event": "start-failed", "error": error }));
        self.startup_failed(error);
        self.begin_stop();
    }

    /// Reports `error` to brnr start if it is still waiting, and in the
    /// foreground on stderr: a step after the session opened (its mode, a
    /// config option) fails the start as much as opening it does. It ends
    /// with the agent's last lines on stderr (ADR 10).
    pub(super) fn startup_failed(&mut self, error: &str) {
        let error = self.with_stderr(error);
        if self.foreground && !self.start_done && !self.startup_reported {
            self.startup_reported = true;
            self.say(format!("brnr: {}", crate::render::clean(&error)));
        }
        self.report_failure(&error);
    }

    /// The agent needs a login, which a headless host can't do: say how.
    fn auth_hint(&self) -> String {
        // By id too: that is what --auth takes (ADR 30).
        let methods: Vec<String> =
            self.auth_methods.iter().map(|m| format!("{} `{}`", m.name(), m.id())).collect();
        let offered =
            if methods.is_empty() { String::new() } else { format!(" ({})", methods.join(", ")) };
        format!(
            ". The agent needs you to log in{offered}; brnr can't do that without a terminal. \
             Log in with the agent's own CLI first (for example `claude` or `codex login`), or \
             name a method that needs no terminal with --auth <id>"
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
            Some(error) => {
                if let PeerOp::Close { session, .. } = &op
                    && let Some(i) = self.find(session)
                {
                    self.sessions[i].closing = None; // Still open: it takes requests again.
                    self.close_failed(session, &error_message(error));
                }
                json!({ "ok": false, "error": error_message(error) })
            }
            None => self.peer_result(op, msg.get("result").unwrap_or(&Value::Null)),
        };
        self.reply(peer, req_id, reply);
    }

    fn peer_result(&mut self, op: PeerOp, result: &Value) -> Value {
        match op {
            // The agent answers only the requester (ADR 28): an editor is
            // told here (see experimental.rs).
            PeerOp::Mode { session, mode } => {
                if let Some(i) = self.find(&session) {
                    self.sessions[i].state.set_mode(&mode);
                    self.apply_result(i, result);
                }
                self.mode_set(&session, &mode);
                json!({ "ok": true, "session": session, "mode": mode })
            }
            PeerOp::Config { session } => {
                let config = self.find(&session).map(|i| {
                    self.apply_result(i, result);
                    self.sessions[i].state.config.clone()
                });
                self.config_set(&session, result);
                json!({ "ok": true, "session": session, "config": config.flatten() })
            }
            PeerOp::Fork { cwd } => {
                let Some(session) = new_session(result) else {
                    return json!({ "ok": false, "error": "session/fork returned no sessionId" });
                };
                let i = self.open_session(&session, Some(&cwd.to_string_lossy()));
                // Its lock, taken as it opened: one another process holds, or
                // one that can't be locked, can't be served here (ADR 3, ADR 50).
                if let Some(why) = self.sessions[i].not_owned() {
                    self.sink.close_session(&self.sessions.remove(i).id);
                    let error = format!("the agent forked into {session}, which {why}");
                    return json!({ "ok": false, "error": error });
                }
                self.sessions[i].state.result(result);
                json!({ "ok": true, "session": session })
            }
            PeerOp::Close { session, by } => {
                if let Some(i) = self.find(&session) {
                    self.close_session(i, by);
                }
                if self.sessions.is_empty() && !self.editor_attached() {
                    self.begin_stop();
                }
                json!({ "ok": true, "session": session })
            }
        }
    }
}

/// What the agent can do, as it said in `initialize`: for status, and for
/// brnr to check up front. What is stable ACP is read as the schema has it;
/// `fork` is unstable, and `steering` a convention advertised in `_meta`
/// (ADR 41), so both are read from the result as it came.
#[derive(Clone, Copy, Default, Serialize)]
pub(super) struct Capabilities {
    pub(super) resume: bool,
    pub(super) load: bool,
    pub(super) list: bool,
    pub(super) fork: bool,
    pub(super) close: bool,
    pub(super) image: bool,
    pub(super) steering: bool,
    pub(super) mcp_http: bool,
    pub(super) mcp_sse: bool,
}

impl Capabilities {
    /// From the result of `initialize`.
    fn of(result: &Value) -> Capabilities {
        let raw = &result["agentCapabilities"];
        let caps: AgentCapabilities = schema::read(raw).unwrap_or_default();
        let session = &caps.session_capabilities;
        Capabilities {
            resume: session.resume.is_some(),
            load: caps.load_session,
            list: session.list.is_some(),
            fork: !raw["sessionCapabilities"]["fork"].is_null(),
            close: session.close.is_some(),
            image: caps.prompt_capabilities.image,
            steering: result["_meta"]["steering"]["supported"] == true,
            mcp_http: caps.mcp_capabilities.http,
            mcp_sse: caps.mcp_capabilities.sse,
        }
    }
}

/// The setup steps of a headless start, once its session is open: its flags
/// over its profile's settings, setting by setting (ADR 58). A config option
/// whose category is `mode` or `model` is that setting, so `--model` replaces
/// a profile's `config = { <its id> = … }`. Two values for one setting from
/// the same source fail (P4).
pub(super) fn setup_steps(
    flags: &Settings,
    profile: &Settings,
    state: &SessionState,
) -> Result<VecDeque<SetupStep>, String> {
    let id = |category| state.option(category).and_then(|o| o["id"].as_str()).map(str::to_owned);
    let ids = (id("mode"), id("model"));
    let flags = Resolved::of(flags, &ids, None)?;
    let profile = Resolved::of(profile, &ids, Some(&flags))?;
    let mode = flags.mode.or(profile.mode);
    let mut steps: VecDeque<SetupStep> = mode.map(SetupStep::Mode).into_iter().collect();
    if let Some(model) = flags.model.or(profile.model) {
        let Some(id) = ids.1 else {
            return Err(format!("setting model {model}: the agent offers no model choice"));
        };
        steps.push_back(SetupStep::Model(id, model));
    }
    let mut config = profile.config;
    config.extend(flags.config);
    steps.extend(config.into_iter().map(|(k, v)| SetupStep::Config(k, v)));
    Ok(steps)
}

/// One source's settings, with its options of category `mode` and `model`
/// taken as the mode and the model.
struct Resolved {
    mode: Option<String>,
    model: Option<String>,
    config: BTreeMap<String, String>,
}

impl Resolved {
    /// `ids`: the agent's mode and model options. `over`: the flags, when
    /// these are the profile's settings: what the flags set, the profile's
    /// values for are neither applied nor checked.
    fn of(
        s: &Settings,
        ids: &(Option<String>, Option<String>),
        over: Option<&Resolved>,
    ) -> Result<Resolved, String> {
        let (single, option) =
            if over.is_some() { ("the profile's ", "its config ") } else { ("--", "--set ") };
        let mut config = s.config.clone();
        let mut one = |what: &str, value: &Option<String>, id: &Option<String>, won: bool| {
            let other = id.as_ref().and_then(|id| Some((id, config.remove(id)?)));
            match (value, other) {
                _ if won => Ok(None),
                (Some(value), Some((id, other))) if *value != other => Err(format!(
                    "{single}{what} {value} and {option}{id}={other} both set the {what}"
                )),
                (Some(value), _) => Ok(Some(value.clone())),
                (None, other) => Ok(other.map(|(_, v)| v)),
            }
        };
        let mode = one("mode", &s.mode, &ids.0, over.is_some_and(|o| o.mode.is_some()))?;
        let model = one("model", &s.model, &ids.1, over.is_some_and(|o| o.model.is_some()))?;
        Ok(Resolved { mode, model, config })
    }
}

/// ACP's `auth_required`, or an agent that says as much.
fn is_auth_error(error: &Value) -> bool {
    schema::read::<Error>(error).is_some_and(|e| {
        e.code == ErrorCode::AuthRequired || e.message.to_lowercase().contains("auth")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_as_the_adapters_advertise_them() {
        let session = json!({
            "additionalDirectories": {}, "close": {}, "delete": {}, "fork": {},
            "list": {}, "resume": {}, "subagents": {},
        });
        // claude-agent-acp 0.85.1.
        let claude = json!({
            "protocolVersion": 1,
            "agentCapabilities": {
                "_meta": { "claudeCode": { "promptQueueing": true }, "authStatus": {} },
                "promptCapabilities": { "image": true, "embeddedContext": true },
                "mcpCapabilities": { "http": true, "sse": true },
                "auth": { "logout": {} },
                "providers": {},
                "loadSession": true,
                "sessionCapabilities": session,
            },
            "agentInfo": { "name": "@agentclientprotocol/claude-agent-acp", "version": "0.85.1" },
            "authMethods": [],
            "_meta": { "steering": { "supported": true } },
        });
        let caps = json!(Capabilities::of(&claude));
        let all = json!({
            "resume": true, "load": true, "list": true, "fork": true, "close": true,
            "image": true, "steering": true, "mcp_http": true, "mcp_sse": true,
        });
        assert_eq!(caps, all);
        // codex-acp 2.1.1: no SSE, and an unstable MCP capability.
        let mut codex = claude.clone();
        codex["agentCapabilities"]["mcpCapabilities"] =
            json!({ "acp": false, "http": true, "sse": false });
        assert_eq!(json!(Capabilities::of(&codex))["mcp_sse"], false);
        // Nothing said, nothing assumed; `fork` is read where the schema
        // doesn't have it (unstable), steering in `_meta`.
        let none = json!(Capabilities::default());
        assert_eq!(json!(Capabilities::of(&json!({ "protocolVersion": 1 }))), none);
        let forks = json!({ "agentCapabilities": { "sessionCapabilities": { "fork": {} } } });
        let forks = Capabilities::of(&forks);
        assert!(forks.fork && !forks.resume && !forks.steering);
    }
}
