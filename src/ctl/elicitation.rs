//! `brnr elicitation` (ADR 4.5 in docs/adr): the agent's requests for input
//! (`elicitation/create`), a form or a URL, listed, shown and answered with
//! ACP's own actions, `accept`, `decline` and `cancel`.
//!
//! An elicitation is named as a permission request is, by its session and
//! its handle (`e<n>`); one scoped to a request outside any session (a
//! login) has none, and is named by its process, `--pid <pid>`. `accept`
//! takes a form's values as `--field name=value` (repeated for a list) or
//! as one JSON object on stdin (`-`); the process checks them against the
//! form's schema before anything is sent, so the CLI and the bridges get
//! the same check. A URL is shown, never opened: the agent's text is not
//! brnr's to run (P8).

use serde_json::{Map, Value, json};

use crate::{json, render};

use super::client::{Host, Pending};
use super::{Args, discover, print_json, print_table, process, read_stdin, running_session, text};

/// Where an elicitation is: a session in a process, or a process alone for
/// one outside any session.
struct Target<'a> {
    host: &'a Host,
    session: Option<String>,
    /// How the commands name it: `<session>`, or `--pid <pid>`.
    arg: String,
}

/// Where `a`'s `<session>` or `--pid <pid>` is, and its `<request>`.
fn locate<'a>(hosts: &'a [Host], a: &Args) -> Result<(Target<'a>, String), String> {
    let target = match a.value("--pid") {
        None => {
            let session = &a.positional[0];
            let (host, id) = running_session(hosts, session)?;
            Target { host, session: Some(id), arg: session.clone() }
        }
        Some(pid) => {
            let host = process(hosts, pid)?;
            Target { host, session: None, arg: format!("--pid {pid}") }
        }
    };
    let request = a.positional.last().cloned().unwrap_or_default();
    Ok((target, request))
}

/// The elicitation `request` waiting at `target`, in `pending`.
fn find<'a>(pending: &'a Pending, target: &Target, request: &str) -> Result<&'a Value, String> {
    let found = pending.elicitation(target.session.as_deref(), request);
    found.ok_or_else(|| match &target.session {
        Some(_) => format!("no pending elicitation {request} in {}", target.arg),
        None => format!(
            "no pending elicitation {request} outside a session in process {}",
            target.host.id()
        ),
    })
}

/// `elicitation requests [<session>]`: those waiting, in every process that
/// answers (saying which don't), or in one session.
pub(super) fn requests(a: &Args) -> Result<(), String> {
    let (arg, json_out) = (a.positional.first(), a.json);
    let hosts = discover()?;
    let (chosen, only): (Vec<&Host>, Option<String>) = match arg {
        // As `permission requests` (ADR 4.5): one that doesn't answer is
        // passed over, and said to be.
        None => {
            for host in hosts.iter().filter(|h| h.status.is_none()) {
                eprintln!(
                    "brnr: process {} is not answering; its elicitations aren't listed",
                    host.id()
                );
            }
            (hosts.iter().filter(|h| h.status.is_some()).collect(), None)
        }
        Some(arg) => {
            let (host, id) = running_session(&hosts, arg)?;
            (vec![host], Some(id))
        }
    };
    // Read, listed and let go of on a stack that takes what the agent sent
    // (ADR 2.5), also when a process asked after it fails, as `permission
    // requests` does.
    let mut rows = Vec::new();
    for host in chosen {
        let pid = host.id().parse::<u64>().ok();
        let more = host.pending(|pending| {
            let row = |e: &Value| {
                json!({
                    "session": e["session"],
                    "pid": pid,
                    "request": e["request"],
                    "owner": e["owner"],
                    "mode": e["mode"],
                    "scope": e["scope"],
                    "message": e["message"],
                })
            };
            pending.elicitations(only.as_deref()).map(row).collect::<Vec<_>>()
        });
        match more {
            Ok(more) => rows.extend(more),
            Err(e) => {
                json::free(Value::Array(rows));
                return Err(e);
            }
        }
    }
    json::holding(Value::Array(rows), |rows| list(&rows, json_out))?
}

/// The elicitations' `rows`, as a table or `--json`.
fn list(rows: &Value, json_out: bool) -> Result<(), String> {
    if json_out {
        return print_json(rows);
    }
    let rows = rows.as_array().map_or(&[][..], Vec::as_slice);
    if rows.is_empty() {
        outln!("nothing waiting");
        return Ok(());
    }
    let mut table = vec![["SESSION", "REQUEST", "OWNER", "MODE", "MESSAGE"].map(String::from)];
    for r in rows {
        // One outside a session is named by its process, as the commands
        // take it.
        let session = match r["session"].as_str() {
            Some(session) => session.to_owned(),
            None => format!("--pid {}", r["pid"]),
        };
        let mode = text(&r["mode"]);
        table.push([session, text(&r["request"]), text(&r["owner"]), mode, text(&r["message"])]);
    }
    print_table(table);
    Ok(())
}

/// `elicitation show`: one in full, so it can be answered: the message, the
/// mode and scope, a form's fields, a URL as the agent sent it.
pub(super) fn show(a: &Args) -> Result<(), String> {
    let json_out = a.json;
    let hosts = discover()?;
    let (target, request) = locate(&hosts, a)?;
    // As deep as the agent made it, with the rest of what waits there:
    // shown, and let go of, on a stack that takes it (ADR 2.5).
    target.host.pending(|pending| {
        let e = find(pending, &target, &request)?;
        if json_out {
            return print_json(e);
        }
        describe(e, &target, &request);
        Ok(())
    })?
}

/// Prints elicitation `e`, `request` at `target`, in full.
fn describe(e: &Value, target: &Target, request: &str) {
    let editor = e["owner"] == "editor";
    let by = if editor { ", waiting in the editor" } else { "" };
    let arg = &target.arg;
    match &target.session {
        Some(session) => outln!("{request}, session {session}{by}"),
        None => outln!("{request}, process {}, outside any session{by}", target.host.id()),
    }
    outln!("{}", text(&e["message"]).trim_end());
    let scope = match e["scope"].as_str() {
        Some("tool_call") => format!("tool call {}", text(&e["tool_call"])),
        Some("request") => format!("request {}", e["scope_request"]),
        scope => scope.unwrap_or("?").to_owned(),
    };
    let mode = text(&e["mode"]);
    outln!("mode: {mode}, for the {scope}");
    let accept = match mode.as_str() {
        "form" => {
            out!("fields:\n{}", render::form(&e["schema"]));
            // The required fields by name, then any of the others.
            let required = e["schema"]["required"].as_array().into_iter().flatten();
            let mut fields: Vec<String> =
                required.map(|k| format!(" --field {}=<value>", text(k))).collect();
            if e["schema"]["properties"].as_object().is_some_and(|p| p.len() > fields.len()) {
                fields.push(" [--field <k>=<v>]...".to_owned());
            }
            Some(format!("brnr elicitation accept {arg} {request}{}", fields.concat()))
        }
        "url" => {
            outln!("url: {}", text(&e["url"]));
            outln!("brnr doesn't open it: open it yourself, then accept");
            Some(format!("brnr elicitation accept {arg} {request}"))
        }
        // A mode brnr doesn't know: shown, never accepted.
        _ => {
            outln!("brnr doesn't know this mode; it can be declined or cancelled");
            None
        }
    };
    if let Some(secs) = e["timeout_seconds"].as_u64() {
        // As the timeout answers (ADR 4.5): `cancel`, never `decline`.
        outln!("cancelled in {secs}s if nobody answers");
    }
    let rest = format!("brnr elicitation decline {arg} {request} or cancel {arg} {request}");
    let here = match accept {
        Some(accept) => format!("{accept}, {rest}"),
        None => rest,
    };
    match (editor, e["why_not"].as_str()) {
        (false, _) => outln!("answer: {here}"),
        (true, None) => outln!("answer: in the editor, or {here} (experimental)"),
        (true, Some(why)) => outln!("answer it in the editor ({why})"),
    }
}

/// `elicitation accept`, `decline` and `cancel` (`action`): the action sent,
/// an accept's fields checked against the form by the process first.
pub(super) fn answer(a: &Args, action: &str) -> Result<(), String> {
    let accept = action == "accept";
    let (json_out, stdin) = (a.json, accept && a.has("-"));
    let mut fields: Map<String, Value> = Map::new();
    for field in if accept { a.values("--field") } else { Vec::new() } {
        let (name, value) =
            field.split_once('=').ok_or(format!("--field {field}: not <name>=<value>"))?;
        // A field given again is a list's next value.
        let value = Value::String(value.to_owned());
        match fields.get_mut(name) {
            Some(Value::Array(list)) => list.push(value),
            Some(first) => *first = json!([first.take(), value]),
            None => {
                fields.insert(name.to_owned(), value);
            }
        }
    }
    let hosts = discover()?;
    let (target, request) = locate(&hosts, a)?;
    let content = match (stdin, fields.is_empty()) {
        (true, false) => {
            return Err("give the fields with --field or on stdin (-), not both".into());
        }
        (true, true) => {
            let text = read_stdin()?;
            let value: Value =
                serde_json::from_str(&text).map_err(|e| format!("stdin isn't JSON: {e}"))?;
            if !value.is_object() {
                return Err("stdin must be one JSON object of the form's fields".into());
            }
            value
        }
        (false, true) if action != "accept" => Value::Null,
        (false, _) => Value::Object(fields),
    };
    let req = json!({
        "cmd": "elicitation",
        "session": target.session,
        "request": request,
        "action": action,
        "content": content,
    });
    let response = target.host.call(&req)?;
    let result = &response["result"];
    if json_out {
        return print_json(&json!({
            "session": target.session,
            "request": request,
            "action": result["action"],
            "content": result["content"],
        }));
    }
    let done = match action {
        "accept" => "accepted",
        "decline" => "declined",
        _ => "cancelled",
    };
    outln!("{request} {done}");
    Ok(())
}
