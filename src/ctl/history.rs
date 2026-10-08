//! `brnr log`: a session's transcript, running or not, as `watch` shows a
//! session live, from the events the host recorded in it.
//!
//! ```text
//! brnr log <session> [--last <n>] [--follow] [--events <a,b,...>] [--json]
//! ```
//!
//! A session running or not, by id or name. It starts at the beginning of
//! the session, or with `--last <n>` at the n-th last message sent to the
//! agent; `--follow` keeps printing until the session closes or its
//! process exits. `--events` and `--json` mean what they do for `watch`.
//! It reads the session's events file; with `acp` events chosen, the raw
//! ACP file beside it too, each message an `acp` event, merged in time
//! order (ADR 22 in docs/adr).

use std::fs::File;
use std::io::{self, BufRead, BufReader, Write};
use std::mem::take;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::thread::sleep;
use std::time::Duration;

use serde_json::{Value, json};

use brnr::host::alive;
use brnr::{paths, render};

use super::{Found, USAGE, default_events, discover, events_arg, find_session};

const POLL: Duration = Duration::from_millis(200);

pub(super) fn log(args: &[String]) -> Result<ExitCode, String> {
    let mut target = None;
    let mut last = None;
    let mut events = default_events();
    let (mut follow, mut json_out) = (false, false);
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--last" => {
                let n = it.next().ok_or("--last needs a number")?;
                last = Some(n.parse::<usize>().map_err(|_| format!("--last: not a number: {n}"))?);
            }
            "--follow" | "-f" => follow = true,
            "--events" => events = events_arg(it.next(), &default_events())?,
            "--json" => json_out = true,
            flag if flag.starts_with('-') => return Err(format!("unknown option: {flag}")),
            _ if target.is_none() => target = Some(arg.clone()),
            _ => return Err(USAGE.to_owned()),
        }
    }
    // A gap in the transcript (`records-skipped`, ADR 6) is always told: it
    // is what this reader relied on (P3).
    let wanted = |e: &Value| {
        e["event"] == "records-skipped" || events.iter().any(|name| e["event"] == name.as_str())
    };
    let show = Show { options: render::Options { session: false, time: true }, json_out };
    let target = target.ok_or(USAGE)?;
    let (path, acp_path, host_pid) = transcript(&target)?;
    let open = |path: &Path| Tail::open(path).map_err(|e| format!("{}: {e}", path.display()));
    let mut acp = None;
    if events.iter().any(|e| e == "acp") {
        match acp_path {
            Some(raw) if raw.exists() => acp = Some(open(&raw)?),
            _ => errln!("brnr: no raw ACP for {target}: log = \"events\" leaves it out"),
        }
    }
    let events_file = open(&path)?;
    let mut transcript =
        Transcript { events: events_file, acp, last: String::new(), held: Vec::new() };

    // Everything so far, from the n-th last message on.
    let records = transcript.read(!follow)?;
    let from = match last {
        Some(n) => {
            let starts: Vec<usize> =
                (0..records.len()).filter(|&i| records[i]["event"] == "user_message").collect();
            match starts.len().checked_sub(n) {
                // `--last 0`: none of them, only what comes next.
                Some(k) => starts.get(k).copied().unwrap_or(records.len()),
                None => 0,
            }
        }
        None => 0,
    };
    if last == Some(0) {
        // ACP held back (see `Transcript::read`) is from before, too.
        transcript.held.clear();
    }
    let mut out = io::stdout().lock();
    let mut shown = 0;
    for event in records[from..].iter().filter(|e| wanted(e)) {
        shown += show.print(&mut out, event)?;
    }
    let no_events = !records.iter().any(|e| e["event"] != "acp");
    if shown == 0 && no_events && !records.is_empty() && !follow {
        eprintln!("brnr: no events in {} (an older brnr wrote it); try --events acp", path.display());
    }
    if !follow {
        return Ok(ExitCode::SUCCESS);
    }
    loop {
        // Looked at first: what the process wrote before it went is read.
        let gone = !host_pid.is_some_and(alive);
        for event in transcript.read(gone)? {
            // Nothing more comes once the session closes or the agent exits.
            let end = event["event"] == "exited" || event["event"] == "session_closed";
            if wanted(&event) {
                show.print(&mut out, &event)?;
            }
            if end {
                return Ok(ExitCode::SUCCESS);
            }
        }
        if gone {
            return Ok(ExitCode::SUCCESS);
        }
        sleep(POLL);
    }
}

/// The files of the session `arg` names, its events and its raw ACP (none
/// if its process records none), and the pid of the process serving it, if
/// one is.
fn transcript(arg: &str) -> Result<(PathBuf, Option<PathBuf>, Option<i64>), String> {
    let hosts = discover()?;
    match find_session(&hosts, arg)? {
        Found::Running(host, id) => {
            let s = host.sessions().iter().find(|s| s["session_id"] == id.as_str());
            let path = s
                .and_then(|s| s["log"].as_str())
                .ok_or("this process keeps no transcript (log = false)")?;
            let acp = s.and_then(|s| s["acp_log"].as_str()).map(PathBuf::from);
            Ok((PathBuf::from(path), acp, host.meta["host_pid"].as_i64()))
        }
        Found::Inactive(past) => {
            let path = PathBuf::from(past["log"].as_str().unwrap_or_default());
            Ok((path.clone(), Some(paths::acp_log(&path)), None))
        }
    }
}

/// A session's two files, read as they grow: its events, and its raw ACP
/// when `acp` events are chosen.
struct Transcript {
    events: Tail,
    acp: Option<Tail>,
    /// When the last record read from the events file was written.
    last: String,
    /// ACP written after it, held for one read (see `read`).
    held: Vec<(String, Value)>,
}

impl Transcript {
    /// What was written since the last read, in time order: each record as
    /// the event `watch` would have shown. `all` for everything read, of a
    /// transcript that won't grow.
    ///
    /// One writer writes both files, each in time order, and the events file
    /// is read first: ACP up to the last event read is all there, but ACP
    /// past it may follow an event written since. That ACP is held, and goes
    /// with the next read, once the events file has been read again.
    fn read(&mut self, all: bool) -> Result<Vec<Value>, String> {
        let events = self.events.read()?;
        if let Some((ts, _)) = events.last() {
            self.last.clone_from(ts);
        }
        let mut acp = take(&mut self.held);
        if let Some(tail) = &mut self.acp {
            let (now, later): (Vec<_>, _) =
                tail.read()?.into_iter().partition(|(ts, _)| all || *ts <= self.last);
            acp.extend(now);
            self.held = later;
        }
        // Both in time order: merged, events first at the same time.
        let mut merged = Vec::with_capacity(events.len() + acp.len());
        let (mut events, mut acp) = (events.into_iter().peekable(), acp.into_iter().peekable());
        loop {
            let next = match (events.peek(), acp.peek()) {
                (Some((e, _)), Some((a, _))) if a < e => acp.next(),
                (Some(_), _) => events.next(),
                (None, _) => acp.next(),
            };
            let Some((_, event)) = next else { return Ok(merged) };
            merged.push(event);
        }
    }
}

/// One file of a transcript, read as it grows.
struct Tail {
    file: BufReader<File>,
    /// A line still being written.
    partial: String,
}

impl Tail {
    fn open(path: &Path) -> io::Result<Tail> {
        Ok(Tail { file: BufReader::new(File::open(path)?), partial: String::new() })
    }

    /// The records appended since the last read, with when each was
    /// written.
    fn read(&mut self) -> Result<Vec<(String, Value)>, String> {
        let mut records = Vec::new();
        loop {
            let n = self.file.read_line(&mut self.partial).map_err(|e| e.to_string())?;
            if n == 0 || !self.partial.ends_with('\n') {
                return Ok(records);
            }
            records.extend(record(&take(&mut self.partial)));
        }
    }
}

/// A transcript record as the event `watch` would have shown, a host event
/// (see host/control.rs, `emit`) or an ACP message as an `acp` event, with
/// when it was written.
fn record(line: &str) -> Option<(String, Value)> {
    let mut record: Value = serde_json::from_str(line).ok()?;
    let ts = record["ts"].as_str().unwrap_or_default().to_owned();
    if record["event"]["event"].is_string() {
        return Some((ts, record["event"].take()));
    }
    let msg = record.get("msg").or(record.get("raw"))?;
    let event = json!({
        "event": "acp",
        "ts": record["ts"],
        "host_id": record["host_id"],
        "session": record["session_id"],
        "dir": record["dir"],
        "msg": msg,
    });
    Some((ts, event))
}

struct Show {
    options: render::Options,
    json_out: bool,
}

impl Show {
    /// Prints one event as asked; returns how many lines it showed.
    fn print(&self, out: &mut impl Write, event: &Value) -> Result<usize, String> {
        let text = if self.json_out {
            Some(event.to_string())
        } else {
            render::event(event, &self.options)
        };
        let Some(text) = text else { return Ok(0) };
        match writeln!(out, "{}", render::clean(&text)).and_then(|()| out.flush()) {
            Ok(()) => Ok(1),
            Err(e) if e.kind() == io::ErrorKind::BrokenPipe => std::process::exit(0),
            Err(e) => Err(e.to_string()),
        }
    }
}
