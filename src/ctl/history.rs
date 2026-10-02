//! `brnr log`: a session's transcript, running or not, as `watch` shows a
//! session live, from the events the host recorded in it.
//!
//! ```text
//! brnr log <target> [--session <id>] [--last <n>] [--follow] [--thoughts] [--raw | --json]
//! ```
//!
//! `--last <n>` starts at the n-th last message sent to the agent;
//! `--follow` keeps printing until the session's host exits; `--raw` prints
//! the transcript's records as they are, ACP included.

use std::fs::File;
use std::io::{self, BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::thread::sleep;
use std::time::Duration;

use serde_json::Value;

use brnr::host::alive;
use brnr::render;

use super::{USAGE, discover, inactive_sessions, resolve};

const POLL: Duration = Duration::from_millis(200);

pub(super) fn log(args: &[String]) -> Result<ExitCode, String> {
    let mut target = None;
    let mut session = None;
    let mut last = None;
    let (mut follow, mut thoughts, mut raw, mut json_out) = (false, false, false, false);
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--session" => session = Some(it.next().ok_or("--session needs an id")?.clone()),
            "--last" => {
                let n = it.next().ok_or("--last needs a number")?;
                last = Some(n.parse::<usize>().map_err(|_| format!("--last: not a number: {n}"))?);
            }
            "--follow" | "-f" => follow = true,
            "--thoughts" => thoughts = true,
            "--raw" => raw = true,
            "--json" => json_out = true,
            flag if flag.starts_with('-') => return Err(format!("unknown option: {flag}")),
            _ if target.is_none() => target = Some(arg.clone()),
            _ => return Err(USAGE.to_owned()),
        }
    }
    if raw && json_out {
        return Err("--raw and --json don't go together".into());
    }
    let target = target.ok_or(USAGE)?;
    let (path, host_pid) = transcript(&target, session.as_deref())?;
    let options = render::Options { session: false, thoughts, time: true };
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
        shown += print(&mut out, line, &options, raw, json_out)?;
    }
    if shown == 0 && !records.is_empty() && !raw && !follow {
        eprintln!("brnr: no events in {} (an older brnr wrote it); try --raw", path.display());
    }
    if !follow {
        return Ok(ExitCode::SUCCESS);
    }
    loop {
        let mut more = Vec::new();
        read_lines(&mut file, &mut partial, &mut more)?;
        for line in &more {
            print(&mut out, line, &options, raw, json_out)?;
            if record_event(line).is_some_and(|e| e["event"] == "exited") {
                return Ok(ExitCode::SUCCESS);
            }
        }
        if more.is_empty() && !host_pid.is_some_and(alive) {
            return Ok(ExitCode::SUCCESS);
        }
        sleep(POLL);
    }
}

/// The transcript `target` means and the pid of the host serving it, if
/// one is: a running session, or else an inactive one by id or prefix.
fn transcript(target: &str, session: Option<&str>) -> Result<(PathBuf, Option<i64>), String> {
    let hosts = discover();
    if let Ok((host, matched)) = resolve(&hosts, target) {
        let wanted = session.map(str::to_owned).or(matched);
        let sessions = host.sessions();
        let s = match (wanted, sessions) {
            (Some(id), _) => sessions
                .iter()
                .find(|s| s["session_id"].as_str().is_some_and(|s| s.starts_with(&id)))
                .ok_or(format!("no session {id} in host {}", host.id()))?,
            (None, [one]) => one,
            (None, []) => return Err(format!("host {} has no session yet", host.id())),
            (None, _) => {
                return Err(format!(
                    "host {} has several sessions; pick one with --session",
                    host.id()
                ));
            }
        };
        let path = s["log"].as_str().ok_or("this host keeps no transcript (log = false)")?;
        return Ok((PathBuf::from(path), host.meta["host_pid"].as_i64()));
    }
    let wanted = session.unwrap_or(target);
    let past = inactive_sessions(&hosts);
    let matches: Vec<&Value> = past
        .iter()
        .filter(|p| p["session_id"].as_str().is_some_and(|id| id.starts_with(wanted)))
        .collect();
    match matches[..] {
        [one] => Ok((PathBuf::from(one["log"].as_str().unwrap_or_default()), None)),
        [] => Err(format!("no host or session matches {wanted} (see brnr list --all)")),
        _ => Err(format!("{wanted} matches several sessions")),
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

/// A host event recorded in the transcript (see host/control.rs, `emit`).
fn record_event(line: &str) -> Option<Value> {
    let record: Value = serde_json::from_str(line).ok()?;
    let event = record.get("event")?;
    event["event"].is_string().then(|| event.clone())
}

fn is_message(line: &str) -> bool {
    record_event(line).is_some_and(|e| e["event"] == "user_message")
}

/// Prints one record as asked; returns how many lines it showed.
fn print(
    out: &mut impl Write,
    line: &str,
    options: &render::Options,
    raw: bool,
    json_out: bool,
) -> Result<usize, String> {
    let text = if raw {
        Some(line.trim_end().to_owned())
    } else {
        record_event(line)
            .and_then(|e| if json_out { Some(e.to_string()) } else { render::event(&e, options) })
    };
    let Some(text) = text else { return Ok(0) };
    match writeln!(out, "{text}").and_then(|()| out.flush()) {
        Ok(()) => Ok(1),
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => std::process::exit(0),
        Err(e) => Err(e.to_string()),
    }
}
