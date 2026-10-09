//! `session delete` (ADR 63): `session/delete`, asked of an agent started
//! just for that, as `session list` asks for `session/list` (no process of
//! brnr's, see sessions.rs), and with `--purge` brnr's transcript of the
//! session deleted too.
//!
//! The agent is the one brnr recorded for the session, as a resume finds it
//! (ADR 14), or `--profile` and `-- <agent>`, named for a session brnr has
//! no transcript of. A session open in a process is refused: it is closed
//! first. Its lock is held while the agent deletes it, so that no process
//! opens it meanwhile.
//!
//! Without `--purge` the transcript stays, and `session_deleted` is appended
//! to it, a record of no process's (`host_id` null: none has the session
//! open). With it, the events file and the raw ACP are deleted (see
//! `log::purge`), but not the host logs, which are shared; an agent that
//! didn't delete the session (it may no longer have it) is said, and the
//! transcript is deleted all the same (P3).

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::SystemTime;

use serde_json::{Value, json};

use brnr::{lock, log};

use super::sessions::{Asked, named_agent};
use super::{Found, USAGE, discover, find_session, print_json};

pub(super) fn delete(args: &[String]) -> Result<ExitCode, String> {
    let (mut session, mut purge, mut profile, mut json_out) = (None, false, None, false);
    let mut agent = Vec::new();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--purge" => purge = true,
            "--profile" => profile = Some(it.next().ok_or("--profile needs a name")?.clone()),
            "--json" => json_out = true,
            "--" => {
                agent = it.by_ref().cloned().collect();
                break;
            }
            flag if flag.starts_with("--") => return Err(format!("unknown option: {flag}")),
            _ if session.is_none() => session = Some(arg.clone()),
            _ => return Err(USAGE.to_owned()),
        }
    }
    let id = session.ok_or(USAGE)?;
    let open = |pid: &str| {
        format!("{id} is open in process {pid}: close it first (brnr session close {id})")
    };
    if let Some(pid) = lock::holder(&id) {
        return Err(open(&pid.to_string()));
    }
    // What brnr recorded of it, as a resume has it, if anything.
    let hosts = discover()?;
    let past = match find_session(&hosts, &id) {
        Ok(Found::Running(host, _)) => return Err(open(host.id())),
        Ok(Found::Inactive(past)) => Some(past),
        Err(_) => None,
    };
    let recorded = || {
        let argv = past.as_ref().and_then(|p| p["agent"].as_array());
        let argv = argv.into_iter().flatten().filter_map(Value::as_str).map(str::to_owned);
        Some(argv.collect::<Vec<_>>()).filter(|a| !a.is_empty())
    };
    let agent = match named_agent(profile.as_deref(), agent)?.or_else(recorded) {
        Some(agent) => agent,
        None if past.is_some() => {
            return Err("no agent recorded for it: give one after -- or with --profile".into());
        }
        None => {
            return Err(format!(
                "brnr has no transcript of {id}: name its agent (--profile or -- <agent>)"
            ));
        }
    };
    // Taken last: a session that is open is refused above, naming its process.
    let held = lock::take(&id).map_err(|e| match e {
        lock::Error::Held(pid) => open(&pid.to_string()),
        lock::Error::Io(why) => format!("{id} can't be locked: {why}"),
    })?;
    let (mut asked, caps) = Asked::start(&agent)?;
    if caps.session_capabilities.delete.is_none() {
        return Err("the agent can't delete sessions (no sessionCapabilities.delete)".into());
    }
    let agent_pid = asked.pid();
    let answer = asked.ask("session/delete", json!({ "sessionId": id })).map(drop);
    drop(asked);
    if let (Err(error), false) = (&answer, purge) {
        return Err(error.clone());
    }
    let (mut recorded, mut purged, mut failed) = (Vec::new(), Vec::new(), Vec::new());
    if purge {
        (purged, failed) = log::purge(&id);
        if let Err(error) = &answer {
            if purged.is_empty() && failed.is_empty() {
                return Err(format!("{error}; brnr has no transcript of {id}"));
            }
            errln!(
                "brnr: the agent didn't delete {id} ({error}): it may no longer have it; deleted \
                 brnr's transcript of it all the same"
            );
        }
    } else {
        let event = json!({
            "event": "session_deleted",
            "session": id,
            "by": "delete",
            "ts": log::rfc3339(SystemTime::now()),
            "host_id": null,
        });
        recorded = log::record_in_transcripts(&id, agent_pid, &event)
            .map_err(|e| format!("deleted {id}, but its session_deleted isn't recorded: {e}"))?;
    }
    drop(held);
    let paths = |files: &[PathBuf]| -> Vec<String> {
        files.iter().map(|p| p.to_string_lossy().into_owned()).collect()
    };
    if json_out {
        print_json(&json!({
            "session": id,
            "deleted": answer.is_ok(),
            "error": answer.as_ref().err(),
            "recorded": paths(&recorded),
            "purged": paths(&purged),
            "failed": failed,
        }))?;
    } else if !purge {
        match recorded.is_empty() {
            true => outln!("deleted {id}; brnr has no transcript of it"),
            false => outln!("deleted {id}; brnr's transcript of it stays (brnr event log {id})"),
        }
    } else {
        let what = if answer.is_ok() { format!("deleted {id}, and") } else { "deleted".to_owned() };
        match purged.is_empty() {
            true => outln!("deleted {id}; brnr had no transcript of it"),
            false => outln!("{what} brnr's transcript of {id}:"),
        }
        for path in paths(&purged) {
            outln!("  {path}");
        }
    }
    if failed.is_empty() {
        return Ok(ExitCode::SUCCESS);
    }
    for why in &failed {
        errln!("brnr: not deleted: {why}");
    }
    Ok(ExitCode::FAILURE)
}
