//! `brnr event log`: a session's transcript, running or not, as `event watch`
//! shows a session live, from the events the host recorded in it.
//!
//! ```text
//! brnr event log <session> [--last <n>] [--follow] [--events <a,b,...>] [--json]
//! ```
//!
//! A session running or not, by its id. It starts at the beginning of
//! the session, or with `--last <n>` at the n-th last message sent to the
//! agent; `--follow` keeps printing until the session closes or its
//! process exits. `--events` and `--json` mean what they do for `event
//! watch`.
//! It reads the session's events file; with `acp` events chosen, the raw
//! ACP file beside it too, each message an `acp` event, merged in time
//! order (ADR 5.1 in docs/adr). A running session's, once its process has
//! written what it recorded until then (ADR 5.5).

use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::thread::sleep;
use std::time::Duration;

use serde_json::{Value, json};

use crate::host::alive;
use crate::{json, paths, render, transcript};

use super::client::discover;
use super::{Found, USAGE, closing_in, default_events, events_arg, find_session};

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
            "--follow" => follow = true,
            "--events" => events = events_arg(it.next(), &default_events())?,
            "--json" => json_out = true,
            flag if flag.starts_with("--") => return Err(format!("unknown option: {flag}")),
            _ if target.is_none() => target = Some(arg.clone()),
            _ => return Err(USAGE.to_owned()),
        }
    }
    // A gap in the transcript (`records-skipped`, ADR 10.1) is always told: it
    // is what this reader relied on (P3).
    let wanted = |e: &Value| {
        e["event"] == "records-skipped" || events.iter().any(|name| e["event"] == name.as_str())
    };
    let show = Show { options: render::Options { session: false, time: true }, json_out };
    let target = target.ok_or(USAGE)?;
    let (path, acp_path, host_pid) = find(&target)?;
    let mut acp = None;
    if events.iter().any(|e| e == "acp") {
        match acp_path {
            Some(raw) if raw.exists() => acp = Some(raw),
            _ => errln!("brnr: no raw ACP for {target}: log = \"events\" leaves it out"),
        }
    }
    let mut reader = transcript::Reader::open(&path, acp.as_deref())?;

    // Everything so far, from the n-th last message on.
    let mut records = reader.read(!follow)?;
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
        // ACP held back (see `Reader::read`) is from before, too.
        reader.forget_held();
    }
    let none = records.is_empty();
    let no_events = !records.iter().any(|e| e["event"] != "acp");
    let shown_now = records.split_off(from);
    json::free(Value::Array(records));
    let (shown, _) = show.all(shown_now, &wanted, false)?;
    if shown == 0 && no_events && !none && !follow {
        eprintln!(
            "brnr: no events in {} (an older brnr wrote it); try --events acp",
            path.display()
        );
    }
    // A line cut short at the end is still being written while its process
    // runs; once it has gone, it is a record cut short (ADR 5.1).
    let has_gone = || !host_pid.is_some_and(alive);
    if !follow {
        if has_gone() {
            reader.cut_short();
        }
        return Ok(ExitCode::SUCCESS);
    }
    loop {
        // Looked at first: what the process wrote before it went is read.
        let gone = has_gone();
        // Nothing more comes once the session closes or the agent exits.
        if show.all(reader.read(gone)?, &wanted, true)?.1 {
            return Ok(ExitCode::SUCCESS);
        }
        if gone {
            reader.cut_short();
            return Ok(ExitCode::SUCCESS);
        }
        sleep(POLL);
    }
}

/// The files of the session `arg` names, its events and its raw ACP (none
/// if its process records none), and the pid of the process serving it, if
/// one is. A running session's process is asked to have written what it
/// recorded until now first: a thread of its own writes it, and what was
/// just seen happen (a turn `session new --wait` reported) may not be there
/// yet (ADR 5.5).
///
/// One closing in its process is read once that process has written its
/// records, `session_closed` last, and let go of it (ADR 5.5).
fn find(arg: &str) -> Result<(PathBuf, Option<PathBuf>, Option<i64>), String> {
    let mut hosts = discover()?;
    if let Some(host) = closing_in(&hosts, arg) {
        // Answered once what it recorded before is written: the close's
        // records, and the lock gone with them.
        if let Err(e) = host.call(&json!({ "cmd": "logged" })) {
            errln!("brnr: {e}: what it recorded last may not be written yet (a slow disk?)");
        }
        hosts = discover()?;
    }
    match find_session(&hosts, arg)? {
        Found::Running(host, id) => {
            let s = host.session(&id);
            let path = s
                .and_then(|s| s["log"].as_str())
                .ok_or("this process keeps no transcript (log = false)")?;
            let acp = s.and_then(|s| s["acp_log"].as_str()).map(PathBuf::from);
            if let Err(e) = host.call(&json!({ "cmd": "logged" })) {
                errln!("brnr: {e}: what it recorded last may not be written yet (a slow disk?)");
            }
            Ok((PathBuf::from(path), acp, host.meta["host_pid"].as_i64()))
        }
        Found::Inactive(past) => {
            let path = PathBuf::from(past["log"].as_str().unwrap_or_default());
            Ok((path.clone(), Some(paths::acp_log(&path)), None))
        }
    }
}

struct Show {
    options: render::Options,
    json_out: bool,
}

impl Show {
    /// Shows each of `events` that is `wanted`, up to the end of the
    /// session with `to_end`, and lets go of each on a stack that takes it
    /// (ADR 2.5): how many lines it showed, and whether the session ended.
    fn all(
        &self,
        events: Vec<Value>,
        wanted: &(impl Fn(&Value) -> bool + Sync),
        to_end: bool,
    ) -> Result<(usize, bool), String> {
        let (mut shown, mut events) = (0, events.into_iter());
        while let Some(event) = events.next() {
            let end = to_end && (event["event"] == "exited" || event["event"] == "session_closed");
            let one = json::holding(event, |e| if wanted(&e) { self.print(&e) } else { Ok(0) });
            match one.and_then(|one| one) {
                Ok(n) if !end => shown += n,
                done => {
                    json::free(Value::Array(events.collect()));
                    return done.map(|n| (shown + n, true));
                }
            }
        }
        Ok((shown, false))
    }

    /// Prints one event as asked; returns how many lines it showed.
    fn print(&self, event: &Value) -> Result<usize, String> {
        let text = if self.json_out {
            Some(event.to_string())
        } else {
            render::event(event, &self.options)
        };
        let Some(text) = text else { return Ok(0) };
        let mut out = io::stdout().lock();
        match writeln!(out, "{}", render::clean(&text)).and_then(|()| out.flush()) {
            Ok(()) => Ok(1),
            Err(e) if e.kind() == io::ErrorKind::BrokenPipe => std::process::exit(0),
            Err(e) => Err(e.to_string()),
        }
    }
}
