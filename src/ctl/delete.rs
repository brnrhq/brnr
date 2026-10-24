//! `session delete` (ADR 8.1): `session/delete`, asked of an agent started
//! just for that, as `session list` asks for `session/list` (no process of
//! brnr's, see sessions.rs), and with `--purge` brnr's transcript of the
//! session deleted too.
//!
//! The agent is the one brnr recorded for the session, as a resume finds it
//! (ADR 3.11), or `--profile` and `-- <agent>`, named for a session brnr has
//! no transcript of. A session open in a process is refused: it is closed
//! first. Its lock is held while the agent deletes it, so that no process
//! opens it meanwhile.
//!
//! Without `--purge` the transcript stays, and `session_deleted` is appended
//! to it, a record of no process's (`host_id` null: none has the session
//! open). With it, the events file and the raw ACP are deleted, an older
//! brnr's and those set aside beside them too (see `log::purge`), but not
//! the host logs, which are shared, whatever the
//! agent answered. An agent that doesn't have the session (ACP's
//! `resource_not_found`) is said, and the command succeeds; any other error,
//! or no answer, is said, and the command fails: the agent may still have
//! the session (P3, P7). A file that couldn't be deleted, or have
//! `session_deleted` recorded in it, is said and fails the command too,
//! the others done all the same.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::SystemTime;

use serde_json::{Value, json};

use crate::error::{self, Code};
use crate::schema::{self, ErrorCode};
use crate::{lock, log};

use super::client::discover;
use super::sessions::{Asked, named_agent};
use super::{Args, ownership, print_json};

pub(super) fn delete(a: &Args) -> Result<ExitCode, error::Error> {
    let (purge, profile, json_out) = (a.has("--purge"), a.value("--profile"), a.json);
    let agent = a.rest.clone().unwrap_or_default();
    let id = a.positional[0].clone();
    // What brnr recorded of it, as a resume has it, if anything; one open in
    // a process, or closing there, is refused (ADR 3.9, ADR 5.5).
    let past = ownership::locate(&discover()?, &id).delete()?;
    let recorded = || {
        let argv = past.as_ref().and_then(|p| p["agent"].as_array());
        let argv = argv.into_iter().flatten().filter_map(Value::as_str).map(str::to_owned);
        Some(argv.collect::<Vec<_>>()).filter(|a| !a.is_empty())
    };
    let agent = match named_agent(profile, agent)?.or_else(recorded) {
        Some(agent) => agent,
        None if past.is_some() => {
            let error = "no agent recorded for it: give one after -- or with --profile";
            return Err(error::Error::new(Code::agent_not_started, error));
        }
        None => {
            let error =
                format!("brnr has no transcript of {id}: name its agent (--profile or -- <agent>)");
            return Err(error::Error::new(Code::agent_not_started, error));
        }
    };
    // Taken last: a session that is open is refused above, naming its process.
    let held = lock::take(&id).map_err(|e| match e {
        lock::Error::Held(pid) => {
            error::Error::new(Code::session_active, ownership::taken(&id, pid))
        }
        lock::Error::Io(why) => format!("{id} can't be locked: {why}").into(),
    })?;
    let (mut asked, caps) = Asked::start(&agent)?;
    if caps.session_capabilities.delete.is_none() {
        let error = "the agent can't delete sessions (no sessionCapabilities.delete)";
        return Err(error::Error::new(Code::capability_missing, error));
    }
    let agent_pid = asked.pid();
    let answer = asked.ask("session/delete", json!({ "sessionId": id })).map(drop);
    drop(asked);
    // The agent doesn't have it: nothing of it is left there to delete.
    let rpc = answer.as_ref().err().and_then(|e| e.rpc_error.as_ref());
    let gone = rpc.and_then(schema::read::<schema::Error>).map(|e| e.code)
        == Some(ErrorCode::ResourceNotFound);
    if let (Err(error), false) = (&answer, purge) {
        return Err(error.clone());
    }
    let (recorded, purged, failed) = if purge {
        let (purged, failed) = log::purge(&id, false);
        if let Err(error) = &answer {
            if purged.is_empty() && failed.is_empty() {
                return Err(error.clone().map(|e| format!("{e}; brnr has no transcript of {id}")));
            }
            // What --purge did, said only as far as it went: what wasn't
            // deleted follows, `not deleted`.
            let transcript = match (purged.is_empty(), failed.is_empty()) {
                (false, true) => "deleted brnr's transcript of it",
                (false, false) => "deleted part of brnr's transcript of it",
                (true, _) => "brnr's transcript of it isn't deleted",
            };
            match (gone, purged.is_empty()) {
                (true, _) => errln!("brnr: the agent doesn't have {id} ({error}); {transcript}"),
                (false, false) => errln!(
                    "brnr: the agent didn't delete {id} ({error}): it may still have it; \
                     {transcript} all the same"
                ),
                (false, true) => errln!(
                    "brnr: the agent didn't delete {id} ({error}): it may still have it; \
                     {transcript} either"
                ),
            }
        }
        (Vec::new(), purged, failed)
    } else {
        let event = json!({
            "event": "session_deleted",
            "session": id,
            "by": "control",
            "ts": log::rfc3339(SystemTime::now()),
            "host_id": null,
            "pid": null,
        });
        let (recorded, failed, made) = log::record_in_transcripts(&id, agent_pid, &event);
        // As the host log's `path_made_private` would (ADR 9.3): the command has none.
        for (path, mode) in made {
            errln!("brnr: path_made_private: {} (its mode was {mode:o})", path.display());
        }
        (recorded, Vec::new(), failed)
    };
    drop(held);
    let paths = |files: &[PathBuf]| -> Vec<String> {
        files.iter().map(|p| p.to_string_lossy().into_owned()).collect()
    };
    if json_out {
        print_json(&json!({
            "session": id,
            "deleted": answer.is_ok(),
            "error": answer.as_ref().err().map(error::Error::json),
            "recorded_in": paths(&recorded),
            "purged": paths(&purged),
            "failed": failed,
        }))?;
    } else if !purge {
        match recorded.is_empty() && failed.is_empty() {
            true => outln!("deleted {id}; brnr has no transcript of it"),
            false => outln!("deleted {id}; brnr's transcript of it stays (brnr event log {id})"),
        }
    } else {
        let what = if answer.is_ok() { format!("deleted {id}, and") } else { "deleted".to_owned() };
        match (purged.is_empty(), failed.is_empty(), answer.is_ok()) {
            (true, true, _) => outln!("deleted {id}; brnr had no transcript of it"),
            (true, false, true) => outln!("deleted {id}, but not brnr's transcript of it"),
            // Neither: stderr says why.
            (true, false, false) => {}
            (false, ..) => outln!("{what} brnr's transcript of {id}:"),
        }
        for path in paths(&purged) {
            outln!("  {path}");
        }
    }
    for why in &failed {
        match purge {
            true => errln!("brnr: not deleted: {why}"),
            false => errln!("brnr: session_deleted isn't recorded: {why}"),
        }
    }
    match failed.is_empty() && (answer.is_ok() || gone) {
        true => Ok(ExitCode::SUCCESS),
        false => Ok(ExitCode::FAILURE),
    }
}
