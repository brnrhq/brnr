//! The host's ACP stream without its processes, for the fuzz targets
//! (fuzz/ in the repository) and tests: lines from the editor and the agent
//! go through the host's own handling (see acp.rs), and what it writes to
//! each side is collected instead of sent.
//!
//! There is no agent, no link, no log and no peer: nothing is spawned or
//! signalled (the stop timer, the one thing that signals the agent, never
//! fires here). What the host does to the filesystem it still does: an
//! editor's `session/load` takes the session's lock in the runtime
//! directory (`$BRNR_DIR`, see paths.rs), held until the harness drops.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use super::Host;
use crate::config::{Experimental, Feature, Log};
use crate::request::{Editor, Headless, Prompt, Role};

/// How far the clock moves at each [`Harness::tick`]: past every timeout
/// the harness sets.
const TICK: Duration = Duration::from_secs(3600);

pub struct Harness {
    host: Host,
    agent: Receiver<Vec<u8>>,
    editor: Receiver<(u8, Vec<u8>)>,
    now: Instant,
}

/// What the host sent each side, since it was last asked.
#[derive(Default)]
pub struct Sent {
    /// Bytes for the agent's stdin.
    pub agent: Vec<Vec<u8>>,
    /// Frames for the proxy: kind and payload (see frame.rs).
    pub editor: Vec<(u8, Vec<u8>)>,
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
            mode: Some("plan".into()),
            prompt: Some(Prompt { text: "hello".into(), blocks: Vec::new() }),
            stop_when_idle: Some(1),
            permission_timeout: Some(1),
            ..Headless::default()
        };
        let mut harness = Harness::new(Role::Headless(headless), strict);
        harness.host.begin_headless_start();
        harness
    }

    fn new(role: Role, strict: bool) -> Harness {
        let (_, rx) = mpsc::sync_channel(1);
        // No agent to signal: a pid no process can have.
        let mut host = Host::new(role, PathBuf::from("/"), strict, Log::Off, libc::pid_t::MAX, rx);
        let (agent_in, agent) = mpsc::channel();
        host.agent_in = Some(agent_in);
        let mut editor = None;
        if host.editor {
            let (link, rx) = mpsc::channel();
            (host.link, host.start_done, editor) = (Some(link), true, Some(rx));
        }
        let editor = editor.unwrap_or_else(|| mpsc::channel().1);
        Harness { host, agent, editor, now: Instant::now() }
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

    /// Time passes: unanswered permission requests and idle sessions time
    /// out.
    pub fn tick(&mut self) {
        self.now += TICK;
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
        Sent { agent, editor }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame;

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
}
