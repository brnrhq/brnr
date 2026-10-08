//! Backpressure on the session's own pipes (ADR 6 in docs/adr).
//!
//! The agent's output on its way to the editor (`from_agent`), and the
//! editor's input on its way to the agent's stdin (`to_agent`), are counted
//! in bytes from when a reader thread reads them until the thread writing
//! them out has. Past [`PIPE_BYTES`] the reader waits, so the writer at the
//! far end blocks as it would writing to the reader directly: the agent on
//! its stdout and stderr, or the proxy and so the editor on the link. Nothing
//! is buffered past the cap, nothing is dropped, and nothing times out: a
//! reader that hangs holds its writer up for as long as it hangs (P1, P6).
//!
//! A line is counted once it is whole; until then it is its reader's. One
//! longer than [`LINE_BYTES`] isn't kept whole: from there it goes on in
//! pieces as it comes, unread by the host, counted as they go (ADR 49).
//!
//! The event loop only counts; it never waits. Headless there is no link,
//! and the count is what the event loop hasn't handled yet.

use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

/// What may be on its way to a pipe: as much as a peer's queue (see
/// control.rs).
pub(super) const PIPE_BYTES: usize = 16 << 20;

/// The longest line the host reads, from the agent's stdout or the editor
/// (ADR 49), and so the most a reader holds of a line still coming. That
/// isn't counted against the cap: a line longer than the cap would hold its
/// reader back for room only its newline could make.
pub(super) const LINE_BYTES: usize = 32 << 20;

/// Bytes on their way through the host to one pipe.
#[derive(Clone, Default)]
pub(super) struct Backlog(Arc<(Mutex<Count>, Condvar)>);

#[derive(Default)]
struct Count {
    bytes: usize,
    /// The pipe's writer has gone: nothing waits for it any more.
    closed: bool,
}

impl Backlog {
    fn count(&self) -> MutexGuard<'_, Count> {
        self.0.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// `n` more bytes on their way. Never waits.
    pub(super) fn add(&self, n: usize) {
        self.count().bytes += n;
    }

    /// `n` of them are through: written out, or not going on.
    pub(super) fn done(&self, n: usize) {
        let mut count = self.count();
        let was = count.bytes;
        count.bytes = was.saturating_sub(n);
        if was > PIPE_BYTES && count.bytes <= PIPE_BYTES {
            self.0.1.notify_all();
        }
    }

    /// The writer has gone; what is counted won't be written.
    pub(super) fn close(&self) {
        self.count().closed = true;
        self.0.1.notify_all();
    }

    /// Past the cap: the reader is held back.
    pub(super) fn full(&self) -> bool {
        let count = self.count();
        !count.closed && count.bytes > PIPE_BYTES
    }

    /// Waits while the backlog is past the cap, for up to `timeout` if
    /// given; whether there is room now.
    pub(super) fn room(&self, timeout: Option<Duration>) -> bool {
        let count = self.count();
        let held = |c: &mut Count| !c.closed && c.bytes > PIPE_BYTES;
        let cond = &self.0.1;
        let mut count = match timeout {
            None => cond.wait_while(count, held).unwrap_or_else(PoisonError::into_inner),
            Some(t) => {
                cond.wait_timeout_while(count, t, held).unwrap_or_else(PoisonError::into_inner).0
            }
        };
        !held(&mut count)
    }
}
