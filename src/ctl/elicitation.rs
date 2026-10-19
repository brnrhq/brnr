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

use crate::render;

use super::{
    Host, USAGE, call, discover, print_json, print_table, process, read_stdin, request,
    running_session, text,
};

/// Where an elicitation is: a session in a process, or a process alone for
/// one outside any session.
struct Target<'a> {
    host: &'a Host,
    session: Option<String>,
    /// How the commands name it: `<session>`, or `--pid <pid>`.
    arg: String,
}

/// `<session>` or `--pid <pid>`, then `<request>`, and the flags `flag`
/// takes (its value, if it has one); the rest of `args` are an error.
fn parse<'a>(
    hosts: &'a [Host],
    args: &[String],
    mut flag: impl FnMut(&str, &mut dyn Iterator<Item = &String>) -> Result<bool, String>,
) -> Result<(Target<'a>, String), String> {
    let (mut pid, mut positional) = (None, Vec::new());
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--pid" => pid = Some(it.next().ok_or("--pid needs a pid")?.clone()),
            a if flag(a, &mut it)? => {}
            a if a.starts_with("--") => return Err(format!("unknown option: {a}")),
            _ => positional.push(arg.clone()),
        }
    }
    let target = match (pid, &positional[..]) {
        (None, [session, _]) => {
            let (host, id) = running_session(hosts, session)?;
            Target { host, session: Some(id), arg: session.clone() }
        }
        (Some(pid), [_]) => {
            let host = process(hosts, &pid)?;
            Target { host, session: None, arg: format!("--pid {pid}") }
        }
        _ => return Err(USAGE.to_owned()),
    };
    let request = positional.pop().unwrap_or_default();
    Ok((target, request))
}

/// The elicitation `request` waiting at `target`, in full.
fn find(target: &Target, request: &str) -> Result<Value, String> {
    let response = call(target.host, &json!({ "cmd": "pending" }))?;
    let found =
        response["elicitations"].as_array().into_iter().flatten().find(|e| {
            e["request"] == request && e["session"].as_str() == target.session.as_deref()
        });
    found.cloned().ok_or_else(|| match &target.session {
        Some(_) => format!("no pending elicitation {request} in {}", target.arg),
        None => format!(
            "no pending elicitation {request} outside a session in process {}",
            target.host.id()
        ),
    })
}

/// `elicitation requests [<session>]`: those waiting, in every process that
/// answers (saying which don't), or in one session.
pub(super) fn requests(args: &[String]) -> Result<(), String> {
    let mut json_out = false;
    let mut arg = None;
    for a in args {
        match a.as_str() {
            "--json" => json_out = true,
            flag if flag.starts_with("--") => return Err(format!("unknown option: {flag}")),
            _ if arg.is_none() => arg = Some(a.clone()),
            _ => return Err(USAGE.to_owned()),
        }
    }
    let hosts = discover()?;
    let (chosen, only): (Vec<&Host>, Option<String>) = match &arg {
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
    let mut rows = Vec::new();
    for host in chosen {
        let response = request(host, &json!({ "cmd": "pending" }))
            .map_err(|e| format!("process {}: {e}", host.id()))?;
        for e in response["elicitations"].as_array().into_iter().flatten() {
            if only.as_deref().is_some_and(|id| e["session"] != id) {
                continue;
            }
            rows.push(json!({
                "session": e["session"],
                "pid": host.id().parse::<u64>().ok(),
                "request": e["request"],
                "owner": e["owner"],
                "mode": e["mode"],
                "scope": e["scope"],
                "message": e["message"],
            }));
        }
    }
    if json_out {
        return print_json(&json!(rows));
    }
    if rows.is_empty() {
        outln!("nothing waiting");
        return Ok(());
    }
    let mut table = vec![["SESSION", "REQUEST", "OWNER", "MODE", "MESSAGE"].map(String::from)];
    for r in &rows {
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
pub(super) fn show(args: &[String]) -> Result<(), String> {
    let mut json_out = false;
    let hosts = discover()?;
    let (target, request) = parse(&hosts, args, |a, _| {
        let json = a == "--json";
        json_out |= json;
        Ok(json)
    })?;
    let e = find(&target, &request)?;
    if json_out {
        return print_json(&e);
    }
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
    Ok(())
}

/// `elicitation accept`, `decline` and `cancel` (`action`): the action sent,
/// an accept's fields checked against the form by the process first.
pub(super) fn answer(args: &[String], action: &str) -> Result<(), String> {
    let mut json_out = false;
    let mut stdin = false;
    let mut fields: Map<String, Value> = Map::new();
    let hosts = discover()?;
    let (target, request) = parse(&hosts, args, |a, it| match a {
        "--json" => {
            json_out = true;
            Ok(true)
        }
        "-" if action == "accept" => {
            stdin = true;
            Ok(true)
        }
        "--field" if action == "accept" => {
            let field = it.next().ok_or("--field needs <name>=<value>")?;
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
            Ok(true)
        }
        _ => Ok(false),
    })?;
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
    let response = call(target.host, &req)?;
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
