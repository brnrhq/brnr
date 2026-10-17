//! Catching up (ADR 69): a command the process cut off for falling behind
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
//! A process that has gone is asked of its host log instead (`peer-dropped`),
//! and its transcript is read to its end. Where the transcript can't have
//! all of it (no transcript, no raw ACP, records not written), the command
//! says what it missed, as an `events-missed` note shaped as ADR 6's
//! `records-skipped`, and exits [`MISSED`].

use std::fs::File;
use std::io::{BufRead, BufReader, ErrorKind};
use std::path::PathBuf;

use serde_json::{Value, json};

use crate::{json, log, paths};

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
    /// host_id, seq}`; none from a process that doesn't say.
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
            peer: answer["peer"].as_str()?.to_owned(),
            events: events.iter().map(|e| e.to_string()).collect(),
            session: session.map(str::to_owned),
            last: answer["seq"].as_u64()?,
            tries: 0,
        })
    }

    /// What it missed and can't catch up on, `why`, after its last event and
    /// up to `until` (none: to the end of a process that has gone).
    pub(super) fn missed(&self, until: Option<u64>, why: &str) -> Value {
        json!({
            "event": "events-missed",
            "ts": log::rfc3339(std::time::SystemTime::now()),
            "host_id": self.host_id,
            "peer": self.peer,
            "after": self.last,
            "until": until,
            "error": why,
        })
    }

    /// Whether the process, having gone, had cut it off: its host log's
    /// `peer-dropped` says so.
    pub(super) fn cut_off_before_it_went(&self) -> bool {
        let Ok(file) = File::open(&self.host_log) else { return false };
        let lines = BufReader::new(file).split(b'\n').map_while(Result::ok);
        let ours =
            |r: &Value| r["event"]["event"] == "peer-dropped" && r["event"]["peer"] == *self.peer;
        lines
            .filter(|l| l.windows(13).any(|w| w == b"\"peer-dropped"))
            .filter_map(|l| serde_json::from_slice::<Value>(&l).ok())
            .any(|r| ours(&r))
    }

    /// The events it missed, after its last and up to `until` (to the end,
    /// for a process that has gone), in order, from the transcript; or why
    /// the transcript can't have them all.
    pub(super) fn read(&self, until: Option<u64>) -> Result<Vec<Value>, String> {
        let acp = self.events.iter().any(|e| e == "acp");
        let host_log = &self.host_log;
        let mut found = Found { follow: self, until, events: Vec::new(), files: Vec::new() };
        match File::open(host_log) {
            Ok(file) => found.read(file, acp)?,
            Err(e) if e.kind() == ErrorKind::NotFound => {
                return Err("the process keeps no transcript (log = false)".into());
            }
            Err(e) => return Err(format!("{}: {e}", host_log.display())),
        }
        for path in std::mem::take(&mut found.files) {
            let file = File::open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            found.read(file, acp)?;
        }
        let mut events = found.events;
        events.sort_by_key(|(seq, _)| *seq);
        // `exited` is in each session's file and the host log.
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
    fn read(&mut self, file: File, acp: bool) -> Result<(), String> {
        let f = self.follow;
        let ours = format!(r#""host_id":"{}""#, f.host_id);
        for line in BufReader::new(file).split(b'\n') {
            let line = line.map_err(|e| e.to_string())?;
            // Another process's, before or after this one's in the file.
            if !line.get(..200).unwrap_or(&line).windows(ours.len()).any(|w| w == ours.as_bytes()) {
                continue;
            }
            // Before or after what it missed: not read whole.
            if position(&line).is_some_and(|n| !self.missed(n)) {
                continue;
            }
            // As the host reads it (see json.rs), as the socket's events.
            let record = json::on_stack(json::depth(&line), || json::parse::<Value>(&line));
            let Some(Some(mut record)) = record else { continue };
            if record["host_id"] != *f.host_id {
                continue;
            }
            let event = &record["event"];
            match event["event"].as_str() {
                Some("session-opened") if event["session_id"].is_string() => {
                    if f.session.as_ref().is_some_and(|s| event["session_id"] != **s) {
                        continue;
                    }
                    let file = PathBuf::from(event["file"].as_str().unwrap_or_default());
                    let raw = event["acp_file"].as_str().map(PathBuf::from);
                    if acp && raw.is_none() {
                        return Err("the process keeps no raw ACP (log = \"events\")".into());
                    }
                    for path in [Some(file), raw.filter(|_| acp)].into_iter().flatten() {
                        if !self.files.contains(&path) {
                            self.files.push(path);
                        }
                    }
                }
                // A gap where what it missed was (ADR 6).
                Some("records-skipped") => {
                    let (first, last) = (event["first_seq"].as_u64(), event["last_seq"].as_u64());
                    if let (Some(first), Some(last)) = (first, last)
                        && last > f.last
                        && self.until.is_none_or(|until| first <= until)
                    {
                        let at = match event["error"].as_str() {
                            Some(error) => format!("a write failed: {error}"),
                            None => "the disk was behind".to_owned(),
                        };
                        return Err(format!(
                            "the transcript lacks events {first} to {last} ({at})"
                        ));
                    }
                }
                Some(name) => {
                    let Some(seq) = event["seq"].as_u64().filter(|&n| self.missed(n)) else {
                        continue;
                    };
                    if self.wanted(name, &event["session"]) {
                        self.events.push((seq, record["event"].take()));
                    }
                }
                // An ACP message, as `acp` events have it.
                None if acp => {
                    let Some(seq) = record["seq"].as_u64().filter(|&n| self.missed(n)) else {
                        continue;
                    };
                    let key = if record.get("msg").is_some() { "msg" } else { "raw" };
                    let Some(msg) = record.get_mut(key).map(Value::take) else { continue };
                    let session = record["session_id"].take();
                    if self.wanted("acp", &session) {
                        let event = json!({
                            "event": "acp",
                            "dir": record["dir"],
                            "session": session,
                            "msg": msg,
                            "ts": record["ts"],
                            "host_id": record["host_id"],
                            "seq": seq,
                        });
                        self.events.push((seq, event));
                    }
                }
                None => {}
            }
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

/// A record's position, read without reading all of it, for a line too
/// long to parse for nothing: an event's `seq` is its last key, so at the
/// record's end; an ACP message's comes before the message. None for a
/// record without one (a note of the logger's).
fn position(line: &[u8]) -> Option<u64> {
    const KEY: &[u8] = b"\"seq\":";
    let digits = |at: &[u8]| -> Option<u64> {
        let end = at.iter().position(|b| !b.is_ascii_digit()).unwrap_or(at.len());
        std::str::from_utf8(&at[..end]).ok()?.parse().ok()
    };
    let last = |within: &[u8]| -> Option<u64> {
        let at = within.windows(KEY.len()).rposition(|w| w == KEY)?;
        digits(&within[at + KEY.len()..])
    };
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    // What the record is follows its head (times, ids, pids and the session,
    // none with a quote unescaped in it), in its first KiB unless the
    // session's id is long, when the record is read whole.
    let head = &line[..line.len().min(1024)];
    let find = |key: &[u8]| head.windows(key.len()).position(|w| w == key);
    match (find(b"\"event\":"), find(b"\"dir\":")) {
        // `…,"event":{…,"seq":12}}`: an event's, ending the record.
        (Some(event), dir) if dir.is_none_or(|d| event < d) => {
            last(&line[line.len().saturating_sub(32)..]).filter(|_| line.ends_with(b"}}"))
        }
        // `…,"session_id":…,"seq":12,"dir":…`: an ACP message's, before it.
        (_, Some(dir)) => last(&head[..dir]),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adr_0069_a_records_position_is_read_from_its_end_or_head() {
        let event = br#"{"ts":"t","host_id":"h","event":{"event":"agent_message","text":"\"seq\":9","x":{"seq":7},"seq":12}}"#;
        assert_eq!(position(event), Some(12));
        let acp = br#"{"ts":"t","host_id":"h","session_id":"s","seq":13,"dir":"agent->editor","msg":{"seq":1}}"#;
        assert_eq!(position(acp), Some(13));
        let note = br#"{"ts":"t","host_id":"h","event":{"event":"records-skipped","count":1}}"#;
        assert_eq!(position(note), None);
        let raw = br#"{"ts":"t","host_id":"h","dir":"agent-stderr","msg":"x"}"#;
        assert_eq!(position(raw), None);
    }
}
