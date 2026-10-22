//! The host's ACP stream without its processes, for the fuzz targets
//! (fuzz/ in the repository) and tests: lines from the editor and the agent
//! go through the host's own handling (see acp.rs), and what it writes to
//! each side is collected instead of sent. Headless start reports are
//! collected too, so the start commits before its prompt goes (ADR 3.4).
//!
//! There is no agent, no link, no log and no peer: nothing is spawned or
//! signalled (the stop timer, the one thing that signals the agent, never
//! fires here). What the host does to the filesystem it still does: an
//! editor's `session/load` takes the session's lock in the state directory
//! (`$BRNR_HOME`, see paths.rs, ADR 3.9), held until the harness drops.

use std::collections::BTreeSet;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use serde_json::Value;

use super::Host;
use crate::config::{ClientCapability, EchoAs, Experimental, Feature};
use crate::request::{Editor, Headless, Prompt, Role, Settings};

/// How far the clock moves at each [`Harness::tick`]: past every timeout
/// the harness sets.
const TICK: Duration = Duration::from_secs(3600);

pub struct Harness {
    host: Host,
    agent: Receiver<Vec<u8>>,
    editor: Receiver<(u8, Vec<u8>)>,
    start: Receiver<Value>,
    now: Instant,
}

/// What the host sent each side, since it was last asked.
#[derive(Default)]
pub struct Sent {
    /// Bytes for the agent's stdin.
    pub agent: Vec<Vec<u8>>,
    /// Frames for the proxy: kind and payload (see frame.rs).
    pub editor: Vec<(u8, Vec<u8>)>,
    /// Reports for the headless start channel, including the commit.
    pub start: Vec<Value>,
}

impl Harness {
    /// An editor's process: everything experimental allowed, and shared
    /// sessions if `shared`.
    pub fn editor(strict: bool, shared: bool) -> Harness {
        let features =
            if shared { BTreeSet::from([Feature::SharedSessions]) } else { BTreeSet::new() };
        let editor = Editor {
            proxy_pid: 0,
            sigmask: Vec::new(),
            experimental: Experimental::ALL.into_iter().collect(),
            client_capabilities: ClientCapability::ALL.into_iter().collect(),
            echo_as: EchoAs::ToolCall,
            features,
        };
        Harness::new(Role::Editor(editor), strict)
    }

    /// A headless process, its start begun (the host has sent `initialize`):
    /// it resumes `resume` or opens a session, then sets a mode and sends a
    /// prompt, and its permission requests and idle sessions time out.
    pub fn headless(strict: bool, resume: Option<String>) -> Harness {
        let headless = Headless {
            resume,
            settings: Settings { mode: Some("plan".into()), ..Settings::default() },
            prompt: Some(Prompt { text: "hello".into(), blocks: Vec::new() }),
            stop_when_idle: Some(1),
            response_timeout: Some(1),
            ..Headless::default()
        };
        let mut harness = Harness::new(Role::Headless(Box::new(headless)), strict);
        harness.host.begin_headless_start();
        harness
    }

    fn new(role: Role, strict: bool) -> Harness {
        let (host, ends) = Host::simulated(role, strict);
        Harness {
            host,
            agent: ends.agent,
            editor: ends.editor,
            start: ends.start,
            now: Instant::now(),
        }
    }

    /// Bytes from the editor, as the link delivers them: lines may be split
    /// across calls.
    pub fn editor_bytes(&mut self, bytes: &[u8]) {
        self.host.editor_bytes(bytes);
    }

    /// The editor closing its stdin, after what it wrote last.
    pub fn editor_eof(&mut self) {
        self.host.editor_eof();
    }

    /// One line of the agent's stdout, with its `\n` unless it was the last.
    pub fn agent_line(&mut self, line: &[u8]) {
        self.host.agent_line(line);
    }

    /// The agent's stdout ending, after its last line.
    pub fn agent_stdout_eof(&mut self) {
        self.host.handle(super::Ev::AgentStdoutEof);
    }

    /// Bytes of the agent's stderr, as a read returned them.
    pub fn agent_stderr(&mut self, bytes: &[u8]) {
        self.host.handle(super::Ev::AgentStderr(bytes.to_vec()));
    }

    /// A bridge's `send`, `cancel`, `queue` or `status` (see control.rs),
    /// from a peer that isn't there: the answer it would get, or its
    /// `error` (ADR 8.7). Other commands start or stop processes, and aren't
    /// taken.
    pub fn bridge(&mut self, req: &Value) -> Result<Value, Value> {
        if !matches!(req["cmd"].as_str(), Some("send" | "cancel" | "queue" | "status")) {
            return Err(crate::error::Error::from("not a command the harness takes").json());
        }
        self.host.command(0, req).map(Option::unwrap_or_default).map_err(|e| e.json())
    }

    /// `session` is this process's before the agent opens it, as if its lock
    /// had been taken (ADR 3.9), with no lock file: for the unit tests, which
    /// have no state directory of their own.
    #[cfg(test)]
    pub(super) fn claim(&mut self, session: &str) {
        self.host.sessions.claim(session, super::sessions::Hold::Owner(None));
    }

    /// The host, for the unit tests to look at what it holds.
    #[cfg(test)]
    pub(super) fn host(&mut self) -> &mut Host {
        &mut self.host
    }

    /// Time passes: unanswered permission requests, idle sessions and a
    /// start that hasn't committed time out.
    pub fn tick(&mut self) {
        self.now += TICK;
        self.host.fire_start_timer(self.now);
        self.host.fire_permission_timers(self.now);
        self.host.fire_idle_timers(self.now);
    }

    /// What the host has sent since the last call.
    pub fn sent(&mut self) -> Sent {
        let editor: Vec<_> = self.editor.try_iter().collect();
        for (_, payload) in &editor {
            self.host.from_agent.done(payload.len());
        }
        let agent: Vec<_> = self.agent.try_iter().collect();
        for bytes in &agent {
            self.host.to_agent.done(bytes.len());
        }
        Sent { agent, editor, start: self.start.try_iter().collect() }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;
    use crate::{frame, json};

    #[test]
    fn the_harness_relays_both_ways() {
        let mut h = Harness::editor(false, false);
        h.editor_bytes(
            b"{\"jsonrpc\":\"2.0\",\"id\":0,\"method\":\"initialize\",\"params\":{}}\nnot js",
        );
        h.editor_bytes(b"on\n");
        let sent = h.sent();
        assert_eq!(sent.agent.len(), 2);
        assert_eq!(sent.agent[1], b"not json\n");
        h.agent_line(b"garbled\n");
        assert_eq!(h.sent().editor, vec![(frame::DATA, b"garbled\n".to_vec())]);

        let mut h = Harness::headless(false, None);
        let sent = h.sent();
        assert!(
            sent.agent[0].starts_with(br#"{"jsonrpc":"2.0","id":"brnr-1","method":"initialize""#)
        );
        h.agent_line(
            b"{\"jsonrpc\":\"2.0\",\"id\":\"brnr-1\",\"result\":{\"protocolVersion\":1}}\n",
        );
        assert!(h.sent().agent[0].windows(11).any(|w| w == b"session/new"));
        h.tick();
    }

    #[test]
    fn adr_0002_brnrs_own_lines_end_with_the_agents_stdout() {
        let mut h = Harness::editor(false, false);
        let update = serde_json::json!({ "sessionUpdate": "agent_message_chunk" });
        h.host.update_editor("sess-1", update.clone());
        assert_eq!(h.sent().editor.len(), 1);
        h.agent_line(b"last\n");
        h.agent_stdout_eof();
        // A message sent from outside, after: nothing follows the end.
        h.host.update_editor("sess-1", update);
        let sent = h.sent().editor;
        assert_eq!(sent, vec![(frame::DATA, b"last\n".to_vec()), (frame::EOF, Vec::new())]);
        // Stderr and the exit status still go.
        h.agent_stderr(b"x");
        assert_eq!(h.sent().editor, vec![(frame::STDERR, b"x".to_vec())]);
    }

    #[test]
    fn adr_0002_stderr_goes_on_before_its_line_ends() {
        let mut h = Harness::headless(false, None);
        let (link, editor) = mpsc::channel();
        h.host.link = Some(link);
        h.agent_stderr(b"first\nlogin ");
        h.agent_stderr(b"required: ");
        let sent: Vec<_> = editor.try_iter().collect();
        let want =
            [(frame::STDERR, b"first\nlogin ".to_vec()), (frame::STDERR, b"required: ".to_vec())];
        assert_eq!(sent, want);
        // The line it is in the middle of ends a failed start's error too.
        let error = h.host.with_stderr("timed out");
        assert!(error.ends_with("stderr:\n  first\n  login required: "), "{error}");
        h.agent_stderr(b"ok\n");
        assert!(h.host.with_stderr("timed out").ends_with("\n  login required: ok"));
    }

    #[test]
    fn adr_0002_the_editors_stdin_ends_with_the_agents() {
        let ask = |id: u64| format!(r#"{{"jsonrpc":"2.0","id":{id},"method":"_ask"}}"#);
        // A stop closes the agent's stdin: the proxy is told to close the
        // editor's, and what comes before it does goes nowhere, as does the
        // start of the line the editor was writing.
        let mut h = Harness::editor(false, false);
        h.editor_bytes(format!("{}\n{{\"jsonrpc\"", ask(1)).as_bytes());
        assert_eq!(h.sent().agent.len(), 1);
        h.host.begin_stop();
        assert_eq!(h.sent().editor, vec![(frame::STDIN_CLOSED, Vec::new())]);
        h.editor_bytes(format!(":\"2.0\"}}\n{}\n", ask(2)).as_bytes());
        h.editor_eof();
        let sent = h.sent();
        assert!(sent.agent.is_empty() && sent.editor.is_empty());
        assert!(!h.host.outbound.waiting(r#""brnr-2""#));

        // The agent closing it: the line that finds it closed went before
        // anyone knew, those after it in the same read don't.
        let mut h = Harness::editor(false, false);
        h.agent = mpsc::channel().1;
        h.editor_bytes(format!("{}\n{}\n", ask(1), ask(2)).as_bytes());
        assert_eq!(h.sent().editor, vec![(frame::STDIN_CLOSED, Vec::new())]);
        assert!(!h.host.outbound.waiting(r#""brnr-2""#));
        h.editor_bytes(format!("{}\n", ask(3)).as_bytes());
        let sent = h.sent();
        assert!(sent.agent.is_empty() && sent.editor.is_empty());
    }

    /// A line `depth` deep: a JSON-RPC request `id` for `method`, whose tool
    /// call's input nests the rest of the way.
    fn deep(id: &str, method: &str, depth: usize) -> Vec<u8> {
        let n = depth - 3;
        let input = format!("{}{}", "[".repeat(n), "]".repeat(n));
        let params =
            format!(r#"{{"sessionId":"s","toolCall":{{"toolCallId":"t","rawInput":{input}}}}}"#);
        let line =
            format!(r#"{{"jsonrpc":"2.0","id":"{id}","method":"{method}","params":{params}}}"#);
        assert_eq!(json::depth(line.as_bytes()), depth);
        line.into_bytes()
    }

    #[test]
    fn adr_0002_the_host_holds_nothing_of_a_line_past_the_limit() {
        const ASK: &str = "session/request_permission";
        let mut h = Harness::editor(false, false);
        // At the limit, the agent's request is read, and held until the
        // editor answers it; the editor's goes with an id of the host's.
        let mut ask = deep("r1", ASK, json::DEPTH);
        ask.push(b'\n');
        h.agent_line(&ask);
        assert_eq!(h.sent().editor, vec![(frame::DATA, ask.clone())]);
        assert_eq!(h.host.approvals.held().count(), 1);
        let mut asked = deep("e1", "_ask", json::DEPTH);
        asked.push(b'\n');
        h.editor_bytes(&asked);
        let sent = h.sent().agent;
        assert!(sent[0].starts_with(br#"{"jsonrpc":"2.0","id":"brnr-1","#), "not rewritten");
        assert!(h.host.outbound.waiting(r#""brnr-1""#));
        // Past it, neither is read: each goes on byte for byte, the
        // editor's with its own id, and neither is held.
        let mut ask = deep("r2", ASK, json::DEPTH + 1);
        ask.push(b'\n');
        h.agent_line(&ask);
        assert_eq!(h.sent().editor, vec![(frame::DATA, ask)]);
        assert_eq!(h.host.approvals.held().count(), 1);
        let mut asked = deep("e2", "_ask", json::DEPTH + 1);
        asked.push(b'\n');
        h.editor_bytes(&asked);
        assert_eq!(h.sent().agent, vec![asked]);
        assert!(!h.host.outbound.waiting(r#""brnr-2""#));
        // A million deep is no different.
        let mut ask = deep("r3", ASK, 1_000_000);
        ask.push(b'\n');
        h.agent_line(&ask);
        assert_eq!(h.sent().editor.len(), 1);
        assert_eq!(h.host.approvals.held().count(), 1);

        // Headless, the agent's is dropped.
        let mut h = Harness::headless(false, None);
        h.sent();
        h.agent_line(&deep("r1", ASK, json::DEPTH + 1));
        let sent = h.sent();
        assert!(sent.agent.is_empty() && sent.editor.is_empty());
        assert_eq!(h.host.approvals.held().count(), 0);
    }

    #[test]
    fn a_headless_start_commits_before_its_prompt() {
        let mut h = Harness::headless(false, None);
        h.sent();
        // No real session lock in this unit test. The fuzz targets use their
        // own state directory for the locks they exercise.
        h.claim("fuzz-start");
        h.agent_line(br#"{"jsonrpc":"2.0","id":"brnr-1","result":{"protocolVersion":1}}"#);
        h.sent();
        h.agent_line(br#"{"jsonrpc":"2.0","id":"brnr-2","result":{"sessionId":"fuzz-start","modes":{"currentModeId":"default","availableModes":[{"id":"plan","name":"Plan"}]}}}"#);
        assert!(h.sent().agent[0].windows(16).any(|w| w == b"session/set_mode"));
        h.agent_line(br#"{"jsonrpc":"2.0","id":"brnr-3","result":{}}"#);
        let sent = h.sent();
        assert_eq!(sent.start.len(), 1);
        assert_eq!(sent.start[0]["ok"], true);
        assert_eq!(sent.start[0]["session"], "fuzz-start");
        assert!(h.host.start_done);
        assert!(sent.agent[0].windows(14).any(|w| w == b"session/prompt"));
    }

    #[test]
    fn adr_0003_a_step_answered_after_the_start_failed_goes_no_further() {
        let init = br#"{"jsonrpc":"2.0","id":"brnr-1","result":{"protocolVersion":1,"authMethods":[{"id":"login","name":"Log in"}]}}"#;
        // The start times out with its `initialize`, or its `authenticate`,
        // unanswered; the answer comes after.
        for (auth, late, next) in [
            (false, &init[..], "brnr-2"),
            (true, br#"{"jsonrpc":"2.0","id":"brnr-2","result":{}}"#, "brnr-3"),
        ] {
            let mut h = Harness::headless(false, None);
            if auth {
                h.host.start.as_mut().unwrap().auth = Some("login".into());
                h.agent_line(init);
                assert!(h.sent().agent[1].windows(12).any(|w| w == b"authenticate"));
            }
            h.tick();
            let sent = h.sent();
            assert_eq!(sent.start.len(), 1);
            let error = serde_json::json!({ "code": "start_timed_out", "message": "timed out waiting for the session" });
            assert_eq!(sent.start[0]["error"], error);
            let seq = h.host.seq;
            h.agent_line(late);
            assert!(h.sent().agent.is_empty(), "the agent was sent a step after the start failed");
            // The answer is recorded, and nothing after it.
            assert_eq!(h.host.seq, seq + 1, "a step was recorded after the start failed");
            assert!(!h.host.outbound.waiting(next), "{next} is waiting for an answer");
        }
    }

    /// The start's session, its prompt `brnr-4` running, with an agent that
    /// steers.
    fn turn_running() -> Harness {
        let mut h = Harness::headless(false, None);
        h.claim("fuzz-send");
        h.agent_line(br#"{"jsonrpc":"2.0","id":"brnr-1","result":{"protocolVersion":1,"_meta":{"steering":{"supported":true}}}}"#);
        h.agent_line(br#"{"jsonrpc":"2.0","id":"brnr-2","result":{"sessionId":"fuzz-send","modes":{"currentModeId":"default","availableModes":[{"id":"plan","name":"Plan"}]}}}"#);
        h.agent_line(br#"{"jsonrpc":"2.0","id":"brnr-3","result":{}}"#);
        h.sent();
        h
    }

    #[test]
    fn adr_0004_a_bridge_sends_through_the_sessions_turns() {
        let mut h = turn_running();
        let send = |text: &str, mode: &str| serde_json::json!({ "cmd": "send", "session": "fuzz-send", "text": text, "mode": mode });
        assert_eq!(h.bridge(&send("later", "prompt")).unwrap()["status"], "held");
        assert_eq!(h.bridge(&send("now", "steer")).unwrap()["status"], "steered");
        let sent = h.sent().agent;
        assert!(sent.len() == 1 && sent[0].windows(17).any(|w| w == b"_session/steering"));
        // The turn's answer, then the steer's: only then does the held one go.
        h.agent_line(br#"{"jsonrpc":"2.0","id":"brnr-4","result":{"stopReason":"end_turn"}}"#);
        assert!(h.sent().agent.is_empty(), "a prompt went while a steer was unanswered");
        let status = h.bridge(&serde_json::json!({ "cmd": "status" })).unwrap();
        assert_eq!(
            (&status["sessions"][0]["state"], &status["sessions"][0]["held_count"]),
            (&"idle".into(), &1.into())
        );
        h.agent_line(br#"{"jsonrpc":"2.0","id":"brnr-5","result":{"outcome":"injected"}}"#);
        let sent = h.sent().agent;
        assert!(sent.len() == 1 && sent[0].windows(5).any(|w| w == b"later"));
        let queue = serde_json::json!({ "cmd": "queue", "session": "fuzz-send", "drop": "m1" });
        let none =
            serde_json::json!({ "code": "message_not_found", "message": "no held message m1" });
        assert_eq!(h.bridge(&queue).err(), Some(none));
        assert!(h.bridge(&serde_json::json!({ "cmd": "stop" })).is_err());
    }
}
