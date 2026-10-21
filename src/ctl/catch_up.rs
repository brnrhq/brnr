//! Catching up (ADR 10.2): a command the process cut off for falling behind
//! reads what it missed from the transcript, then goes on live.
//!
//! Every event has a position, its `seq`: one more than the last event's or
//! ACP message's of its process. A subscription's answer says where it
//! starts, and each event it gets moves that on. Cut off, a command asks
//! the process whether it was (`was_cut_off`), subscribes again, which says
//! where the new connection starts, and once the process has written what
//! it recorded until then (`logged`), reads what lies between from the
//! transcript: the host log, which names each session's files, and the
//! files of the session it follows (every session's, for a whole process).
//! A process that has gone is asked of its host log instead (`peer_dropped`),
//! and its transcript is read to its end. Where the transcript can't have
//! all of it (no transcript, no raw ACP, records not written), the command
//! says what it missed, as an `events_missed` note shaped as ADR 10.1's
//! `records_skipped`, and exits [`MISSED`].

use std::fs::File;
use std::io::{BufRead, BufReader, ErrorKind};
use std::path::PathBuf;

use serde_json::{Value, json};

use crate::error::{self, Code};
use crate::{json, log, paths, transcript};

/// The exit status of a command that was cut off and couldn't catch up.
pub(super) const MISSED: u8 = 3;

/// How many times in a row a command may be cut off, with nothing read live
/// in between, before it gives up: a new connection cut off while the
/// command reads the transcript is tried again so many times.
pub(super) const TRIES: u32 = 3;

/// A subscription, to catch up from once cut off.
pub(super) struct Follow {
    /// The process's control socket.
    pub(super) socket: PathBuf,
    pub(super) host_id: String,
    /// The process's pid.
    pub(super) pid: Value,
    /// The process's host log, which names its sessions' files.
    pub(super) host_log: PathBuf,
    /// What the process calls this connection.
    pub(super) peer: String,
    /// What it subscribed to.
    pub(super) events: Vec<String>,
    /// The one session it follows; none for every session of the process.
    pub(super) session: Option<String>,
    /// The last event it got, or where its subscription started.
    pub(super) last: u64,
    /// Cut-offs in a row, with nothing read live in between.
    pub(super) tries: u32,
}

impl Follow {
    /// From a subscription's answer, or a start's ready report, `{peer,
    /// host_id, pid, seq}`; none from a process that doesn't say.
    pub(super) fn new(
        socket: PathBuf,
        answer: &Value,
        events: &[&str],
        session: Option<&str>,
    ) -> Option<Follow> {
        let host_id = answer["host_id"].as_str()?;
        Some(Follow {
            socket,
            host_log: paths::host_log(host_id),
            host_id: host_id.to_owned(),
            pid: answer["pid"].clone(),
            peer: answer["peer"].as_str()?.to_owned(),
            events: events.iter().map(|e| e.to_string()).collect(),
            session: session.map(str::to_owned),
            last: answer["seq"].as_u64()?,
            tries: 0,
        })
    }

    /// What it missed and can't catch up on, `why`, after its last event and
    /// up to `until` (none: to the end of a process that has gone).
    pub(super) fn missed(&self, until: Option<u64>, why: &error::Error) -> Value {
        json!({
            "event": "events_missed",
            "ts": log::rfc3339(std::time::SystemTime::now()),
            "host_id": self.host_id,
            "pid": self.pid,
            "peer": self.peer,
            "after_seq": self.last,
            "until_seq": until,
            "error": why.json(),
        })
    }

    /// Whether the process, having gone, had cut it off: its host log's
    /// `peer_dropped` says so.
    pub(super) fn cut_off_before_it_went(&self) -> bool {
        let Ok(file) = transcript::open(&self.host_log) else { return false };
        let lines = BufReader::new(file).split(b'\n').map_while(Result::ok);
        let ours =
            |r: &Value| r["event"]["event"] == "peer_dropped" && r["event"]["peer"] == *self.peer;
        lines
            .filter(|l| l.windows(13).any(|w| w == b"\"peer_dropped"))
            .filter_map(|l| transcript::head(&l))
            .any(|r| ours(&r))
    }

    /// The events it missed, after its last and up to `until` (to the end,
    /// for a process that has gone), in order, from the transcript; or why
    /// the transcript can't have them all.
    pub(super) fn read(&self, until: Option<u64>) -> Result<Vec<Value>, error::Error> {
        let acp = self.events.iter().any(|e| e == "acp");
        let host_log = &self.host_log;
        let mut found = Found { follow: self, until, events: Vec::new(), files: Vec::new() };
        match transcript::open(host_log) {
            Ok(file) => found.read(file, acp)?,
            Err(e) if e.kind() == ErrorKind::NotFound => {
                let error = "the process keeps no transcript (log = \"off\")";
                return Err(error::Error::new(Code::transcript_not_kept, error));
            }
            Err(e) => return Err(format!("{}: {e}", host_log.display()).into()),
        }
        for path in std::mem::take(&mut found.files) {
            let file = transcript::open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            found.read(file, acp)?;
        }
        let mut events = std::mem::take(&mut found.events);
        events.sort_by_key(|(seq, _)| *seq);
        // `process_exited` is in each session's file and the host log.
        events.dedup_by_key(|(seq, _)| *seq);
        Ok(events.into_iter().map(|(_, e)| e).collect())
    }
}

/// What reading the transcript finds.
struct Found<'a> {
    follow: &'a Follow,
    until: Option<u64>,
    events: Vec<(u64, Value)>,
    /// Files the host log names that are still to read.
    files: Vec<PathBuf>,
}

impl Found<'_> {
    /// Reads one of the process's files: the events it missed, and (in the
    /// host log) its sessions' files, the raw ACP too with `acp`.
    fn read(&mut self, file: File, acp: bool) -> Result<(), error::Error> {
        let f = self.follow;
        let ours = format!(r#""host_id":"{}""#, f.host_id);
        for line in BufReader::new(file).split(b'\n') {
            let line = line.map_err(|e| e.to_string())?;
            // Another process's, before or after this one's in the file.
            if !line.get(..200).unwrap_or(&line).windows(ours.len()).any(|w| w == ours.as_bytes()) {
                continue;
            }
            // Before or after what it missed: not read whole.
            if transcript::position(&line).is_some_and(|n| !self.missed(n)) {
                continue;
            }
            // As brnr reads what it wrote, as the socket's events: one too
            // deep is none it wrote, and is said (ADR 2.5).
            json::within(&line, json::OWN_DEPTH, "a record")?;
            self.record(&line, acp)?;
        }
        Ok(())
    }

    /// One record of the process's, `line`: an event it missed is kept, and
    /// (in the host log) a session's files are to read.
    fn record(&mut self, line: &[u8], acp: bool) -> Result<(), error::Error> {
        let f = self.follow;
        let Some(record) = transcript::record(line) else { return Ok(()) };
        if record["host_id"] != *f.host_id {
            return Ok(());
        }
        // An ACP message is an `acp` event, as `event log` has it.
        let Some(mut event) = transcript::event(record) else { return Ok(()) };
        match event["event"].as_str() {
            Some("session_opened") => {
                if f.session.as_ref().is_some_and(|s| event["session"] != **s) {
                    return Ok(());
                }
                let file = PathBuf::from(event["events_log"].as_str().unwrap_or_default());
                let raw = event["acp_log"].as_str().map(PathBuf::from);
                if acp && raw.is_none() {
                    let error = "the process keeps no raw ACP (log = \"events\")";
                    return Err(error::Error::new(Code::transcript_not_kept, error));
                }
                for path in [Some(file), raw.filter(|_| acp)].into_iter().flatten() {
                    if !self.files.contains(&path) {
                        self.files.push(path);
                    }
                }
            }
            // A gap where what it missed was (ADR 10.1).
            Some("records_skipped") => {
                let (first, last) = (event["first_seq"].as_u64(), event["last_seq"].as_u64());
                if let (Some(first), Some(last)) = (first, last)
                    && last > f.last
                    && self.until.is_none_or(|until| first <= until)
                {
                    let at = match event["error"]["message"].as_str() {
                        Some(error) => format!("a write failed: {error}"),
                        None => "the disk was behind".to_owned(),
                    };
                    return Err(
                        format!("the transcript lacks events {first} to {last} ({at})").into()
                    );
                }
            }
            Some(name) => {
                let Some(seq) = event["seq"].as_u64().filter(|&n| self.missed(n)) else {
                    return Ok(());
                };
                if self.wanted(name, &event["session"]) {
                    self.events.push((seq, event.take()));
                }
            }
            None => {}
        }
        Ok(())
    }

    /// Whether the event at `seq` is one it missed.
    fn missed(&self, seq: u64) -> bool {
        seq > self.follow.last && self.until.is_none_or(|until| seq <= until)
    }

    /// Whether it subscribed to `name`, and follows `session` (an event of
    /// the process's has none).
    fn wanted(&self, name: &str, session: &Value) -> bool {
        let f = self.follow;
        f.events.iter().any(|e| e == name)
            && (session.is_null() || f.session.as_ref().is_none_or(|s| session == s.as_str()))
    }
}
