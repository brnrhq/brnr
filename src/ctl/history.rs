//! `brnr log`: a session's transcript, running or not, as `watch` shows a
//! session live, from the events the host recorded in it.
//!
//! ```text
//! brnr log <session> [--last <n>] [--follow] [--events <a,b,...>] [--json]
//! ```
//!
//! A session running or not, by id or name. It starts at the beginning of
//! the session, or with `--last <n>` at the n-th last message sent to the
//! agent; `--follow` keeps printing until the session's process exits. `--events` and `--json` mean what they do for
//! `watch`; an ACP message in the transcript is an `acp` event.

use std::fs::File;
use std::io::{self, BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::thread::sleep;
use std::time::Duration;

use serde_json::{Value, json};

use brnr::host::alive;
use brnr::render;

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
    let wanted = |e: &Value| events.iter().any(|name| e["event"] == name.as_str());
    let show = Show { options: render::Options { session: false, time: true }, json_out };
    let target = target.ok_or(USAGE)?;
    let (path, host_pid) = transcript(&target)?;
    let mut file =
        BufReader::new(File::open(&path).map_err(|e| format!("{}: {e}", path.display()))?);

    // Everything so far, from the n-th last message on.
    let mut records = Vec::new();
    let mut partial = String::new();
    read_lines(&mut file, &mut partial, &mut records)?;
    let from = match last {
        Some(n) => {
            let starts: Vec<usize> =
                (0..records.len()).filter(|&i| is_message(&records[i])).collect();
            starts.len().checked_sub(n).map_or(0, |k| starts[k])
        }
        None => 0,
    };
    let mut out = io::stdout().lock();
    let mut shown = 0;
    for line in &records[from..] {
        shown += show.print(&mut out, record_event(line).filter(wanted))?;
    }
    let no_events = !records.iter().any(|l| record_event(l).is_some_and(|e| e["event"] != "acp"));
    if shown == 0 && no_events && !records.is_empty() && !follow {
        eprintln!("brnr: no events in {} (an older brnr wrote it); try --events acp", path.display());
    }
    if !follow {
        return Ok(ExitCode::SUCCESS);
    }
    loop {
        let mut more = Vec::new();
        read_lines(&mut file, &mut partial, &mut more)?;
        for line in &more {
            let event = record_event(line);
            let exited = event.as_ref().is_some_and(|e| e["event"] == "exited");
            show.print(&mut out, event.filter(wanted))?;
            if exited {
                return Ok(ExitCode::SUCCESS);
            }
        }
        if more.is_empty() && !host_pid.is_some_and(alive) {
            return Ok(ExitCode::SUCCESS);
        }
        sleep(POLL);
    }
}

/// The transcript of the session `arg` names, and the pid of the process
/// serving it, if one is.
fn transcript(arg: &str) -> Result<(PathBuf, Option<i64>), String> {
    let hosts = discover();
    match find_session(&hosts, arg)? {
        Found::Running(host, id) => {
            let s = host.sessions().iter().find(|s| s["session_id"] == id.as_str());
            let path = s
                .and_then(|s| s["log"].as_str())
                .ok_or("this process keeps no transcript (log = false)")?;
            Ok((PathBuf::from(path), host.meta["host_pid"].as_i64()))
        }
        Found::Inactive(past) => Ok((PathBuf::from(past["log"].as_str().unwrap_or_default()), None)),
    }
}

/// Complete lines appended since the last read; a line still being
/// written stays in `partial`.
fn read_lines(
    file: &mut BufReader<File>,
    partial: &mut String,
    out: &mut Vec<String>,
) -> Result<(), String> {
    loop {
        let n = file.read_line(partial).map_err(|e| e.to_string())?;
        if n == 0 || !partial.ends_with('\n') {
            return Ok(());
        }
        out.push(std::mem::take(partial));
    }
}

/// A transcript record as the event `watch` would have shown: a host event
/// (see host/control.rs, `emit`), or an ACP message as an `acp` event.
fn record_event(line: &str) -> Option<Value> {
    let mut record: Value = serde_json::from_str(line).ok()?;
    if record["event"]["event"].is_string() {
        return Some(record["event"].take());
    }
    let msg = record.get("msg").or(record.get("raw"))?;
    Some(json!({
        "event": "acp",
        "ts": record["ts"],
        "host_id": record["host_id"],
        "session": record["session_id"],
        "dir": record["dir"],
        "msg": msg,
    }))
}

fn is_message(line: &str) -> bool {
    record_event(line).is_some_and(|e| e["event"] == "user_message")
}

struct Show {
    options: render::Options,
    json_out: bool,
}

impl Show {
    /// Prints one event as asked; returns how many lines it showed.
    fn print(&self, out: &mut impl Write, event: Option<Value>) -> Result<usize, String> {
        let text = event.and_then(|e| {
            if self.json_out { Some(e.to_string()) } else { render::event(&e, &self.options) }
        });
        let Some(text) = text else { return Ok(0) };
        match writeln!(out, "{text}").and_then(|()| out.flush()) {
            Ok(()) => Ok(1),
            Err(e) if e.kind() == io::ErrorKind::BrokenPipe => std::process::exit(0),
            Err(e) => Err(e.to_string()),
        }
    }
}
