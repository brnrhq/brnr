//! The foreground's display (ADR 9 in docs/adr): the session's events on
//! stdout and brnr's own messages on stderr, written on a thread of its own,
//! so a terminal or a pipe that is slow, paused or closed never holds up the
//! host.
//!
//! Events are queued up to [`DISPLAY_BYTES`]; past that they are skipped,
//! and the next line says how many (`… 120 events not shown`; with `--json`,
//! `{"not_shown": 120}`). Until then an event is queued however big, as for
//! a peer. brnr's own messages are never skipped. stdout closing (`brnr
//! start --foreground … | head -1`) ends the display, with a note in the
//! host log; the session carries on. The agent's stderr doesn't come this
//! way: its reader writes it to stderr as it comes (ADR 10).

use std::collections::VecDeque;
use std::fs::File;
use std::mem::{ManuallyDrop, take};
use std::os::fd::FromRawFd;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};

use serde_json::json;

use crate::log::Sink;
use crate::proxy::write_all;

/// Events queued for the display before the next are skipped.
const DISPLAY_BYTES: usize = 1 << 20;

pub(super) struct Display {
    shared: Arc<(Mutex<Queue>, Condvar)>,
    thread: JoinHandle<()>,
}

#[derive(Default)]
struct Queue {
    lines: VecDeque<Line>,
    /// Bytes of the events in `lines`.
    bytes: usize,
    /// Events skipped since the last line about them.
    skipped: u64,
    stdout_closed: bool,
    finished: bool,
}

enum Line {
    Event(String),
    /// Says how many events were skipped.
    Skipped(u64),
    /// brnr's own, for stderr.
    Note(String),
}

impl Display {
    pub(super) fn start(sink: Sink, json: bool) -> Display {
        let shared = Arc::new((Mutex::new(Queue::default()), Condvar::new()));
        let s = shared.clone();
        let thread = thread::spawn(move || write(&s, &sink, json));
        Display { shared, thread }
    }

    fn queue(&self) -> MutexGuard<'_, Queue> {
        self.shared.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Shows the event `line` renders, or counts it skipped. Rendered only
    /// if it is to be shown.
    pub(super) fn event(&self, line: impl FnOnce() -> Option<String>) {
        {
            let mut queue = self.queue();
            if queue.stdout_closed {
                return;
            }
            if queue.bytes > DISPLAY_BYTES {
                queue.skipped += 1;
                return;
            }
        }
        // Only the writer takes from the queue: there is still room.
        let Some(line) = line() else { return };
        let mut queue = self.queue();
        if queue.stdout_closed {
            return;
        }
        if queue.skipped > 0 {
            let skipped = take(&mut queue.skipped);
            queue.lines.push_back(Line::Skipped(skipped));
        }
        queue.bytes += line.len();
        queue.lines.push_back(Line::Event(line));
        self.shared.1.notify_one();
    }

    /// A message of brnr's own, on stderr.
    pub(super) fn note(&self, text: String) {
        self.queue().lines.push_back(Line::Note(text));
        self.shared.1.notify_one();
    }

    /// Ends the display once what is queued is written.
    pub(super) fn finish(&self) {
        self.queue().finished = true;
        self.shared.1.notify_one();
    }

    /// Whether it has ended.
    pub(super) fn done(&self) -> bool {
        self.thread.is_finished()
    }
}

/// The display's thread: writes what is queued until the display finishes.
fn write(shared: &(Mutex<Queue>, Condvar), sink: &Sink, json: bool) {
    let mut stdout = ManuallyDrop::new(unsafe { File::from_raw_fd(1) });
    let mut stderr = ManuallyDrop::new(unsafe { File::from_raw_fd(2) });
    let lock = || shared.0.lock().unwrap_or_else(PoisonError::into_inner);
    loop {
        let line = {
            let mut queue = lock();
            loop {
                if let Some(line) = queue.lines.pop_front() {
                    break line;
                }
                // Caught up: the events skipped since are said now.
                if queue.skipped > 0 && !queue.stdout_closed {
                    break Line::Skipped(take(&mut queue.skipped));
                }
                if queue.finished {
                    return;
                }
                queue = shared.1.wait(queue).unwrap_or_else(PoisonError::into_inner);
            }
        };
        let (text, len) = match line {
            Line::Note(text) => {
                let _ = write_all(&mut stderr, format!("{text}\n").as_bytes());
                continue;
            }
            Line::Event(text) => {
                let len = text.len();
                (text, len)
            }
            Line::Skipped(n) if json => (json!({ "not_shown": n }).to_string(), 0),
            Line::Skipped(n) => (format!("… {n} events not shown"), 0),
        };
        let written = write_all(&mut stdout, format!("{text}\n").as_bytes());
        let mut queue = lock();
        queue.bytes = queue.bytes.saturating_sub(len);
        if let Err(err) = written
            && !queue.stdout_closed
        {
            // What was queued for it goes with it; brnr's notes stay.
            queue.stdout_closed = true;
            queue.lines.retain(|l| matches!(l, Line::Note(_)));
            (queue.bytes, queue.skipped) = (0, 0);
            drop(queue);
            sink.note(None, json!({ "event": "display-stopped", "error": err.to_string() }));
        }
    }
}
