//! `brnr permission show <session> <request> [--json]`: one waiting approval in
//! full, so it can be judged before it is answered: the tool, its kind and
//! paths, the command or input, and the diff of an edit.

use std::process::ExitCode;

use serde_json::Value;

use crate::render;

use super::{USAGE, discover, print_json, running_session};

pub(super) fn show(args: &[String]) -> Result<ExitCode, String> {
    let mut positional = Vec::new();
    let mut json_out = false;
    for arg in args {
        match arg.as_str() {
            "--json" => json_out = true,
            flag if flag.starts_with("--") => return Err(format!("unknown option: {flag}")),
            _ => positional.push(arg),
        }
    }
    let [arg, request] = positional[..] else { return Err(USAGE.to_owned()) };
    let hosts = discover()?;
    let (host, session) = running_session(&hosts, arg)?;
    // As deep as the agent made it: shown, and let go of, on a stack that
    // takes it (ADR 2.5).
    host.pending(|pending| {
        let p = pending.approval(&session, request);
        let p = p.ok_or(format!("no pending request {request} in {arg}"))?;
        if json_out {
            return print_json(p);
        }
        describe(p, &session, arg, request);
        Ok(())
    })??;
    Ok(ExitCode::SUCCESS)
}

/// Prints permission request `p`, `request` of `session` (named `arg`), in
/// full.
fn describe(p: &Value, session: &str, arg: &str, request: &str) {
    let editor = p["owner"] == "editor";
    let by = if editor { ", waiting in the editor" } else { "" };
    outln!("{request}, session {session}{by}");
    out!("{}", render::tool_call(&p["tool_call"]));
    let options: Vec<String> = p["options"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|o| {
            format!(
                "{} ({})",
                o["optionId"].as_str().unwrap_or("?"),
                o["kind"].as_str().unwrap_or("?")
            )
        })
        .collect();
    outln!("options: {}", options.join(", "));
    if let Some(secs) = p["timeout_seconds"].as_u64() {
        // As the timeout answers (ADR 8.1): the one reject_once option, else
        // the turn is cancelled.
        let once = p["options"].as_array().into_iter().flatten();
        let once = once.filter(|o| o["kind"] == "reject_once").count();
        let how = if once == 1 { "rejected once" } else { "its turn cancelled" };
        outln!("{how} in {secs}s if nobody answers");
    }
    let here = format!(
        "brnr permission allow {arg} {request} or reject {arg} {request} (--always, --option <id>)"
    );
    match (editor, p["why_not"].as_str()) {
        (false, _) => outln!("answer: {here}"),
        (true, None) => outln!("answer: in the editor, or {here} (experimental)"),
        (true, Some(why)) => outln!("answer it in the editor ({why})"),
    }
}
