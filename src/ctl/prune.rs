//! `session prune` and the config's `retention` (ADR 9.5): brnr's
//! transcripts of the sessions last active before a time, and the host logs
//! of the processes that have gone and were last active before it, deleted.
//!
//! A session's age is its last activity, the newest record a process
//! recorded in any of its files that isn't a deletion, as `session list` has
//! it (`last_active`, ADR 5.1); a host log's, its newest record. Never
//! pruned: a session open in a process, or whose lock is held, and the host
//! log of a process that is running, here or with another runtime directory
//! (its pid alive, and no `process_exited` at its end). A session's files
//! are deleted holding its lock, and only if it is still that old then, as
//! `session delete --purge` deletes them (`log::purge`). What brnr doesn't
//! read, a file in another format (ADR 5.10) or with no record, isn't
//! pruned: brnr can't tell its age.
//!
//! `session prune --older-than <s>` prunes when asked, and `--dry-run` says
//! what it would delete. With `retention` set, every process prunes as it
//! starts ([`as_started`]), on a thread of its own, and records what it
//! deleted in its host log as `transcripts_pruned`, with the fields
//! `--json` has. The agent's own copy of a session isn't touched: that is
//! `session delete`'s.

use std::ffi::OsStr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::error;
use crate::log::{self, Sink};
use crate::{lock, transcript};

use super::args::{self, Args};
use super::{active_at, discover, duration, inactive_sessions, print_json, when};

/// What a prune deleted, or with `dry_run` would delete.
struct Pruned {
    older_than: u64,
    before: SystemTime,
    dry_run: bool,
    /// `{session, last_active, files}` for each session.
    sessions: Vec<Value>,
    /// `{host_id, last_active, files}` for each host log.
    host_logs: Vec<Value>,
    /// Why a file wasn't deleted, or a session's lock taken.
    failed: Vec<String>,
}

impl Pruned {
    fn json(&self) -> Value {
        json!({
            "older_than": self.older_than,
            "before": log::rfc3339(self.before),
            "dry_run": self.dry_run,
            "sessions": self.sessions,
            "host_logs": self.host_logs,
            "failed": self.failed,
        })
    }
}

/// `brnr session prune --older-than <s> [--dry-run] [--json]`.
pub(super) fn prune(a: &Args) -> Result<ExitCode, args::Error> {
    let Some(older_than) = a.number::<u64>("--older-than") else {
        return Err(args::Error::Invalid("--older-than <s> is needed".into()));
    };
    let pruned = run(older_than, a.has("--dry-run"), None)?;
    if a.json {
        print_json(&pruned.json())?;
    } else {
        say(&pruned);
    }
    for why in &pruned.failed {
        errln!("brnr: not deleted: {why}");
    }
    match pruned.failed.is_empty() {
        true => Ok(ExitCode::SUCCESS),
        false => Ok(ExitCode::FAILURE),
    }
}

/// What a process with `retention` set prunes as it starts: what was last
/// active more than `older_than` seconds ago, but `resumed`, the session a
/// headless start resumes. Recorded in the host log as `transcripts_pruned`
/// when something was deleted or couldn't be.
pub(crate) fn as_started(older_than: u64, resumed: Option<String>, sink: &Sink) {
    let pruned = run(older_than, false, resumed.as_deref()).unwrap_or_else(|e| Pruned {
        older_than,
        before: cut_off(older_than),
        dry_run: false,
        sessions: Vec::new(),
        host_logs: Vec::new(),
        failed: vec![e.message],
    });
    if pruned.sessions.is_empty() && pruned.host_logs.is_empty() && pruned.failed.is_empty() {
        return;
    }
    let mut record = json!({ "event": "transcripts_pruned" });
    if let (Some(record), Value::Object(fields)) = (record.as_object_mut(), pruned.json()) {
        record.extend(fields.into_iter().filter(|(k, _)| k != "dry_run"));
    }
    sink.note(None, record);
}

/// `older_than` seconds before now.
fn cut_off(older_than: u64) -> SystemTime {
    SystemTime::now().checked_sub(Duration::from_secs(older_than)).unwrap_or(UNIX_EPOCH)
}

/// Prunes, or with `dry_run` finds, what was last active more than
/// `older_than` seconds ago, leaving `leave`. Which sessions are open is
/// what the processes and the locks say: one that can't be known fails it
/// all, as nothing open may be deleted.
fn run(older_than: u64, dry_run: bool, leave: Option<&str>) -> Result<Pruned, error::Error> {
    let before = cut_off(older_than);
    let hosts = discover()?;
    let mut pruned = Pruned {
        older_than,
        before,
        dry_run,
        sessions: Vec::new(),
        host_logs: Vec::new(),
        failed: Vec::new(),
    };
    // Those open in a process, or whose lock is held, aren't listed.
    for past in inactive_sessions(&hosts) {
        let Some(id) = past["session"].as_str().filter(|id| Some(*id) != leave) else { continue };
        if !older(&past["last_active"], before) {
            continue;
        }
        let (files, failed) = match dry_run {
            true => log::purge(id, true),
            // Its lock held while its files go, and only if nothing has
            // happened in it since it was listed.
            false => {
                match lock::holding(id, || still_old(id, before).then(|| log::purge(id, false))) {
                    Ok(Some(done)) => done,
                    Ok(None) | Err(lock::Error::Held(_)) => continue,
                    Err(lock::Error::Io(why)) => {
                        (Vec::new(), vec![format!("{id} can't be locked: {why}")])
                    }
                }
            }
        };
        pruned.failed.extend(failed);
        if !files.is_empty() {
            let files: Vec<String> =
                files.iter().map(|p| p.to_string_lossy().into_owned()).collect();
            let last_active = &past["last_active"];
            pruned
                .sessions
                .push(json!({ "session": id, "last_active": last_active, "files": files }));
        }
    }
    let running: Vec<&str> = hosts.iter().filter_map(|h| h.meta["host_id"].as_str()).collect();
    for path in transcript::host_logs() {
        let Some(run) = path.file_stem().and_then(OsStr::to_str) else { continue };
        if running.contains(&run) {
            continue;
        }
        let Some(last) = transcript::newest(&path).filter(transcript::readable) else { continue };
        // A process listed with another runtime directory, or still writing.
        let ended = super::doctor::recorded_exit(&path) == Some(true);
        if !older(&last["ts"], before) || (!ended && last["pid"].as_i64().is_some_and(alive)) {
            continue;
        }
        match log::purge_host_log(&path, dry_run) {
            Ok(true) => pruned.host_logs.push(json!({
                "host_id": run,
                "last_active": last["ts"],
                "files": [path.to_string_lossy()],
            })),
            Ok(false) => {}
            Err(why) => pruned.failed.push(why),
        }
    }
    Ok(pruned)
}

fn alive(pid: i64) -> bool {
    crate::host::alive(pid)
}

/// Whether `ts`, an RFC 3339 time, is before `before`. One that isn't a
/// time isn't: brnr doesn't guess an age (P4).
fn older(ts: &Value, before: SystemTime) -> bool {
    ts.as_str().and_then(log::parse_rfc3339).is_some_and(|t| t < before)
}

/// Whether session `id`, its lock held, still was last active before
/// `before`: a process may have opened it, and closed it again, since it was
/// listed.
fn still_old(id: &str, before: SystemTime) -> bool {
    let last = |file: PathBuf| {
        let newest = transcript::newest(&file).filter(transcript::readable)?;
        Some(active_at(&file, &newest))
    };
    transcript::of_session(id).into_iter().filter_map(last).all(|at| older(&at, before))
}

/// The text of what `pruned` did, the same data as its JSON (P5).
fn say(pruned: &Pruned) {
    let before = when(&log::rfc3339(pruned.before));
    let ago = age(pruned.older_than);
    if pruned.sessions.is_empty() && pruned.host_logs.is_empty() {
        let would = if pruned.dry_run { " to prune" } else { "" };
        outln!("nothing{would} was last active before {before} (more than {ago} ago)");
        return;
    }
    let what = if pruned.dry_run { "would prune" } else { "pruned" };
    outln!("{what} what was last active before {before} (more than {ago} ago):");
    let items = pruned.sessions.iter().map(|s| ("session", &s["session"], s));
    let items = items.chain(pruned.host_logs.iter().map(|h| ("host log", &h["host_id"], h)));
    for (kind, name, item) in items {
        let at = when(item["last_active"].as_str().unwrap_or_default());
        outln!("  {kind} {}, last active {at}", name.as_str().unwrap_or_default());
        for file in item["files"].as_array().into_iter().flatten().filter_map(Value::as_str) {
            outln!("    {file}");
        }
    }
}

/// `30 days`, `1 day`, or as `session status` says a duration (`2h05m`).
pub(super) fn age(secs: u64) -> String {
    match secs / 86400 {
        1 if secs.is_multiple_of(86400) => "1 day".to_owned(),
        days if days > 0 && secs.is_multiple_of(86400) => format!("{days} days"),
        _ => duration(secs),
    }
}
