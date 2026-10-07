//! The start channel (ADR 7 in docs/adr): brnr start's end of a socketpair,
//! at fd 3, and the host's first peer.
//!
//! Whatever fails before the start commits is reported on it, `{ok: false,
//! error}`, and stops the process. The commit is the ready report, `{ok:
//! true, pid, session, message}`, written once the session is open and its
//! mode and config options are set, and before the agent gets the prompt
//! (see `finish_start`; `message` is the prompt's `m<n>`). With `--wait` the
//! channel is subscribed from the start to the events brnr start follows its
//! turn by, which it reads on the same channel after the report.
//!
//! brnr start never writes on it; the host reads it on a thread for EOF.
//! brnr start gone before the commit means nobody knows of the session: the
//! process stops at once (`start-abandoned`). After the commit the session
//! carries on, whatever brnr start does (P14).

use std::io::{self, ErrorKind, Read};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::mpsc::SyncSender;
use std::thread;

use serde_json::json;

use super::control::{self, Closer, Peer, Queue};
use super::{Ev, Host};

/// The start channel while the start is under way.
pub(super) struct StartChannel {
    peer: u64,
    /// Looked at for EOF when committing (see `report_ready`).
    stream: UnixStream,
}

impl Host {
    /// Makes the start channel a peer, subscribed to `events`.
    pub(super) fn open_start_channel(
        &mut self,
        channel: UnixStream,
        events: Vec<String>,
        tx: &SyncSender<Ev>,
    ) -> io::Result<()> {
        let (writer, reader, closer) =
            (channel.try_clone()?, channel.try_clone()?, channel.try_clone()?);
        let peer = control::NEXT_PEER.fetch_add(1, Relaxed);
        let (queue, lines, queued) = Queue::new();
        thread::spawn(move || control::write_lines(writer, lines, queued));
        // A socket peer, as brnr's own connections are: not a bridge.
        let mut p = Peer::new(queue, "socket#start".into(), Closer::Socket(closer));
        (p.subscribed, p.events) = (true, Some(events));
        self.peers.insert(peer, p);
        let t = tx.clone();
        thread::spawn(move || {
            read_to_eof(reader);
            let _ = t.send(Ev::StartGone { peer });
        });
        self.start_channel = Some(StartChannel { peer, stream: channel });
        Ok(())
    }

    /// brnr start closed its end of the channel.
    pub(super) fn start_gone(&mut self, peer: u64) {
        self.peers.remove(&peer);
        if self.start_channel.as_ref().is_some_and(|c| c.peer == peer) {
            self.start_channel = None;
            self.abandon_start();
        }
    }

    fn abandon_start(&mut self) {
        self.sink.note(None, json!({ "event": "start-abandoned" }));
        self.begin_stop();
    }

    /// The commit: brnr start is told the session, and the message its
    /// prompt will be. False if it has gone, which abandons the start: its
    /// EOF may still be on its way to the event loop, so it is looked for
    /// here.
    pub(super) fn report_ready(&mut self, session: &str, message: Option<&str>) -> bool {
        let Some(channel) = self.start_channel.take() else { return false };
        if closed(&channel.stream) {
            self.peers.remove(&channel.peer);
            self.abandon_start();
            return false;
        }
        let mut ready = json!({ "ok": true, "pid": std::process::id(), "session": session });
        if let Some(message) = message {
            ready["message"] = json!(message);
        }
        self.reply(channel.peer, None, ready);
        true
    }

    /// Tells brnr start the start failed, if it is still waiting to hear.
    pub(super) fn report_failure(&mut self, error: &str) {
        if let Some(channel) = self.start_channel.take() {
            self.reply(channel.peer, None, json!({ "ok": false, "error": error }));
        }
    }
}

/// Whether the other end has closed, without waiting or taking anything.
fn closed(stream: &UnixStream) -> bool {
    let mut byte = 0u8;
    let flags = libc::MSG_PEEK | libc::MSG_DONTWAIT;
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
