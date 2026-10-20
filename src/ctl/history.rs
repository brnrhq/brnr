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
use std::process::ExitCode;
use std::thread::sleep;
use std::time::Duration;

use serde_json::Value;

use crate::host::alive;
use crate::{render, transcript};

use super::{Args, default_events, events_arg, ownership};

const POLL: Duration = Duration::from_millis(200);

pub(super) fn log(a: &Args) -> Result<ExitCode, String> {
    let last = a.number("--last");
    let events = events_arg(a, &default_events())?;
    let (follow, json_out) = (a.has("--follow"), a.json);
    // A gap in the transcript (`records-skipped`, ADR 10.1) is always told: it
    // is what this reader relied on (P3).
    let wanted = |e: &Value| {
        e["event"] == "records-skipped" || events.iter().any(|name| e["event"] == name.as_str())
    };
    let show = Show { options: render::Options { session: false, time: true }, json_out };
    let target = a.positional[0].clone();
    let (path, acp_path, host_pid) = ownership::transcript(&target)?;
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

struct Show {
    options: render::Options,
    json_out: bool,
}

impl Show {
    /// Shows each of `events` that is `wanted`, up to the end of the
    /// session with `to_end`: how many lines it showed, and whether the
    /// session ended.
    fn all(
        &self,
        events: Vec<Value>,
        wanted: &impl Fn(&Value) -> bool,
        to_end: bool,
    ) -> Result<(usize, bool), String> {
        let mut shown = 0;
        for event in events {
            let end = to_end && (event["event"] == "exited" || event["event"] == "session_closed");
            shown += if wanted(&event) { self.print(&event)? } else { 0 };
            if end {
                return Ok((shown, true));
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
