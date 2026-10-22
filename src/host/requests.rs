//! Requests the host sends the agent as its client:
//!
//! - opening the session of a headless start: `initialize` (whose answer
//!   must choose ACP version 1, ADR 2.2), `authenticate`
//!   if the start names a login method (ADR 4.7), then `session/new`,
//!   `session/resume` or `session/load` (a resumed session's lock taken
//!   first, ADR 3.9), then the settings the start asked for, its flags over
//!   its profile's (ADR 6.1); then the start commits (see start.rs), and the
//!   prompt goes;
//! - what bridges ask of the agent through the host: settings (`config set`,
//!   resolved as a start's are, ADR 8.1), fork or close a session. The bridge
//!   gets its answer when the agent's arrives;
//! - a session a bridge opens in the running process (`new`, `resume`:
//!   `session new --pid`, `session resume --pid`, ADR 8.1), opened, set up and
//!   committed as a start's is. The commit is the bridge's answer, queued for
//!   it while it is still connected, and then the prompt goes. Whatever fails
//!   after the agent opened the session (a setting, the bridge's timeout
//!   passing, the bridge going away) closes it again where the agent can
//!   close sessions, and says so; where it can't, the answer names the
//!   session left open (P3). A process stopping meanwhile doesn't commit it,
//!   and one with a session still opening doesn't stop for having none;
//! - steering a message into a running turn (`_session/steering`, see
//!   acp.rs).

use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::time::Instant;

use serde::Serialize;
use serde_json::{Map, Value, json};

use super::Host;
use super::acp::{Close, Replay, new_session};
use super::outbound::{Asked, By, Opens, Then};
use super::sessions::Ending;
use super::state::SessionState;
use super::turns::Held;
use crate::error::{Code, Error};
use crate::log::{self, Dir};
use crate::request::{Prompt, Settings};
use crate::schema::{
    self, AgentCapabilities, ElicitationCapabilities, ElicitationFormCapabilities,
    ElicitationUrlCapabilities, Error as AcpError, ErrorCode, error_message,
};

/// One of the host's own requests to the agent, as it waits for the
/// answer: what the answer goes on to (see outbound.rs).
pub(super) enum HostRequest {
    /// A step of the headless start.
    Start(StartStep),
    /// A peer's `new` or `resume`.
    Opening(Open, Box<Opening>),
    /// The step the agent is answering, and what is left.
    Setup(SetupStep, Setup),
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

/// The headless start's steps before its settings: `initialize`, the
/// login, the session.
pub(super) enum StartStep {
    Initialize,
    Authenticate(String),
    Open(Open),
}

/// How a headless start, or a peer's `new` or `resume`, gets its session.
pub(super) enum Open {
    New,
    Resume(String),
    Load(String),
}

/// One setting, resolved (see [`resolve_settings`]): what is sent for it.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum SetupStep {
    /// `session/set_mode`: an agent with v1 modes and no mode option.
    Mode(String),
    /// `session/set_config_option` for option `id`, found by its category
    /// (`mode`, `model`, `thought_level`) or, with none, by its id.
    Option { id: String, value: String, category: Option<&'static str> },
}

impl SetupStep {
    fn describe(&self) -> String {
        match self {
            SetupStep::Mode(mode) => format!("setting mode {mode}"),
            SetupStep::Option { value, category: Some(c), .. } => {
                format!("setting {} {value}", c.replace('_', " "))
            }
            SetupStep::Option { id, value, category: None } => format!("setting {id}={value}"),
        }
    }

    /// As `config set` reports it.
    fn report(&self) -> Value {
        match self {
            SetupStep::Mode(mode) => json!({ "option": null, "category": "mode", "value": mode }),
            SetupStep::Option { id, value, category } => {
                json!({ "option": id, "category": category, "value": value })
            }
        }
    }
}

/// Settings being sent to `session`'s agent one at a time, each once the
/// one before is answered: a headless start's, after which it commits, a
/// bridge's `set_config`, answered once all are set or one fails, or a
/// bridge's `new` or `resume`, which then commits as a start does.
pub(super) struct Setup {
    pub(super) session: String,
    pub(super) steps: VecDeque<SetupStep>,
    /// Those the agent has set, for the bridge's answer.
    pub(super) done: Vec<SetupStep>,
    pub(super) of: SetupOf,
}

/// Whose settings a [`Setup`] sends.
pub(super) enum SetupOf {
    /// The headless start's.
    Start,
    /// A bridge's `set_config`: the bridge, and its request's `req_id`.
    Config(u64, Option<Value>),
    /// A bridge's `new` or `resume`.
    Open(Box<Opening>),
}

/// A session a bridge opens in this process (`new`, `resume`, ADR 8.1): who
/// asked, and what it commits with.
pub(super) struct Opening {
    pub(super) peer: u64,
    pub(super) req_id: Option<Value>,
    /// Where the session runs.
    pub(super) cwd: PathBuf,
    /// Its flags, over the process's profile's as a start's are (ADR 6.1).
    pub(super) settings: Settings,
    /// Sent once it commits.
    pub(super) prompt: Option<Prompt>,
    /// The command's timeout: past it the opening is abandoned rather than
    /// committed, before the command, which waits a little longer, gives
    /// up (ADR 3.4).
    pub(super) deadline: Option<Instant>,
}

/// What a bridge asked the agent for. A close is `by` `close`, or `idle` for
/// `stop_when_idle`, as `session_closed` has it; one with `failed` closes the
/// session a bridge's `new` or `resume` opened, and `failed`, why, is the
/// bridge's answer.
pub(super) enum PeerOp {
    Fork { cwd: PathBuf },
    Close { session: String, by: &'static str, failed: Option<Error> },
}

impl Host {
    /// Started with no editor (`brnr session new` or `resume`): the host
    /// opens the session itself. It offers no `fs` or `terminal`: with no
    /// editor, nothing would serve them (ADR 2.7 in docs/adr).
    pub(super) fn begin_headless_start(&mut self) {
        // Elicitation in both modes (ADR 4.5): a form, or a URL the user
        // opens; as the schema has them, an object per mode (ADR 2.3).
        let elicitation = ElicitationCapabilities::new()
            .form(ElicitationFormCapabilities::new())
            .url(ElicitationUrlCapabilities::new());
        let params = json!({
            "protocolVersion": schema::PROTOCOL_VERSION,
            "clientCapabilities": { "elicitation": elicitation },
            "clientInfo": { "name": "brnr", "version": env!("CARGO_PKG_VERSION") },
        });
        self.host_request("initialize", params, HostRequest::Start(StartStep::Initialize));
    }

    pub(super) fn host_request(&mut self, method: &str, params: Value, request: HostRequest) {
        let id = self.outbound.next_id();
        let session = params["sessionId"].as_str().map(str::to_owned);
        let mut opens = Opens::of(method, &params, Dir::ControlToAgent);
        let msg = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        let mut line = serde_json::to_vec(&msg).unwrap();
        line.push(b'\n');
        // Without the MCP servers' secrets (ADR 9.1 in docs/adr).
        let recorded = msg.as_object().and_then(log::redacted).unwrap_or_else(|| line.clone());
        self.record(session.as_deref(), Dir::ControlToAgent, &recorded);
        if let Some(opens) = &mut opens {
            opens.request = Some(recorded);
        }
        let then = Then::Host(request);
        self.outbound.insert(&id, Asked { by: By::Host, session, opens, then });
        self.write_agent(&line);
    }

    /// The agent's answer to one of the host's own requests.
    pub(super) fn host_request_done(&mut self, request: HostRequest, msg: &Map<String, Value>) {
        match request {
            HostRequest::Start(step) => self.start_answered(step, msg),
            HostRequest::Opening(open, opening) => self.peer_opened(open, opening, msg),
            HostRequest::Setup(step, setup) => self.setup_answered(step, setup, msg),
            HostRequest::Peer { peer, req_id, op } => self.peer_done(peer, req_id, op, msg),
            HostRequest::Steer { session, message } => self.steer_answered(&session, &message, msg),
        }
    }

    /// The agent's answer to one of the headless start's steps, `step`: an
    /// error fails the start; otherwise the next step goes.
    fn start_answered(&mut self, step: StartStep, msg: &Map<String, Value>) {
        if let Some(error) = msg.get("error") {
            let what = match &step {
                StartStep::Initialize => "initialize".to_owned(),
                StartStep::Authenticate(method) => format!("authenticate {method}"),
                StartStep::Open(Open::New) => "session/new".to_owned(),
                StartStep::Open(Open::Resume(_)) => "session/resume".to_owned(),
                StartStep::Open(Open::Load(session)) => {
                    // Its lock goes once its replay is written, with the
                    // exit's records at the latest (ADR 5.5).
                    self.open_failed(session, None);
                    "session/load".to_owned()
                }
            };
            let mut text = format!("{what} failed: {}", error_message(error));
            // The hint is for a login that wasn't asked for.
            if is_auth_error(error) && !matches!(step, StartStep::Authenticate(_)) {
                text.push_str(&self.auth_hint());
            }
            return self.fail_start(&Error::agent(text, error));
        }
        let result = msg.get("result").cloned().unwrap_or(Value::Null);
        match step {
            StartStep::Initialize => {
                // An answer in a version brnr doesn't speak isn't read any
                // further, and nothing more is sent (ADR 2.2).
                if let Err(err) = schema::check_protocol_version(&result) {
                    return self.fail_start(&Error::new(Code::acp_version_unsupported, err));
                }
                self.initialized(&result);
                let Some(start) = &self.start else { return }; // Failed, or abandoned.
                let auth = start.auth.clone();
                let checked =
                    start.prompt.as_ref().map_or(Ok(()), |p| self.check_blocks(&p.blocks));
                if let Err(err) = self.check_mcp_servers().and(checked) {
                    return self.fail_start(&err);
                }
                match auth {
                    Some(method) => self.authenticate(method),
                    None => self.open_first_session(),
                }
            }
            StartStep::Authenticate(_) => self.open_first_session(),
            StartStep::Open(open) => {
                let session = match open {
                    Open::New => match new_session(&result) {
                        Some(session) => session,
                        None => {
                            return self.fail_start(&"session/new returned no sessionId".into());
                        }
                    },
                    Open::Resume(session) | Open::Load(session) => session,
                };
                // The start already failed (timed out), or was stopped.
                let Some(start) = self.start.as_mut().filter(|_| !self.stop_requested) else {
                    return;
                };
                let flags = std::mem::take(&mut start.flags);
                let i = self.open_session(&session, None);
                // A resumed session's lock was taken before the agent was
                // asked for it; a new one's, as it opened. Without it, the
                // session gets no work, and nobody is to find it (ADR 3.9).
                if let Some(why) = self.sessions[i].not_owned() {
                    self.close_session(i, Ending::NotOwned, None);
                    let error = why.map(|why| format!("the agent opened {session}, which {why}"));
                    return self.fail_start(&error);
                }
                self.end_replay(i);
                self.sessions[i].state.result(&result);
                match resolve_settings(&flags, &self.defaults, &self.sessions[i].state) {
                    Ok(steps) => {
                        self.run_setup(Setup { session, steps, done: vec![], of: SetupOf::Start })
                    }
                    Err(error) => self.fail_start(&error),
                }
            }
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

    fn check_mcp_servers(&self) -> Result<(), Error> {
        for server in &self.mcp_servers {
            let kind = server["type"].as_str().unwrap_or("stdio");
            let supported = match kind {
                "http" => self.caps.mcp_http,
                "sse" => self.caps.mcp_sse,
                _ => true,
            };
            if !supported {
                let name = server["name"].as_str().unwrap_or("?");
                let error = format!("MCP server {name}: the agent doesn't support {kind} servers");
                return Err(Error::new(Code::capability_missing, error));
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
            return self.fail_start(&Error::new(Code::setting_invalid, error));
        }
        let params = json!({ "methodId": method });
        let step = HostRequest::Start(StartStep::Authenticate(method));
        self.host_request("authenticate", params, step);
    }

    fn open_first_session(&mut self) {
        let cwd = self.cwd.to_string_lossy().into_owned();
        let mcp = json!(self.mcp_servers);
        let Some(start) = &self.start else { return }; // Failed, or abandoned.
        let transcript = start.transcript;
        let Some(session) = start.resume.clone() else {
            let params = json!({ "cwd": cwd, "mcpServers": mcp });
            let step = HostRequest::Start(StartStep::Open(Open::New));
            return self.host_request("session/new", params, step);
        };
        let params = json!({ "sessionId": session, "cwd": cwd, "mcpServers": mcp });
        let caps = self.caps;
        if caps.resume || caps.load {
            // Taken before the agent hears of it: a session another process
            // holds is refused (ADR 3.9).
            if let Err(err) = self.sessions.own(&session) {
                return self.fail_start(&err);
            }
        }
        if caps.resume {
            let step = HostRequest::Start(StartStep::Open(Open::Resume(session)));
            self.host_request("session/resume", params, step);
        } else if caps.load {
            // The agent replays the history: recorded unless the transcript
            // has it already (ADR 5.6).
            let i = self.open_session(&session, None);
            self.begin_replay(i, !transcript);
            let step = HostRequest::Start(StartStep::Open(Open::Load(session)));
            self.host_request("session/load", params, step);
        } else {
            let error = "the agent can't resume sessions (no session/resume or session/load)";
            self.fail_start(&Error::new(Code::capability_missing, error));
        }
    }

    /// A load of session `i` is to replay its history, `record`ed if brnr
    /// has no transcript of it (ADR 5.6).
    pub(super) fn begin_replay(&mut self, i: usize, record: bool) {
        // A message still coming isn't the history's.
        self.flush_agent_message(i);
        let tools = self.sessions[i].state.tools.iter().map(|t| t.tool_call_id.clone()).collect();
        let replay = Replay {
            record,
            updates: 0,
            recorded_updates: 0,
            tools,
            loads: 1,
            unanswered: 1,
            loaded: false,
            opened: false,
        };
        self.sessions[i].replay = Some(replay);
    }

    /// Session `i`'s replay, ended, whether its loads succeeded or not: a
    /// tool call the history left unfinished isn't running now (ADR 8.2).
    pub(super) fn drop_replay(&mut self, i: usize) -> Option<Replay> {
        let replay = self.sessions[i].replay.take()?;
        if replay.record {
            self.sessions[i].state.tools.retain(|t| replay.tools.contains(&t.tool_call_id));
        }
        Some(replay)
    }

    /// The end of a load's replay, when the agent answers it: the history
    /// event says how many updates it replayed, and whether they were
    /// recorded (ADR 5.6).
    pub(super) fn end_replay(&mut self, i: usize) {
        // The last replayed message, while it is still marked as replayed.
        self.flush_agent_message(i);
        let Some(replay) = self.drop_replay(i) else { return };
        let session = self.sessions[i].id.clone();
        self.emit(history(&session, &replay));
    }

    /// Sends the next of `setup`'s settings, or, when there are none left,
    /// ends it: a start commits, a bridge is answered.
    pub(super) fn run_setup(&mut self, mut setup: Setup) {
        if matches!(setup.of, SetupOf::Start) && self.stop_requested {
            return; // The start already failed (timed out), or was stopped.
        }
        // Before each setting, and before the commit.
        if let SetupOf::Open(o) = &setup.of
            && let Some(why) = self.open_refused(o)
        {
            let (peer, req_id) = (o.peer, o.req_id.clone());
            return self.abandon_open(&setup.session, peer, req_id, why);
        }
        let Some(i) = self.find(&setup.session) else {
            let error = format!("{} closed before its settings were set", setup.session);
            return self.setup_failed(setup, Error::new(Code::session_closed, error));
        };
        let Some(step) = setup.steps.pop_front() else {
            let (peer, req_id) = match setup.of {
                SetupOf::Start => return self.finish_start(i),
                SetupOf::Open(opening) => return self.commit_open(i, opening),
                SetupOf::Config(peer, req_id) => (peer, req_id),
            };
            let state = &self.sessions[i].state;
            let set: Vec<Value> = setup.done.iter().map(SetupStep::report).collect();
            let reply = json!({
                "ok": true,
                "session": setup.session,
                "set": set,
                "mode": state.current_mode(),
                "config": state.config,
            });
            return self.reply(peer, req_id, reply);
        };
        let session = &setup.session;
        let request = match &step {
            SetupStep::Mode(mode) => {
                Ok(("session/set_mode", json!({ "sessionId": session, "modeId": mode })))
            }
            SetupStep::Option { id, value, .. } => self.sessions[i]
                .state
                .config_params(session, id, value)
                .map(|params| ("session/set_config_option", params)),
        };
        match request {
            Ok((method, params)) => {
                self.host_request(method, params, HostRequest::Setup(step, setup));
            }
            Err(error) => {
                let error = format!("{}: {error}", step.describe());
                self.setup_failed(setup, Error::new(Code::setting_invalid, error));
            }
        }
    }

    /// The agent's answer to one of `setup`'s settings, `step`.
    fn setup_answered(&mut self, step: SetupStep, mut setup: Setup, msg: &Map<String, Value>) {
        if let Some(error) = msg.get("error") {
            let mut text = format!("{} failed: {}", step.describe(), error_message(error));
            if matches!(setup.of, SetupOf::Start) && is_auth_error(error) {
                text.push_str(&self.auth_hint());
            }
            return self.setup_failed(setup, Error::agent(text, error));
        }
        let result = msg.get("result").cloned().unwrap_or(Value::Null);
        if let Some(i) = self.find(&setup.session) {
            if let SetupStep::Mode(mode) = &step {
                self.sessions[i].state.set_mode(mode);
            }
            // A config option's answer has them all (a mode or model too);
            // what it changed is a `session_changed`.
            self.apply_result(i, &result);
            // The agent answers only the requester (ADR 6.2): an editor is
            // told here (see experimental.rs).
            match &step {
                SetupStep::Mode(mode) => self.mode_set(&setup.session, mode),
                SetupStep::Option { .. } => self.config_set(&setup.session, &result),
            }
        }
        setup.done.push(step);
        self.run_setup(setup);
    }

    /// One of `setup`'s settings failed with `error`: a start fails, a
    /// bridge is told, with what was set before it (P3), and a session a
    /// bridge was opening is closed again.
    fn setup_failed(&mut self, setup: Setup, mut error: Error) {
        if !matches!(setup.of, SetupOf::Start) && !setup.done.is_empty() {
            let done: Vec<String> =
                setup.done.iter().map(|s| s.describe().replacen("setting ", "", 1)).collect();
            error.message.push_str(&format!(" (already set: {})", done.join(", ")));
        }
        match setup.of {
            SetupOf::Start => self.fail_start(&error),
            SetupOf::Config(peer, req_id) => self.reply(peer, req_id, error.reply()),
            SetupOf::Open(o) => self.abandon_open(&setup.session, o.peer, o.req_id, error),
        }
    }

    /// The commit (ADR 3.4): brnr session new hears of the session before the
    /// agent gets any work, and if the report can't be written to it, nobody
    /// knows this session exists. Then the prompt goes.
    fn finish_start(&mut self, i: usize) {
        if self.stop_requested {
            return; // The start already failed (timed out), or was stopped.
        }
        let session = self.sessions[i].id.clone();
        let prompted = self.start.as_ref().is_some_and(|s| s.prompt.is_some());
        let message = prompted.then(|| self.message_id());
        if !self.report_ready(&session, message.as_deref()) {
            return;
        }
        let prompt = self.start.take().and_then(|s| s.prompt);
        self.start_done = true;
        if self.foreground {
            self.say(format!("brnr: session {}", crate::render::clean(&session)));
        }
        if let (Some(prompt), Some(id)) = (prompt, message) {
            self.send_prompt(i, Held { id, text: prompt.text, blocks: prompt.blocks });
        }
    }

    pub(super) fn fail_start(&mut self, error: &Error) {
        self.sessions.unclaim_all();
        self.sink.note(None, json!({ "event": "start_failed", "error": error.json() }));
        self.startup_failed(error);
        self.begin_stop();
    }

    /// Reports `error` to brnr session new if it is still waiting, and in the
    /// foreground on stderr: a step after the session opened (its mode, a
    /// config option) fails the start as much as opening it does. It ends
    /// with the agent's last lines on stderr (ADR 3.6).
    pub(super) fn startup_failed(&mut self, error: &Error) {
        let error = error.clone().map(|m| self.with_stderr(&m));
        if self.foreground && !self.start_done && !self.startup_reported {
            self.startup_reported = true;
            let said = crate::render::coded(&error.message, error.code.name());
            self.say(format!("brnr: {}", crate::render::clean(&said)));
        }
        self.report_failure(&error);
    }

    /// The agent needs a login, which a headless host can't do: say how.
    fn auth_hint(&self) -> String {
        // By id too: that is what --auth takes (ADR 4.7).
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

    /// The agent's answer to a bridge's `new` or `resume` (ADR 8.1): the
    /// session opens as a start's does, locked (ADR 3.9), and then its
    /// settings are set. Until it commits it takes no requests (see
    /// `Session::opening`).
    fn peer_opened(&mut self, open: Open, opening: Box<Opening>, msg: &Map<String, Value>) {
        let (what, asked) = match open {
            Open::New => ("session/new", None),
            Open::Resume(session) => ("session/resume", Some(session)),
            Open::Load(session) => ("session/load", Some(session)),
        };
        if let Some(error) = msg.get("error") {
            let reply = Error::agent(format!("{what} failed: {}", error_message(error)), error);
            let reply = reply.reply();
            // Its lock, taken as it opened, goes, and the bridge is answered,
            // once what was recorded of it (a load's replay, ADR 5.6) is
            // written, as a close's are: whoever resumes it next appends
            // after that (ADR 5.5).
            let mut answer = reply.clone();
            if let Some(req_id) = opening.req_id.clone() {
                answer["req_id"] = req_id;
            }
            if !asked.is_some_and(|s| self.open_failed(&s, Some((opening.peer, answer)))) {
                self.reply(opening.peer, opening.req_id, reply);
            }
            return self.stop_if_empty();
        }
        let result = msg.get("result").cloned().unwrap_or(Value::Null);
        let Some(session) = asked.or_else(|| new_session(&result)) else {
            let error = Error::from(format!("{what} returned no sessionId"));
            self.reply(opening.peer, opening.req_id, error.reply());
            return self.stop_if_empty();
        };
        let i = self.open_session(&session, Some(&opening.cwd.to_string_lossy()));
        if let Some(why) = self.sessions[i].not_owned() {
            self.close_session(i, Ending::NotOwned, None);
            let error = why.map(|why| format!("the agent opened {session}, which {why}"));
            return self.abandon_open(&session, opening.peer, opening.req_id, error);
        }
        self.sessions[i].opening = true;
        self.end_replay(i);
        self.sessions[i].state.result(&result);
        // The process's profile's settings, under the bridge's (ADR 6.1).
        match resolve_settings(&opening.settings, &self.defaults, &self.sessions[i].state) {
            Ok(steps) => {
                self.run_setup(Setup { session, steps, done: vec![], of: SetupOf::Open(opening) })
            }
            Err(error) => self.abandon_open(&session, opening.peer, opening.req_id, error),
        }
    }

    /// The commit of a bridge's `new` or `resume`, as a start's ready report
    /// is (ADR 3.4): the bridge is answered with the session and the message
    /// its prompt will be, and then the prompt goes. A bridge gone before its
    /// answer is queued (it gave up, or was stopped) is nobody to tell of the
    /// session, which is closed again; one that goes after leaves it running,
    /// as after any commit.
    fn commit_open(&mut self, i: usize, opening: Box<Opening>) {
        let session = self.sessions[i].id.clone();
        let Opening { peer, req_id, prompt, .. } = *opening;
        let message = prompt.is_some().then(|| self.message_id());
        if self.peers.contains_key(&peer) {
            let pid = std::process::id();
            let ready = json!({ "ok": true, "pid": pid, "session": session, "message": message });
            self.reply(peer, req_id.clone(), ready);
        }
        // Not there, or dropped as its answer was queued.
        if !self.peers.contains_key(&peer) {
            self.sink.note(Some(&session), json!({ "event": "open_abandoned" }));
            let error = Error::from("nobody is waiting for it");
            return self.abandon_open(&session, peer, req_id, error);
        }
        self.sessions[i].opening = false;
        if let (Some(prompt), Some(id)) = (prompt, message) {
            self.send_prompt(i, Held { id, text: prompt.text, blocks: prompt.blocks });
        }
    }

    /// Why a bridge's `new` or `resume` can't go on to commit: the process
    /// is stopping, so the session would end unused with it, or the
    /// command's timeout has passed (ADR 3.4).
    fn open_refused(&self, opening: &Opening) -> Option<Error> {
        if self.stop_requested || self.agent_in.is_none() {
            return Some(Error::new(Code::process_stopping, "the process is stopping"));
        }
        if opening.deadline.is_some_and(|t| Instant::now() >= t) {
            return Some(timed_out());
        }
        None
    }

    /// A bridge's `new` or `resume` failed with `error` after the agent
    /// opened `session`: it is closed again (`session/close`), and the bridge
    /// is answered once it is, saying so. An agent that can't close sessions
    /// keeps it open, and the answer names it (P3). A stopping process can't
    /// tell the agent any more: the session ends with it.
    fn abandon_open(&mut self, session: &str, peer: u64, req_id: Option<Value>, error: Error) {
        let event = json!({ "event": "open_failed", "session": session, "error": error.json() });
        self.sink.note(None, event);
        if self.agent_in.is_none() {
            return self.reply(peer, req_id, error.reply());
        }
        if !self.caps.close {
            if let Some(i) = self.find(session) {
                self.sessions[i].opening = false; // Served as any other is.
            }
            let pid = std::process::id();
            let error = error.map(|e| {
                format!("{e}; the agent can't close sessions, so {session} is left open in process {pid}")
            });
            return self.reply(peer, req_id, error.reply());
        }
        if let Some(i) = self.find(session) {
            self.sessions[i].closing = Some(Close::Sent);
        }
        let op = PeerOp::Close { session: session.to_owned(), by: "close", failed: Some(error) };
        self.peer_op(peer, req_id, op, "session/close", json!({ "sessionId": session }));
    }

    /// A headless process stops with its last session, unless a bridge's
    /// `new` or `resume` is still waiting for the agent to open one.
    fn stop_if_empty(&mut self) {
        if self.sessions.is_empty() && !self.outbound.opening() && !self.editor_attached() {
            self.begin_stop();
        }
    }

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
                let said = error_message(error);
                let mut failed_too = None;
                if let PeerOp::Close { session, failed, .. } = &op {
                    // One open again since isn't closing (see `peer_result`).
                    let closing =
                        |i: &usize| matches!(self.sessions[*i].closing, Some(Close::Sent));
                    if let Some(i) = self.find(session).filter(closing) {
                        let s = &mut self.sessions[i];
                        s.closing = None; // Still open: it takes requests again.
                        s.opening = false;
                        self.close_failed(session, &said);
                    }
                    // What failed first is the request's failure (ADR 8.7).
                    failed_too = failed.clone().map(|failed| {
                        let pid = std::process::id();
                        failed.map(|f| {
                            format!(
                                "{f}; closing {session} failed too ({said}), so it is left open \
                                 in process {pid}"
                            )
                        })
                    });
                    // Nobody to answer, as for an editor's session this
                    // process doesn't serve (`not_served`): the host log
                    // says so (P3).
                    if peer == 0
                        && let Some(error) = &failed_too
                    {
                        let event = json!({ "event": "open_failed", "session": session, "error": error.json() });
                        self.sink.note(None, event);
                    }
                }
                failed_too.unwrap_or_else(|| Error::agent(said, error)).reply()
            }
            None => {
                let result = msg.get("result").unwrap_or(&Value::Null);
                let Some(reply) = self.peer_result(peer, req_id.clone(), op, result) else {
                    return; // Answered once the session is let go of.
                };
                reply
            }
        };
        self.reply(peer, req_id, reply);
    }

    /// The answer to `peer`, or `None` for a close, answered as its session
    /// is let go of (see `close_session`).
    fn peer_result(
        &mut self,
        peer: u64,
        req_id: Option<Value>,
        op: PeerOp,
        result: &Value,
    ) -> Option<Value> {
        Some(match op {
            PeerOp::Fork { cwd } => {
                let Some(session) = new_session(result) else {
                    return Some(Error::from("session/fork returned no sessionId").reply());
                };
                let i = self.open_session(&session, Some(&cwd.to_string_lossy()));
                // Its lock, taken as it opened: one another process holds, or
                // one that can't be locked, can't be served here (ADR 3.9).
                // Nothing to let go of once written (ADR 5.5): this process
                // took no lock on it, and opened no transcript for it.
                if let Some(why) = self.sessions[i].not_owned() {
                    self.close_session(i, Ending::NotOwned, None);
                    let error =
                        why.map(|why| format!("the agent forked into {session}, which {why}"));
                    return Some(error.reply());
                }
                self.sessions[i].state.result(result);
                json!({ "ok": true, "session": session })
            }
            PeerOp::Close { session, by, failed } => {
                let mut reply = match &failed {
                    Some(failed) => {
                        failed.clone().map(|f| format!("{f}; {session} was closed")).reply()
                    }
                    None => json!({ "ok": true, "session": session }),
                };
                // One open here that isn't closing opened again since: an
                // editor's load, after a close of one this process didn't
                // serve (`not_served`), isn't closed by it.
                let closing = |i: &usize| matches!(self.sessions[*i].closing, Some(Close::Sent));
                let Some(i) = self.find(&session).filter(closing) else {
                    self.stop_if_empty();
                    return Some(reply);
                };
                if let Some(req_id) = req_id {
                    reply["req_id"] = req_id;
                }
                // Once its records are written: whoever resumes it next
                // appends after them (ADR 5.5).
                self.close_session(i, Ending::Closed(by), Some((peer, reply)));
                self.stop_if_empty();
                return None;
            }
        })
    }
}

/// What the agent can do, as it said in `initialize`: for status, and for
/// brnr to check up front. What is stable ACP is read as the schema has it;
/// `fork` is unstable, and `steering` a convention advertised in `_meta`
/// (ADR 2.1), so both are read from the result as it came.
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

/// The settings of a start or of `config set`, resolved against session
/// `state`'s config options into what is sent, in order (ADR 6.1, ADR 8.1):
/// `flags` over `profile`, setting by setting, `profile` being empty for
/// `config set`.
///
/// `mode`, `model` and `thought_level` find their option by category
/// (ADR 6.2), so `--model` and an option set by the model option's id are
/// one setting; `options` are by id. A mode is `session/set_mode` only for
/// an agent with v1 modes and no mode option. Two values for one setting
/// from one source fail (P4); a mode, model or thought level the agent has
/// no option for, a v1 mode it doesn't list, or a value its option's type
/// doesn't take fails before any is sent (P7). An option by id the agent
/// hasn't advertised is sent, its value as a value id, for the agent to
/// take or refuse (ADR 6.2). The mode goes first, then the model, the
/// thought level, and the other options by id.
pub(super) fn resolve_settings(
    flags: &Settings,
    profile: &Settings,
    state: &SessionState,
) -> Result<VecDeque<SetupStep>, Error> {
    resolve(flags, profile, state).map_err(|e| Error::new(Code::setting_invalid, e))
}

/// [`resolve_settings`], its failure said.
fn resolve(
    flags: &Settings,
    profile: &Settings,
    state: &SessionState,
) -> Result<VecDeque<SetupStep>, String> {
    let id = |category| state.option(category).and_then(|o| o["id"].as_str()).map(str::to_owned);
    let ids = [id("mode"), id("model"), id("thought_level")];
    let flags = Resolved::of(flags, &ids, None)?;
    let profile = Resolved::of(profile, &ids, Some(&flags))?;
    // What the flags set wins, setting by setting.
    let won = |n: usize| flags.by_category[n].clone().or_else(|| profile.by_category[n].clone());
    let mut steps = VecDeque::new();
    if let Some(mode) = won(0) {
        steps.push_back(match (&ids[0], &state.modes) {
            (Some(id), _) => {
                SetupStep::Option { id: id.clone(), value: mode, category: Some("mode") }
            }
            (None, Some(modes)) => {
                let listed = modes["availableModes"].as_array();
                if listed.is_some_and(|m| !m.iter().any(|x| x["id"] == mode.as_str())) {
                    return Err(format!("setting mode {mode}: the agent has no mode {mode}"));
                }
                SetupStep::Mode(mode)
            }
            (None, None) => return Err(format!("setting mode {mode}: the agent offers no modes")),
        });
    }
    for (n, category, offers) in
        [(1, "model", "model choice"), (2, "thought_level", "thought level")]
    {
        let Some(value) = won(n) else { continue };
        let Some(id) = ids[n].clone() else {
            let what = category.replace('_', " ");
            return Err(format!("setting {what} {value}: the agent offers no {offers}"));
        };
        steps.push_back(SetupStep::Option { id, value, category: Some(category) });
    }
    let mut options = profile.options;
    options.extend(flags.options);
    let by_id =
        options.into_iter().map(|(id, value)| SetupStep::Option { id, value, category: None });
    steps.extend(by_id);
    // A value its option's type doesn't take (ADR 6.2) fails before any of
    // them is sent.
    for step in &steps {
        if let SetupStep::Option { id, value, .. } = step {
            state.config_params("", id, value).map_err(|e| format!("{}: {e}", step.describe()))?;
        }
    }
    Ok(steps)
}

/// One source's settings, with its options of category `mode`, `model` and
/// `thought_level` taken as those settings.
struct Resolved {
    /// The mode, the model and the thought level.
    by_category: [Option<String>; 3],
    options: BTreeMap<String, String>,
}

impl Resolved {
    /// `ids`: the agent's mode, model and thought level options. `over`:
    /// the flags, when these are the profile's settings: what the flags set,
    /// the profile's values for are neither applied nor checked.
    fn of(
        s: &Settings,
        ids: &[Option<String>; 3],
        over: Option<&Resolved>,
    ) -> Result<Resolved, String> {
        let mut options = s.options.clone();
        let values = [&s.mode, &s.model, &s.thought_level];
        let names = [("mode", "mode"), ("model", "model"), ("thought-level", "thought_level")];
        let mut by_category = [None, None, None];
        for (n, (flag, key)) in names.into_iter().enumerate() {
            let other = ids[n].as_ref().and_then(|id| Some((id, options.remove(id)?)));
            if over.is_some_and(|o| o.by_category[n].is_some()) {
                continue;
            }
            by_category[n] = match (values[n], other) {
                (Some(value), Some((id, other))) if *value != other => {
                    let what = key.replace('_', " ");
                    return Err(match over {
                        Some(_) => format!(
                            "the profile's {key} {value} and its options {id}={other} both set \
                             the {what}"
                        ),
                        None => {
                            format!(
                                "--{flag} {value} and --option {id}={other} both set the {what}"
                            )
                        }
                    });
                }
                (Some(value), _) => Some(value.clone()),
                (None, other) => other.map(|(_, v)| v),
            };
        }
        Ok(Resolved { by_category, options })
    }
}

/// A start or an opening that didn't commit in its time (ADR 3.4).
pub(super) fn timed_out() -> Error {
    Error::new(Code::start_timed_out, "timed out waiting for the session")
}

/// ACP's `auth_required`, or an agent that says as much.
fn is_auth_error(error: &Value) -> bool {
    schema::read::<AcpError>(error).is_some_and(|e| {
        e.code == ErrorCode::AuthRequired || e.message.to_lowercase().contains("auth")
    })
}

/// The `history` event of `session`'s `replay`, ended (ADR 5.6).
pub(super) fn history(session: &str, replay: &Replay) -> Value {
    json!({
        "event": "history",
        "session": session,
        "updates": replay.updates,
        "recorded": replay.record,
        "recorded_updates": replay.recorded_updates,
        "loads": replay.loads,
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

    #[test]
    fn adr_0006_a_start_refuses_a_boolean_it_cant_send_before_sending_any() {
        let state = SessionState {
            config: Some(json!([
                { "id": "model", "category": "model", "type": "select", "currentValue": "small" },
                { "id": "fast", "type": "boolean", "currentValue": false },
            ])),
            ..SessionState::default()
        };
        let flags = |fast: &str| Settings {
            model: Some("large".into()),
            options: BTreeMap::from([("fast".into(), fast.into())]),
            ..Settings::default()
        };
        let resolve = |flags| resolve_settings(&flags, &Settings::default(), &state);
        let steps: Vec<_> = resolve(flags("true")).unwrap().into_iter().collect();
        let want = [
            SetupStep::Option {
                id: "model".into(),
                value: "large".into(),
                category: Some("model"),
            },
            SetupStep::Option { id: "fast".into(), value: "true".into(), category: None },
        ];
        assert_eq!(steps, want);
        let error = resolve(flags("on")).unwrap_err();
        let said = "setting fast=on: fast is a boolean option: true or false, not on";
        assert_eq!(error, Error::new(Code::setting_invalid, said));
    }
}
