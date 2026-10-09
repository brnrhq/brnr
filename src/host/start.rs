//! The start channel (ADR 7 in docs/adr): brnr session new's (or `resume`'s)
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
//! so it misses none of them.
//!
//! brnr session new never writes on it; the host reads it on a thread for EOF.
//! brnr session new gone before the commit means nobody knows of the session:
//! the process stops at once (`start-abandoned`). After the commit the session
//! carries on, whatever brnr session new does (P14).

use std::env;
use std::io::{self, ErrorKind, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::sync::LazyLock;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::mpsc::{Sender, SyncSender};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::control::{self, Closer, Peer, Queue};
use super::{Ev, Host};

/// The start channel while the start is under way.
pub(super) enum StartChannel {
    /// A simulated start in the fuzz harness: reports are collected without
    /// sockets, threads or peers, as the agent's input and editor link are.
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
        self.start_channel = Some(StartChannel::Socket { peer, stream: channel, closer, events });
        let t = tx.clone();
        thread::spawn(move || {
            read_to_eof(reader);
            let _ = t.send(Ev::StartGone { peer });
        });
        Ok(())
    }

    /// The start channel is let go of, with nothing written to it: how the
    /// start ended is `main`'s to tell brnr session new, on fd 3, as of a
    /// start that fails before the event loop (see `Host::died_starting`).
    pub(super) fn release_start_channel(&mut self) {
        self.start_channel = None;
    }

    /// brnr session new closed its end of the channel.
    pub(super) fn start_gone(&mut self, peer: u64) {
        self.peers.remove(&peer);
        if matches!(&self.start_channel, Some(StartChannel::Socket { peer: id, .. }) if *id == peer)
        {
            self.start_channel = None;
            self.abandon_start();
        }
    }

    fn abandon_start(&mut self) {
        self.sink.note(None, json!({ "event": "start-abandoned" }));
        self.begin_stop();
    }

    /// The commit: brnr session new is told the session, and the message its
    /// prompt will be. False if the report can't be written, which abandons
    /// the start: brnr session new has gone, whether or not its EOF has
    /// reached the event loop yet.
    pub(super) fn report_ready(&mut self, session: &str, message: Option<&str>) -> bool {
        let Some(channel) = self.start_channel.take() else { return false };
        let mut ready = json!({ "ok": true, "pid": std::process::id(), "session": session });
        if let Some(message) = message {
            ready["message"] = json!(message);
        }
        let (peer, mut stream, closer, events) = match channel {
            StartChannel::Socket { peer, stream, closer, events } => (peer, stream, closer, events),
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
        let mut p = Peer::new(queue, "socket#start".into(), Closer::Socket(closer));
        (p.subscribed, p.events) = (true, Some(events));
        self.peers.insert(peer, p);
        true
    }

    /// Tells brnr session new the start failed, if it is still waiting to
    /// hear.
    pub(super) fn report_failure(&mut self, error: &str) {
        let report = json!({ "ok": false, "error": error });
        match self.start_channel.take() {
            Some(StartChannel::Socket { mut stream, .. }) => {
                let _ = write_report(&mut stream, &report);
            }
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
    /// goes at the worst moment. `test-ready-held` in the host log says it is
    /// held.
    fn test_hold_ready(&self, stream: &UnixStream) {
        if !*TEST_HOLD_READY {
            return;
        }
        self.sink.note(None, json!({ "event": "test-ready-held" }));
        let until = Instant::now() + Duration::from_secs(30);
        while !closed(stream) && Instant::now() < until {
            thread::sleep(Duration::from_millis(10));
        }
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
