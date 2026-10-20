//! A session's settings and the sessions themselves: `config get`, `config
//! set`, `prompt commands`, `session fork` and `session close`.

use std::collections::BTreeMap;
use std::process::ExitCode;

use serde_json::{Value, json};

use super::client::Host;
use super::{Args, discover, print_json, print_table, running_session, text};

/// The running session `arg` names: its process and its status.
fn session_status(arg: &str) -> Result<(Host, Value), String> {
    let hosts = discover()?;
    let (host, id) = running_session(&hosts, arg)?;
    let status = host.session(&id).cloned().ok_or("the process is not answering")?;
    Ok((host.clone(), status))
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or("?")
}

/// What `config get` narrows to: an option found by category, as `config
/// set` finds it, or by id.
enum Wanted {
    Category(&'static str),
    Id(String),
}

/// `config get`: every config option with its value and choices (each with
/// its name and description), and the v1 modes where the agent has them
/// (`option` null), as the agent gave them; `--mode`, `--model`,
/// `--thought-level` and `--option <o>` narrow it to those, found as
/// `config set` finds them (ADR 8.1).
pub(super) fn config_get(a: &Args) -> Result<ExitCode, String> {
    let json_out = a.json;
    // In the order given, `--option`'s ids among them.
    let mut ids = a.values("--option").into_iter();
    let wanted: Vec<Wanted> = a
        .given()
        .filter_map(|flag| match flag {
            "--mode" => Some(Wanted::Category("mode")),
            "--model" => Some(Wanted::Category("model")),
            "--thought-level" => Some(Wanted::Category("thought_level")),
            _ => ids.next().map(Wanted::Id),
        })
        .collect();
    let (_, status) = session_status(&a.positional[0])?;
    let id = s(&status["session"]).to_owned();
    let options = status["config"].as_array().map_or(&[][..], Vec::as_slice);
    let mut rows: Vec<Value> = options
        .iter()
        .map(|o| {
            json!({
                "option": o["id"],
                "category": o["category"],
                "value": o["currentValue"],
                "choices": choices(o),
                "name": o["name"],
                "description": o["description"],
            })
        })
        .collect();
    // After the options, so that a mode option is the mode, as for `set`.
    if let Some(modes) = status["modes"].as_object() {
        let available = modes.get("availableModes").and_then(Value::as_array);
        let choices: Vec<Value> = available
            .into_iter()
            .flatten()
            .map(
                |m| json!({ "value": m["id"], "name": m["name"], "description": m["description"] }),
            )
            .collect();
        rows.push(json!({
            "option": null,
            "category": "mode",
            "value": modes.get("currentModeId"),
            "choices": choices,
            "name": null,
            "description": null,
        }));
    }
    if !wanted.is_empty() {
        let mut keep = Vec::new();
        for want in &wanted {
            let found = match want {
                Wanted::Category(c) => rows.iter().position(|r| r["category"] == *c).ok_or(
                    match *c {
                        "mode" => "the agent offers no modes",
                        "model" => "the agent offers no model choice",
                        _ => "the agent offers no thought level",
                    }
                    .to_owned(),
                ),
                Wanted::Id(o) => rows
                    .iter()
                    .position(|r| r["option"] == o.as_str())
                    .ok_or(format!("the agent has no option {o}")),
            };
            keep.push(found?);
        }
        keep.sort_unstable();
        keep.dedup();
        rows = keep.into_iter().map(|n| rows[n].clone()).collect();
    }
    if json_out {
        print_json(&json!({ "session": id, "options": rows }))?;
        return Ok(ExitCode::SUCCESS);
    }
    if rows.is_empty() {
        outln!("the agent has no config options");
        return Ok(ExitCode::SUCCESS);
    }
    let shown = |v: &Value| match v {
        Value::String(v) => v.clone(),
        Value::Null => "-".to_owned(),
        other => other.to_string(),
    };
    // An option's row, then a row for each of its choices, the current one
    // marked.
    let text = |v: &Value| v.as_str().unwrap_or("").to_owned();
    let mut table = vec![["OPTION", "CATEGORY", "VALUE", "NAME", "DESCRIPTION"].map(String::from)];
    for r in &rows {
        table.push([
            shown(&r["option"]),
            shown(&r["category"]),
            shown(&r["value"]),
            text(&r["name"]),
            text(&r["description"]),
        ]);
        for c in r["choices"].as_array().into_iter().flatten() {
            let mark = if c["value"] == r["value"] { "*" } else { " " };
            let choice = format!("{mark} {}", shown(&c["value"]));
            table.push([
                String::new(),
                String::new(),
                choice,
                text(&c["name"]),
                text(&c["description"]),
            ]);
        }
    }
    print_table(table);
    Ok(ExitCode::SUCCESS)
}

/// A select option's choices, its groups' too: each value, with its name
/// and description.
fn choices(option: &Value) -> Vec<Value> {
    let mut listed = Vec::new();
    for c in option["options"].as_array().into_iter().flatten() {
        match c["options"].as_array() {
            Some(group) => listed.extend(group),
            None => listed.push(c),
        }
    }
    let choice = |c: &Value| json!({ "value": c["value"], "name": c["name"], "description": c["description"] });
    listed.into_iter().map(choice).collect()
}

/// `config set`: `--mode`, `--model` and `--thought-level` find their option
/// by category, `--option <o>=<v>` by id, resolved by the process as a
/// start's settings are (ADR 6.1, ADR 8.1) and sent one at a time.
pub(super) fn config_set(a: &Args) -> Result<ExitCode, String> {
    let json_out = a.json;
    let options = options("--option", &a.values("--option"))?;
    let (mode, model, thought_level) =
        (a.value("--mode"), a.value("--model"), a.value("--thought-level"));
    let (host, status) = session_status(&a.positional[0])?;
    let id = s(&status["session"]).to_owned();
    let req = json!({
        "cmd": "set_config",
        "session": id,
        "mode": mode,
        "model": model,
        "thought_level": thought_level,
        "options": options,
    });
    let response = host.call_agent(&req)?;
    if json_out {
        print_json(&json!({ "session": id, "set": response["set"] }))?;
        return Ok(ExitCode::SUCCESS);
    }
    for set in response["set"].as_array().into_iter().flatten() {
        // A v1 mode has no option.
        outln!("{}={}", set["option"].as_str().unwrap_or("mode"), s(&set["value"]));
    }
    Ok(ExitCode::SUCCESS)
}

/// `<option>=<value>` pairs, given with `flag`, by option; the same option
/// twice with two values fails (ADR 6.1).
pub(super) fn options(flag: &str, pairs: &[String]) -> Result<BTreeMap<String, String>, String> {
    let mut options = BTreeMap::new();
    for pair in pairs {
        let (option, value) =
            pair.split_once('=').ok_or(format!("{flag} takes <option>=<value>, not {pair}"))?;
        if let Some(was) = options.insert(option.to_owned(), value.to_owned())
            && was != value
        {
            return Err(format!("{flag} {option}={was} and {flag} {option}={value} disagree"));
        }
    }
    Ok(options)
}

pub(super) fn commands(a: &Args) -> Result<ExitCode, String> {
    let json_out = a.json;
    let (_, status) = session_status(&a.positional[0])?;
    let commands: Vec<Value> = status["commands"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|c| json!({ "command": c["name"], "hint": c["input"]["hint"], "description": c["description"] }))
        .collect();
    if json_out {
        print_json(&json!({ "session": status["session"], "commands": commands }))?;
        return Ok(ExitCode::SUCCESS);
    }
    if commands.is_empty() {
        outln!("the agent has announced no commands");
    }
    for c in commands {
        let hint = c["hint"].as_str().map(|h| format!(" <{h}>")).unwrap_or_default();
        outln!("/{}{hint}  {}", s(&c["command"]), c["description"].as_str().unwrap_or(""));
    }
    Ok(ExitCode::SUCCESS)
}

// ---- forking, closing ---------------------------------------------------

pub(super) fn fork(a: &Args) -> Result<ExitCode, String> {
    let (arg, json_out) = (&a.positional[0], a.json);
    let hosts = discover()?;
    let (host, from) = running_session(&hosts, arg)?;
    let response = host.call_agent(&json!({ "cmd": "fork", "session": from }))?;
    let session = text(&response["session"]);
    if json_out {
        print_json(&json!({ "session": session, "from": from }))?;
    } else {
        outln!("forked {from} into {session}");
    }
    Ok(ExitCode::SUCCESS)
}

pub(super) fn close(a: &Args) -> Result<ExitCode, String> {
    let (arg, json_out) = (&a.positional[0], a.json);
    let hosts = discover()?;
    let (host, id) = running_session(&hosts, arg)?;
    let response = host.call_agent(&json!({ "cmd": "close", "session": id }))?;
    if json_out {
        print_json(&response)?;
        return Ok(ExitCode::SUCCESS);
    }
    outln!("closed {id}");
    Ok(ExitCode::SUCCESS)
}

/// `session resume --take-over`: `owner`, which holds `session`'s lock, closes
/// it as `brnr session close` does, cancelling a running turn, and so lets go
/// of it (ADR 3.9) for process `to` to resume. An editor's process does only if
/// its profile enables the experimental `close`, and tells the editor where the
/// session went (ADR 7.1).
pub(super) fn take_over(owner: &Host, session: &str, to: u32) -> Result<(), String> {
    let pid = owner.id();
    let running = format!("{session} is running in process {pid}");
    let req = json!({ "cmd": "close", "session": session, "take_over": to });
    owner.call_agent(&req).map_err(|e| {
        if owner.info()["owner"] == "editor" {
            format!("{running}, an editor's: {e}")
        } else {
            format!("{running}, which didn't close it: {e}")
        }
    })?;
    errln!("brnr: closed {session} in process {pid}");
    Ok(())
}
