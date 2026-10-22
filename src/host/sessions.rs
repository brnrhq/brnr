//! The session table (ADR 3.4, 3.9, 3.12, 5.5, 5.6): every session the
//! process knows of, from the lock it takes before the agent opens one until
//! that lock goes, once the session's records are written.
//!
//! A session's life here: claimed (its lock taken, or found held, before
//! the agent is asked to resume, load or prompt it), open (`Session`; a
//! bridge's `opening` until it commits, ADR 3.4; one an editor's prompt
//! opened `pending`, with no transcript, until the agent says anything of
//! it, ADR 5.6), closing (`Session::closing`, until the agent has closed
//! it, ADR 3.12), then let go of (`LettingGo`): its lock, and the answer
//! to its close, go once the logger has written what the process recorded
//! of it, 2 s at most (ADR 5.5). A load's replay outlasts its session's
//! close while the load is unanswered (ADR 5.6).
//!
//! Sessions open and close here only: [`Host::open_fork`] (or
//! [`Host::open_prompted`], pending) and
//! [`Host::close_session`], which every ending goes through, as its
//! [`Ending`] says, so no close forgets a step. The rest of the host reaches
//! an open session by its index (`host.sessions[i]`).

use std::collections::{HashMap, VecDeque};
use std::ops::{Index, IndexMut};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use serde_json::{Value, json};

use super::acp::{Replay, Session};
use super::requests::history;
use super::turns::Activity;
use super::{Host, LOG_FLUSH, control};
use crate::error::{Code, Error};
use crate::lock::{self, Lock};
use crate::log::Sink;

#[derive(Default)]
pub(super) struct Sessions {
    /// Open, in the order they opened.
    open: Vec<Session>,
    /// Taken (or failed to be taken) the lock of before the agent opened
    /// them: a resume or a load not yet answered.
    claimed: HashMap<String, Hold>,
    /// The replays of editors' sessions closed while a load of them was
    /// unanswered, until the last is (ADR 5.6): still counted, though not
    /// recorded, and the session's again if a load succeeds.
    closed_loads: HashMap<String, Replay>,
    /// Closed here, until their records are written (ADR 5.5).
    letting_go: Vec<LettingGo>,
    /// Closed by brnr under the editor (ADR 7.1) → the process that took
    /// each over, if one did: the editor's requests for them are answered
    /// by the host (see experimental.rs). In the order they closed, up to
    /// [`UNDER_EDITOR`], the oldest going first: the editor's request for
    /// one that has gone reaches the agent, which answers it with its own
    /// error.
    under_editor: VecDeque<(String, Option<u32>)>,
}

/// The most sessions kept as closed under the editor: past that, an editor
/// that never loads them again would have the host keep them without limit.
const UNDER_EDITOR: usize = 1024;

/// How a process holds a session it serves (ADR 3.9).
pub(super) enum Hold {
    /// Its lock: the process is the session's owner, and keeps its
    /// transcript. `None` once it has let go, as the process exits.
    Owner(Option<Lock>),
    /// The lock couldn't be taken (why, naming its path), so whether another
    /// process holds it can't be known. A headless process never serves such
    /// a session (P4: the client refuses); an editor's passes it through and
    /// keeps its transcript all the same (P1, P4: the proxy passes on what it
    /// can't be sure of), and `status` says it isn't locked (ADR 3.9).
    Unlocked(String),
    /// Process `pid` held the lock when the editor loaded the session here
    /// (`shared_sessions`, ADR 3.10): that one keeps the transcript, and this
    /// one records the session in its host log only.
    Shared(u32),
}

/// How an open session leaves the table (see [`Host::close_session`]).
#[derive(Clone, Copy)]
pub(super) enum Ending<'a> {
    /// The agent closed it, for `close`, `idle` or the `editor` (a deletion
    /// too): `session_closed` says so (ADR 5.2), and the replay of an
    /// editor's load still unanswered outlasts it (ADR 5.6).
    Closed(&'a str),
    /// The agent failed what it was opened for: a load, after replaying
    /// what it did of the history, recorded perhaps, or the editor's prompt
    /// of a session the agent said nothing of (ADR 5.6). It never opened,
    /// so no `session_closed`.
    OpenFailed,
    /// The agent opened it, and it isn't this process's to serve: a headless
    /// process's session another process holds, or one it couldn't lock
    /// (ADR 3.9). Nothing of it was recorded.
    NotOwned,
}

/// A session closed here, whose lock goes, and whose close is answered,
/// once the logger has written its records, `session_closed` last, or at
/// `until`, whichever comes first: a process that resumes it next appends
/// after them (ADR 5.5). The event loop never waits for it (P1).
struct LettingGo {
    session: String,
    until: Instant,
    /// Taken by whichever comes first: the logger's thread or the event
    /// loop's timer. This process opening the session again takes back only
    /// its lock: the close is still answered once it is written.
    held: Arc<Mutex<Option<Release>>>,
}

/// What goes as a closed session is let go of: its lock, then the answers
/// to the close, each a line for its peer.
struct Release {
    lock: Option<Lock>,
    answers: Vec<(control::Queue, String)>,
}

impl LettingGo {
    fn take(&self) -> Option<Release> {
        self.held.lock().unwrap_or_else(PoisonError::into_inner).take()
    }

    fn done(&self) -> bool {
        self.held.lock().unwrap_or_else(PoisonError::into_inner).is_none()
    }

    /// The session's lock, if it is still this process's.
    fn reclaim(&self) -> Option<Lock> {
        self.held.lock().unwrap_or_else(PoisonError::into_inner).as_mut()?.lock.take()
    }

    fn locked(&self) -> bool {
        self.held
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .is_some_and(|r| r.lock.is_some())
    }
}

impl Release {
    /// Lets go of the lock, if the session isn't open here again, then
    /// answers, so whoever hears it finds the session free.
    fn go(self) {
        drop(self.lock);
        for (queue, line) in self.answers {
            queue.post(line);
        }
    }
}

impl Index<usize> for Sessions {
    type Output = Session;

    fn index(&self, i: usize) -> &Session {
        &self.open[i]
    }
}

impl IndexMut<usize> for Sessions {
    fn index_mut(&mut self, i: usize) -> &mut Session {
        &mut self.open[i]
    }
}

impl<'a> IntoIterator for &'a Sessions {
    type Item = &'a Session;
    type IntoIter = std::slice::Iter<'a, Session>;

    fn into_iter(self) -> Self::IntoIter {
        self.open.iter()
    }
}

impl Sessions {
    pub(super) fn len(&self) -> usize {
        self.open.len()
    }

    pub(super) fn is_empty(&self) -> bool {
        self.open.is_empty()
    }

    /// Whether `session` is this process's already: open, or claimed.
    pub(super) fn knows(&self, session: &str) -> bool {
        self.open.iter().any(|s| s.id == session) || self.claimed.contains_key(session)
    }

    /// `session`, not open yet, is held as `hold` until the agent opens it.
    pub(super) fn claim(&mut self, session: &str, hold: Hold) {
        self.claimed.insert(session.to_owned(), hold);
    }

    /// `session`, claimed, isn't to be opened: how it was held, for its
    /// lock to go (see [`Host::claim_failed`]).
    pub(super) fn unclaim(&mut self, session: &str) -> Option<Hold> {
        self.claimed.remove(session)
    }

    /// A start that fails: nobody is to know of a session it was opening,
    /// and nothing was recorded under one still claimed, so its lock can go
    /// now (ADR 5.5).
    pub(super) fn unclaim_all(&mut self) {
        self.claimed.clear();
    }

    /// `session`'s lock, for a session this process is to serve. One another
    /// process holds is shared, as `attach` lets it be; one that can't be
    /// locked is `Unlocked`, and the host log says why. Whoever opened it
    /// decides what becomes of either (ADR 3.9).
    pub(super) fn take_lock(&mut self, session: &str, sink: &Sink) -> Hold {
        if let Some(lock) = self.reclaim(session) {
            return Hold::Owner(Some(lock));
        }
        match lock::take(session) {
            Ok(lock) => Hold::Owner(Some(lock)),
            Err(lock::Error::Held(pid)) => Hold::Shared(pid),
            Err(lock::Error::Io(error)) => {
                let failed = Error::from(error.as_str()).json();
                let event = json!({ "event": "lock_failed", "session": session, "error": failed });
                sink.note(None, event);
                Hold::Unlocked(error)
            }
        }
    }

    /// Takes the lock of `session`, which a headless start or a bridge
    /// resumes, before the agent is asked for it: one another process holds,
    /// or one that can't be locked, can't be served here (ADR 3.9).
    pub(super) fn own(&mut self, session: &str) -> Result<(), Error> {
        let lock = self.reclaim(session).map_or_else(|| lock::take(session), Ok);
        let lock = lock.map_err(|err| match err {
            lock::Error::Held(pid) => {
                Error::new(Code::session_active, format!("{session} is running in process {pid}"))
            }
            lock::Error::Io(why) => Error::from(format!("{session} can't be locked: {why}")),
        })?;
        self.claim(session, Hold::Owner(Some(lock)));
        Ok(())
    }

    /// The replay of `session`'s load, closed while the load was unanswered
    /// (ADR 5.6).
    pub(super) fn closed_load(&mut self, session: &str) -> Option<&mut Replay> {
        self.closed_loads.get_mut(session)
    }

    /// Lets go of `session`, gone from the table, held as `hold`, and posts
    /// `answers`, once `sink` has written what this process recorded of it,
    /// LOG_FLUSH at most (see `LettingGo`): a close's records, or a failed
    /// open's (a load's replay, ADR 5.6), come before the next process's
    /// (ADR 5.5). With no lock and nobody to answer, there is nothing to let
    /// go of.
    fn let_go(
        &mut self,
        session: String,
        hold: Hold,
        answers: Vec<(control::Queue, String)>,
        sink: &Sink,
    ) {
        let lock = match hold {
            Hold::Owner(lock) => lock,
            Hold::Shared(_) | Hold::Unlocked(_) => None,
        };
        if lock.is_none() && answers.is_empty() {
            return;
        }
        let held = Arc::new(Mutex::new(Some(Release { lock, answers })));
        let written = held.clone();
        sink.when_written(move |_| {
            // Taken out first, so the lock isn't held while it goes.
            let release = written.lock().unwrap_or_else(PoisonError::into_inner).take();
            if let Some(release) = release {
                release.go();
            }
        });
        let until = Instant::now() + LOG_FLUSH;
        self.letting_go.push(LettingGo { session, until, held });
    }

    /// Closed sessions whose time to be written is up are let go of as they
    /// are; past that, records of theirs can come after a resume's (ADR 5.5).
    pub(super) fn fire_letting_go(&mut self, now: Instant) {
        self.letting_go.retain(|l| {
            if now < l.until {
                return !l.done();
            }
            if let Some(release) = l.take() {
                release.go();
            }
            false
        });
    }

    /// When the next closed session is let go of, for the event loop.
    pub(super) fn next_letting_go(&self) -> Option<Instant> {
        self.letting_go.iter().map(|l| l.until).min()
    }

    /// Exiting, once the logger has had its time (ADR 5.5): every session is
    /// let go of as brnr stops listing the process (ADR 3.9), those closing
    /// here too, whose closes are answered.
    pub(super) fn let_go_of_all(&mut self) {
        for release in self.letting_go.drain(..).filter_map(|l| l.take()) {
            release.go();
        }
        self.claimed.clear();
        for s in &mut self.open {
            if let Hold::Owner(lock) = &mut s.hold {
                lock.take();
            }
        }
    }

    /// `session`'s lock, if it closed here and isn't let go of yet: opened
    /// here again, it is this process's still, and its records come after
    /// the close's from the same logger. The close is answered as before,
    /// once its records are written (ADR 5.5).
    fn reclaim(&mut self, session: &str) -> Option<Lock> {
        self.letting_go.iter().filter(|l| l.session == session).find_map(LettingGo::reclaim)
    }

    /// Whether `session` closed here and its lock is still this process's.
    pub(super) fn letting_go_of(&self, session: &str) -> bool {
        self.letting_go.iter().any(|l| l.session == session && l.locked())
    }

    /// The sessions closed here whose locks are still this process's, until
    /// their records are written: `status` says so, so that brnr can tell
    /// one closing from one opening (ADR 5.5).
    pub(super) fn closing(&self) -> Vec<&str> {
        self.letting_go.iter().filter(|l| l.locked()).map(|l| l.session.as_str()).collect()
    }

    /// brnr closed `session` under the editor, for process `taken_by` if one
    /// took it over (ADR 7.1).
    pub(super) fn close_under_editor(&mut self, session: &str, taken_by: Option<u32>) {
        self.under_editor.retain(|(s, _)| s != session);
        self.under_editor.push_back((session.to_owned(), taken_by));
        while self.under_editor.len() > UNDER_EDITOR {
            self.under_editor.pop_front();
        }
    }

    /// The editor's again (loaded, or its close failed): whether brnr had
    /// closed it under the editor.
    pub(super) fn back_to_editor(&mut self, session: &str) -> bool {
        let pos = self.under_editor.iter().position(|(s, _)| s == session);
        pos.and_then(|pos| self.under_editor.remove(pos)).is_some()
    }

    /// Whether brnr closed `session` under the editor, and for which process.
    pub(super) fn closed_under_editor(&self, session: &str) -> Option<Option<u32>> {
        self.under_editor.iter().find(|(s, _)| s == session).map(|&(_, taken_by)| taken_by)
    }
}

impl Host {
    pub(super) fn find(&self, session: &str) -> Option<usize> {
        self.sessions.open.iter().position(|s| s.id == session)
    }

    /// Index of `session`, started (with its lock and log file) if this is
    /// the first we hear of it. Without a cwd, the host's own is used.
    pub(super) fn open_session(&mut self, session: &str, cwd: Option<&str>) -> usize {
        self.open_fork(session, cwd, None)
    }

    /// Index of `session`, for the editor's prompt of it: one not open yet
    /// opens pending (`Session::pending`), its records in the host log until
    /// the agent says anything of it (see [`Self::confirm`]) or the editor
    /// loads it. A conforming agent fails a prompt of a session it doesn't
    /// have, and a transcript of one would stop a later load of it recording
    /// its history (ADR 5.6).
    pub(super) fn open_prompted(&mut self, session: &str) -> usize {
        match self.find(session) {
            Some(i) => i,
            None => self.open_in(session, None, None, true),
        }
    }

    /// Session `i` is the agent's: if it was pending, it no longer is, and
    /// its files open if they aren't (see [`Self::open_files`]).
    pub(super) fn confirm(&mut self, i: usize, cwd: Option<&str>) {
        if std::mem::take(&mut self.sessions[i].pending) {
            self.open_files(i, cwd);
        }
    }

    /// Session `i`'s files, which a pending one hasn't, open, as they would
    /// have as it opened, under `cwd` if one is given: what is recorded of it
    /// from now on goes there (ADR 5.6). Once only.
    pub(super) fn open_files(&mut self, i: usize, cwd: Option<&str>) {
        let s = &mut self.sessions[i];
        if !std::mem::take(&mut s.unopened) {
            return;
        }
        if let Some(cwd) = cwd {
            s.cwd = PathBuf::from(cwd);
        }
        let s = &self.sessions[i];
        if self.keeps_transcript(&s.hold) {
            self.sink.open_session(&s.id, &s.cwd, s.forked_from.as_deref());
        }
    }

    /// Session `i`, if pending, closes once the agent has failed everything
    /// the editor asked of it, having said nothing of it: no turn of it
    /// runs, and no load or resume of it waits for its answer (ADR 5.6).
    /// As a failed load's, it never opened, and its lock goes. Whether it
    /// closed.
    pub(super) fn close_if_failed(&mut self, i: usize) -> bool {
        let s = &self.sessions[i];
        let asked = s.turns.activity() == Activity::Busy
            || s.replay.as_ref().is_some_and(|r| r.unanswered > 0)
            || self.outbound.resuming(&s.id);
        if !s.pending || asked {
            return false;
        }
        self.close_session(i, Ending::OpenFailed, None);
        true
    }

    /// Whether this process keeps the transcript of a session it holds as
    /// `hold`: its owner does, and an editor's does of one it couldn't lock
    /// (P1, P4); a headless process won't serve that one, and a shared one
    /// is its owner's (ADR 3.9).
    fn keeps_transcript(&self, hold: &Hold) -> bool {
        match hold {
            Hold::Owner(_) => true,
            Hold::Unlocked(_) => self.editor_attached(),
            Hold::Shared(_) => false,
        }
    }

    /// As [`Self::open_session`], for a session forked from `forked_from`,
    /// which its transcript's `session_opened` and its status name (ADR 3.12).
    /// A claimed session opens with its claim's hold; one a load still to be
    /// answered had, closed under it, with its replay (ADR 5.6).
    pub(super) fn open_fork(
        &mut self,
        session: &str,
        cwd: Option<&str>,
        forked_from: Option<&str>,
    ) -> usize {
        match self.find(session) {
            Some(i) => i,
            None => self.open_in(session, cwd, forked_from, false),
        }
    }

    /// `session`, not open, opens: with its files, unless `pending`.
    fn open_in(
        &mut self,
        session: &str,
        cwd: Option<&str>,
        forked_from: Option<&str>,
        pending: bool,
    ) -> usize {
        let cwd = cwd.map(PathBuf::from).unwrap_or_else(|| self.cwd.clone());
        let hold = match self.sessions.claimed.remove(session) {
            Some(hold) => hold,
            None => self.sessions.take_lock(session, &self.sink),
        };
        if let Hold::Shared(pid) = hold {
            let event = json!({ "event": "session_shared", "session": session, "lock_pid": pid });
            self.sink.note(None, event);
        }
        if !pending && self.keeps_transcript(&hold) {
            self.sink.open_session(session, &cwd, forked_from);
        }
        let replay = self.sessions.closed_loads.remove(session);
        let mut opened = Session::new(session, cwd, forked_from, hold, replay);
        (opened.pending, opened.unopened) = (pending, pending);
        self.sessions.open.push(opened);
        self.sessions.len() - 1
    }

    /// The one way session `i` leaves the table, ended as `ending` says:
    /// the message still coming goes out (still marked as replayed, P3), what
    /// it holds is dropped, each with its event, and `session_closed` says
    /// so if it was open (ADR 5.2). Its files close, and its lock goes, and
    /// the peer in `answer` is answered with its reply, once what was
    /// recorded of it is written, LOG_FLUSH at most (ADR 5.5).
    pub(super) fn close_session(&mut self, i: usize, ending: Ending, answer: Option<(u64, Value)>) {
        self.flush_agent_message(i);
        self.drop_all(i, "close");
        let Session { id: session, hold, replay, .. } = self.sessions.open.remove(i);
        // Whatever ended it, the editor's late answers to it aren't kept.
        self.approvals.closed(&session);
        if let Ending::Closed(how) = ending {
            // Who, and why (ADR 5.9): the editor closes; brnr closes for a
            // command's `close` or when idle.
            let (by, reason) = if how == "editor" { ("editor", "close") } else { ("control", how) };
            self.emit(json!({ "event": "session_closed", "session": session, "by": by, "reason": reason }));
            // A load of it still to be answered may open it again (ADR 5.6).
            if let Some(replay) = replay.filter(|r| r.unanswered > 0 && self.editor_attached()) {
                self.sessions.closed_loads.insert(session.clone(), replay);
            }
        }
        // After its last record, so nothing of it is lost (ADR 5.1). Every
        // ending closes them: one `NotOwned` has none to close, as a headless
        // process, the one that refuses such a session, opens none for it
        // (see `open_fork`), nor has one still pending (see
        // `open_prompted`), so closing them is a no-op for those.
        self.sink.close_session(&session);
        let answers =
            answer.and_then(|(peer, reply)| Some((self.peer_queue(peer)?, reply.to_string())));
        self.sessions.let_go(session, hold, answers.into_iter().collect(), &self.sink);
    }

    /// The agent failed to open `session`, a load or a resume of it: one
    /// open for a load closes (`Ending::OpenFailed`), and one only claimed
    /// is let go of (see [`Self::claim_failed`]), the peer in `answer`
    /// answered with its reply once what was recorded of it is written
    /// (ADR 5.5). Whether this process held it at all.
    pub(super) fn open_failed(&mut self, session: &str, answer: Option<(u64, Value)>) -> bool {
        if let Some(i) = self.find(session) {
            self.close_session(i, Ending::OpenFailed, answer);
            return true;
        }
        self.claim_failed(session, answer)
    }

    /// The agent failed to open `session`, claimed and not open: it is let
    /// go of, and the peer in `answer` answered with its reply, once what was
    /// recorded of it is written. A lock the claim took back from a close
    /// here goes after that close's records (ADR 5.5). Whether it was
    /// claimed.
    pub(super) fn claim_failed(&mut self, session: &str, answer: Option<(u64, Value)>) -> bool {
        let Some(hold) = self.sessions.unclaim(session) else { return false };
        let answers =
            answer.and_then(|(peer, reply)| Some((self.peer_queue(peer)?, reply.to_string())));
        self.sessions.let_go(session.to_owned(), hold, answers.into_iter().collect(), &self.sink);
        true
    }

    /// A load of `session`, closed meanwhile, failed. Its replay goes with
    /// the last of its loads; if one of them succeeded before the close,
    /// its `history` says so, with the session closed (ADR 5.6).
    pub(super) fn closed_load_failed(&mut self, session: &str) {
        let Some(replay) = self.sessions.closed_loads.get_mut(session) else { return };
        replay.unanswered = replay.unanswered.saturating_sub(1);
        if replay.unanswered > 0 {
            return;
        }
        if let Some(replay) = self.sessions.closed_loads.remove(session)
            && replay.loaded
        {
            self.emit(history(session, &replay));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::approvals::{EditorAnswer, Who};
    use crate::host::fuzz::Harness;
    use crate::host::id_key;
    use crate::log::Logger;

    /// An editor's process with `session` claimed, as if its lock were
    /// taken, and a load of it sent: the agent's id for it is `brnr-1`.
    fn loading(session: &str) -> Harness {
        let mut h = Harness::editor(false, false);
        h.claim(session);
        let load = json!({ "jsonrpc": "2.0", "id": 2, "method": "session/load",
            "params": { "sessionId": session, "cwd": "/", "mcpServers": [] } });
        h.editor_bytes(format!("{load}\n").as_bytes());
        let update = json!({ "jsonrpc": "2.0", "method": "session/update", "params": {
            "sessionId": session, "update": { "sessionUpdate": "agent_message_chunk",
            "content": { "type": "text", "text": "replayed" } } } });
        h.agent_line(format!("{update}\n").as_bytes());
        h.sent();
        h
    }

    const LOAD_FAILED: &[u8] =
        br#"{"jsonrpc":"2.0","id":"brnr-1","error":{"code":-32603,"message":"no"}}"#;

    #[test]
    fn adr_0003_a_claim_is_known_until_the_agent_opens_it_or_not() {
        let mut table = Sessions::default();
        assert!(!table.knows("s1"));
        table.claim("s1", Hold::Owner(None));
        table.claim("s2", Hold::Shared(7));
        assert!(table.knows("s1") && table.knows("s2"));
        // How it was held, for its lock to go; once.
        assert!(matches!(table.unclaim("s1"), Some(Hold::Owner(None))));
        assert!(table.unclaim("s1").is_none());
        assert!(!table.knows("s1"));
        table.unclaim_all();
        assert!(!table.knows("s2"));
    }

    #[test]
    fn adr_0005_nothing_is_let_go_of_without_a_lock_or_an_answer() {
        let mut table = Sessions::default();
        let sink = Logger::disabled().sink();
        table.let_go("s1".into(), Hold::Shared(7), Vec::new(), &sink);
        table.let_go("s2".into(), Hold::Unlocked("no".into()), Vec::new(), &sink);
        table.let_go("s3".into(), Hold::Owner(None), Vec::new(), &sink);
        assert!(table.letting_go.is_empty());
        assert_eq!(table.next_letting_go(), None);
        assert!(table.closing().is_empty());
    }

    #[test]
    fn adr_0005_an_editors_failed_load_closes_the_session_it_opened() {
        let mut h = loading("unit-load-failed");
        assert!(h.host().sessions.knows("unit-load-failed"));
        h.agent_line(LOAD_FAILED);
        let table = &h.host().sessions;
        assert!(table.is_empty() && !table.knows("unit-load-failed"));
        assert!(table.closed_loads.is_empty(), "a replay outlasts a session that never opened");
    }

    #[test]
    fn adr_0005_a_replay_outlasts_its_sessions_close_until_its_last_load() {
        let session = "unit-load-closed";
        let mut h = loading(session);
        let close = json!({ "jsonrpc": "2.0", "id": 3, "method": "session/close",
            "params": { "sessionId": session } });
        h.editor_bytes(format!("{close}\n").as_bytes());
        h.agent_line(br#"{"jsonrpc":"2.0","id":"brnr-2","result":{}}"#);
        let table = &mut h.host().sessions;
        assert!(table.is_empty());
        assert_eq!(table.closed_load(session).map(|r| r.unanswered), Some(1));
        h.agent_line(LOAD_FAILED);
        assert!(h.host().sessions.closed_load(session).is_none());
    }

    /// The editor's `method` of `session`, with id `id`: a prompt, a load
    /// or a resume, as each takes what the others don't need.
    fn ask(h: &mut Harness, id: u64, method: &str, session: &str) {
        let params = json!({ "sessionId": session, "cwd": "/", "mcpServers": [],
            "prompt": [{ "type": "text", "text": "hi" }] });
        let msg = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        h.editor_bytes(format!("{msg}\n").as_bytes());
    }

    /// The agent's failure of its request `wire`: it has no such session.
    fn not_found(wire: &str) -> Vec<u8> {
        let error = json!({ "code": -32002, "message": "Session not found" });
        json!({ "jsonrpc": "2.0", "id": wire, "error": error }).to_string().into_bytes()
    }

    #[test]
    fn adr_0005_a_prompted_session_outlasts_a_failed_load_while_its_turn_runs() {
        let session = "unit-prompt-load";
        let mut h = Harness::editor(false, false);
        h.claim(session);
        ask(&mut h, 2, "session/prompt", session);
        ask(&mut h, 3, "session/load", session);
        h.agent_line(&not_found("brnr-2"));
        assert!(h.host().find(session).is_some(), "closed under its running prompt");
        h.agent_line(&not_found("brnr-1"));
        let table = &h.host().sessions;
        assert!(table.is_empty() && !table.knows(session), "failed all it was asked");
    }

    #[test]
    fn adr_0005_a_prompted_session_is_held_until_its_resume_is_answered() {
        for resumed in [false, true] {
            let session = "unit-prompt-resume";
            let mut h = Harness::editor(false, false);
            h.claim(session);
            ask(&mut h, 2, "session/prompt", session);
            ask(&mut h, 3, "session/resume", session);
            h.agent_line(&not_found("brnr-1"));
            assert!(h.host().sessions.knows(session), "let go of before its resume's answer");
            if resumed {
                h.agent_line(br#"{"jsonrpc":"2.0","id":"brnr-2","result":{}}"#);
                let i = h.host().find(session).expect("closed though resumed");
                let s = &h.host().sessions[i];
                assert!(!s.pending && matches!(s.hold, Hold::Owner(_)));
            } else {
                h.agent_line(&not_found("brnr-2"));
                let table = &h.host().sessions;
                assert!(table.is_empty() && !table.knows(session));
                assert!(table.letting_go.is_empty() && table.claimed.is_empty());
            }
        }
    }

    #[test]
    fn adr_0003_a_session_another_process_holds_fails_a_headless_start() {
        let mut h = Harness::headless(false, None);
        h.host().sessions.claim("unit-shared", Hold::Shared(7));
        h.agent_line(br#"{"jsonrpc":"2.0","id":"brnr-1","result":{"protocolVersion":1}}"#);
        h.agent_line(br#"{"jsonrpc":"2.0","id":"brnr-2","result":{"sessionId":"unit-shared"}}"#);
        let start = h.sent().start;
        assert_eq!(start.len(), 1);
        let error = start[0]["error"]["message"].as_str().unwrap_or_default();
        assert!(error.ends_with("unit-shared, which is running in process 7"), "{error}");
        assert_eq!(start[0]["error"]["code"], "session_active");
        let table = &h.host().sessions;
        assert!(table.is_empty() && !table.knows("unit-shared") && table.letting_go.is_empty());
    }

    #[test]
    fn adr_0007_a_session_closed_under_the_editor_is_its_again_once_loaded() {
        let mut table = Sessions::default();
        table.close_under_editor("s1", Some(42));
        table.close_under_editor("s2", None);
        assert_eq!(table.closed_under_editor("s1"), Some(Some(42)));
        assert_eq!(table.closed_under_editor("s2"), Some(None));
        assert!(table.back_to_editor("s1"));
        assert!(!table.back_to_editor("s1"));
        assert_eq!(table.closed_under_editor("s1"), None);
    }

    #[test]
    fn adr_0010_sessions_closed_under_the_editor_past_the_cap_lose_the_oldest() {
        let mut table = Sessions::default();
        for n in 0..=UNDER_EDITOR {
            table.close_under_editor(&format!("s{n}"), None);
        }
        assert_eq!(table.under_editor.len(), UNDER_EDITOR);
        assert_eq!(table.closed_under_editor("s0"), None, "oldest kept");
        assert_eq!(table.closed_under_editor(&format!("s{UNDER_EDITOR}")), Some(None));
    }

    #[test]
    fn adr_0010_a_closed_sessions_answered_requests_go() {
        let session = "unit-answered";
        for close in [false, true] {
            let mut h = Harness::editor(false, false);
            h.claim(session);
            let i = h.host().open_session(session, None);
            let options =
                json!([{ "optionId": "allow_once", "name": "Allow", "kind": "allow_once" }]);
            let ask = json!({ "jsonrpc": "2.0", "id": 7, "method": "session/request_permission",
                "params": { "sessionId": session, "toolCall": { "toolCallId": "t1" },
                "options": options } });
            h.agent_line(format!("{ask}\n").as_bytes());
            h.host().cancel(session, &Who::because("cancel"));
            if close {
                h.host().close_session(i, Ending::Closed("close"), None);
            }
            h.sent();
            let late = json!({ "jsonrpc": "2.0", "id": 7,
                "result": { "outcome": { "outcome": "selected", "optionId": "allow_once" } } });
            h.editor_bytes(format!("{late}\n").as_bytes());
            // Kept, the late answer is dropped; gone with its session, it goes
            // on, and the agent ignores it (ADR 2.5).
            assert_eq!(!h.sent().agent.is_empty(), close, "close: {close}");
        }
    }

    #[test]
    fn adr_0010_a_request_cancelled_after_its_close_is_not_kept() {
        let session = "unit-given-up";
        for close in [false, true] {
            let mut h = Harness::editor(false, false);
            h.claim(session);
            let i = h.host().open_session(session, None);
            let options =
                json!([{ "optionId": "allow_once", "name": "Allow", "kind": "allow_once" }]);
            let ask = json!({ "jsonrpc": "2.0", "id": 7, "method": "session/request_permission",
                "params": { "sessionId": session, "toolCall": { "toolCallId": "t1" },
                "options": options } });
            h.agent_line(format!("{ask}\n").as_bytes());
            if close {
                h.host().close_session(i, Ending::Closed("close"), None);
            }
            let cancel = json!({ "jsonrpc": "2.0", "method": "$/cancel_request",
                "params": { "requestId": 7 } });
            h.agent_line(format!("{cancel}\n").as_bytes());
            // Open, it is kept for the editor's answer; closed, it isn't.
            let answer =
                h.host().approvals.editor_answer(&id_key(&json!(7)), &serde_json::Map::new());
            match answer {
                EditorAnswer::GivenUp(Some(s)) => assert!(!close && s == session),
                EditorAnswer::Unknown => assert!(close, "not kept while open"),
                _ => panic!("close: {close}"),
            }
        }
    }
}
