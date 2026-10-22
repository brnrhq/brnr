//! The start channel (ADR 3.4 in docs/adr): brnr session new's (or `resume`'s)
//! end of a socketpair, at fd 3.
//!
//! Whatever fails before the start commits is reported on it, `{ok: false,
//! error}`, and stops the process. The commit is the ready report, `{ok:
//! true, pid, session, message}`, written once the session is open and its
//! mode and config options are set, and before the agent gets the prompt
//! (see `finish_start`; `message` is the prompt's `m<n>`). Until then the
//! channel isn't a peer and nothing else is written on it, so either report
//! is written there and then, by the event loop, and the commit is that
//! write succeeding: one that fails (brnr session new has gone, though its EOF
//! may not have reached the event loop yet) abandons the start. A write that
//! succeeds is as far as the host can know: brnr session new may still go
//! before it reads the report, and the session then carries on, as after any
//! commit. Committed, the channel becomes a peer, subscribed with `--wait`
//! to the events brnr session new follows its turn by, before the prompt goes,
//! so it misses none of them. The report names it as a peer, with the last
//! event before it, as a subscription's answer does, to catch up from should
//! it be cut off (ADR 10.2).
//!
//! brnr session new never writes on it; the host reads it on a thread for EOF.
//! brnr session new gone before the commit means nobody knows of the session:
//! the process stops at once (`start_abandoned`). After the commit the session
//! carries on, whatever brnr session new does (P14).

use std::env;
use std::io::{self, ErrorKind, Read, Write};
use std::mem::take;
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::sync::LazyLock;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::mpsc::SyncSender;
#[cfg(any(test, feature = "internals"))]
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::control::{self, Closer, Peer, Queue};
use super::{Ev, Host};
use crate::error::Error;
use crate::request::{Headless, Prompt, Settings};

/// The start channel's name as a peer.
const START_PEER: &str = "socket#start";

/// A headless start, from the request (ADR 3.3) until it commits or fails
/// (ADR 3.4): what only it uses, which goes with it.
pub(super) struct Start {
    /// brnr session new's start channel, once it is open.
    channel: Option<StartChannel>,
    /// When the start fails if it hasn't committed: the request's start
    /// timeout.
    pub(super) deadline: Option<Instant>,
    /// The prompt, sent once the start has committed.
    pub(super) prompt: Option<Prompt>,
    /// The login method to run before the session opens (ADR 4.7).
    pub(super) auth: Option<String>,
    /// Resume this session instead of opening a new one.
    pub(super) resume: Option<String>,
    /// brnr has a transcript of the session resumed, so a load's replay
    /// isn't recorded again (ADR 5.6).
    pub(super) transcript: bool,
    /// What its flags set, over the profile's (ADR 6.1).
    pub(super) flags: Settings,
}

impl Start {
    /// The start `headless` asks for, its timeout counting from now.
    pub(super) fn of(headless: &mut Headless) -> Start {
        Start {
            channel: None,
            deadline: Instant::now().checked_add(Duration::from_secs(headless.start_timeout)),
            prompt: headless.prompt.take(),
            auth: headless.auth.take(),
            resume: headless.resume.take(),
            transcript: headless.transcript,
            flags: take(&mut headless.settings),
        }
    }
}

/// The start channel while the start is under way.
pub(super) enum StartChannel {
    /// A simulated start in the fuzz harness: reports are collected without
    /// sockets, threads or peers, as the agent's input and editor link are.
    #[cfg(any(test, feature = "internals"))]
    Collected(Sender<Value>),
    Socket {
        /// Its id as a peer, once it is one.
        peer: u64,
        /// Where the report is written.
        stream: UnixStream,
        /// To cut it off, once it is a peer.
        closer: UnixStream,
        /// What it is subscribed to once the start commits.
        events: Vec<String>,
    },
}

impl Host {
    /// Reads the start channel for EOF. It becomes a peer, subscribed to
    /// `events`, at the commit (see `report_ready`).
    pub(super) fn open_start_channel(
        &mut self,
        channel: UnixStream,
        events: Vec<String>,
        tx: &SyncSender<Ev>,
    ) -> io::Result<()> {
        let (reader, closer) = (channel.try_clone()?, channel.try_clone()?);
        let peer = control::NEXT_PEER.fetch_add(1, Relaxed);
        let socket = StartChannel::Socket { peer, stream: channel, closer, events };
        if let Some(start) = &mut self.start {
            start.channel = Some(socket);
        }
        let t = tx.clone();
        thread::spawn(move || {
            read_to_eof(reader);
            let _ = t.send(Ev::StartGone { peer });
        });
        Ok(())
    }

    /// The start, and its channel, are let go of, with nothing written to
    /// it: how the start ended is `main`'s to tell brnr session new, on fd
    /// 3, as of a start that fails before the event loop (see
    /// `Host::died_starting`).
    pub(super) fn release_start_channel(&mut self) {
        self.start = None;
    }

    /// brnr session new closed its end of the channel.
    pub(super) fn start_gone(&mut self, peer: u64) {
        self.peers.remove(&peer);
        let channel = self.start.as_ref().and_then(|s| s.channel.as_ref());
        if matches!(channel, Some(StartChannel::Socket { peer: id, .. }) if *id == peer) {
            self.abandon_start();
        }
    }

    fn abandon_start(&mut self) {
        self.start = None;
        self.sink.note(None, json!({ "event": "start_abandoned" }));
        self.begin_stop();
    }

    /// The commit: brnr session new is told the session, and the message its
    /// prompt will be. False if the report can't be written, which abandons
    /// the start: brnr session new has gone, whether or not its EOF has
    /// reached the event loop yet.
    pub(super) fn report_ready(&mut self, session: &str, message: Option<&str>) -> bool {
        let Some(channel) = self.start.as_mut().and_then(|s| s.channel.take()) else {
            return false;
        };
        let mut ready = json!({ "ok": true, "pid": std::process::id(), "session": session });
        if let Some(message) = message {
            ready["message"] = json!(message);
        }
        // Where its events start, as a subscription's answer says (ADR 10.2).
        ready["peer"] = json!(START_PEER);
        ready["host_id"] = json!(self.host_id);
        ready["seq"] = json!(self.seq);
        let (peer, mut stream, closer, events) = match channel {
            StartChannel::Socket { peer, stream, closer, events } => (peer, stream, closer, events),
            #[cfg(any(test, feature = "internals"))]
            StartChannel::Collected(tx) => return tx.send(ready).is_ok(),
        };
        self.test_hold_ready(&stream);
        if write_report(&mut stream, &ready).is_err() {
            self.abandon_start();
            return false;
        }
        let (queue, lines, queued) = Queue::new();
        thread::spawn(move || control::write_lines(stream, lines, queued));
        // A socket peer, as brnr's own connections are: not a bridge.
        let mut p = Peer::new(queue, START_PEER.into(), Closer::Socket(closer));
        (p.subscribed, p.events) = (true, Some(events));
        self.peers.insert(peer, p);
        true
    }

    /// The start's reports, collected rather than written, for the fuzz
    /// harness (see fuzz.rs): none for an editor's process, which has no
    /// start.
    #[cfg(any(test, feature = "internals"))]
    pub(super) fn collect_start(&mut self) -> Receiver<Value> {
        let (tx, rx) = mpsc::channel();
        if let Some(start) = &mut self.start {
            start.channel = Some(StartChannel::Collected(tx));
        }
        rx
    }

    /// Tells brnr session new the start failed, if it is still waiting to
    /// hear: the start goes.
    pub(super) fn report_failure(&mut self, error: &Error) {
        let report = error.reply();
        match self.start.take().and_then(|s| s.channel) {
            Some(StartChannel::Socket { mut stream, .. }) => {
                let _ = write_report(&mut stream, &report);
            }
            #[cfg(any(test, feature = "internals"))]
            Some(StartChannel::Collected(tx)) => {
                let _ = tx.send(report);
            }
            None => {}
        }
    }
}

/// A report, as one line in one write. Nothing else is ever written on the
/// channel before it, so the socket's buffer is empty and it doesn't wait.
fn write_report(stream: &mut UnixStream, report: &Value) -> io::Result<()> {
    stream.write_all(format!("{report}\n").as_bytes())
}

/// `BRNR_TEST_READY=hold` (see [`Host::test_hold_ready`]), read once.
static TEST_HOLD_READY: LazyLock<bool> =
    LazyLock::new(|| env::var_os("BRNR_TEST_READY").is_some_and(|v| v == "hold"));

impl Host {
    /// For the tests, with `BRNR_TEST_READY=hold`: the commit is held, once
    /// the host has decided to report ready and before the report is
    /// written, until brnr session new has gone (for up to 30 s), so that it
    /// goes at the worst moment. `test_ready_held` in the host log says it is
    /// held.
    fn test_hold_ready(&self, stream: &UnixStream) {
        if !*TEST_HOLD_READY {
            return;
        }
        self.sink.note(None, json!({ "event": "test_ready_held" }));
        let until = Instant::now() + Duration::from_secs(30);
        while !closed(stream) && Instant::now() < until {
            thread::sleep(Duration::from_millis(10));
        }
    }
}

/// `BRNR_TEST_START_TIMER=stderr` (see [`Host::test_hold_start_timer`]), read
/// once, the first time the start timer would fire: until when it is held.
static TEST_START_TIMER_UNTIL: LazyLock<Option<Instant>> = LazyLock::new(|| {
    let hold = env::var_os("BRNR_TEST_START_TIMER").is_some_and(|v| v == "stderr");
    hold.then(|| Instant::now() + Duration::from_secs(30))
});

impl Host {
    /// For the tests, with `BRNR_TEST_START_TIMER=stderr`: a start timeout
    /// that has passed fails the start only once the agent has written on
    /// stderr (for up to 30 s more), so that the error has a line to show
    /// however slowly the agent starts. Whether it is held; held, the
    /// deadline moves 10 ms on, so the event loop sleeps rather than spins.
    pub(super) fn test_hold_start_timer(&mut self, now: Instant) -> bool {
        let wrote = !self.stderr_tail.is_empty() || self.stderr_lines.partial().is_some();
        let held = !wrote && TEST_START_TIMER_UNTIL.is_some_and(|until| now < until);
        if held && let Some(start) = &mut self.start {
            start.deadline = Some(now + Duration::from_millis(10));
        }
        held
    }
}

/// Whether the other end has closed, without waiting or taking anything.
fn closed(stream: &UnixStream) -> bool {
    let mut byte = 0u8;
    let flags = libc::MSG_PEEK | libc::MSG_DONTWAIT;
    // SAFETY: byte is a valid one-byte buffer for recv(2), and stream's
    // descriptor is open while it is borrowed; MSG_PEEK leaves what it reads
    // in the stream.
    unsafe { libc::recv(stream.as_raw_fd(), (&raw mut byte).cast(), 1, flags) == 0 }
}

/// Reads (and drops) whatever comes until EOF.
fn read_to_eof(mut channel: UnixStream) {
    let mut buf = [0; 512];
    loop {
        match channel.read(&mut buf) {
            Ok(0) => return,
            Ok(_) => {}
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(_) => return,
        }
    }
}
