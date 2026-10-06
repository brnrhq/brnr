//! The agent's settings and sessions: `mode`, `config`, `model`,
//! `commands`, `sessions`, `fork` and `close`.

use std::process::ExitCode;
use std::time::Duration;

use serde_json::{Value, json};

use super::{Host, USAGE, discover, inactive_sessions, print_table, request_timeout, resolve};

/// The agent may take a while to switch model or fork a session.
const AGENT_TIMEOUT: Duration = Duration::from_secs(120);

/// `<target> [--session <id>]` and the rest of the arguments.
fn target_args(args: &[String]) -> Result<(String, Option<String>, Vec<String>), String> {
    let mut target = None;
    let mut session = None;
    let mut rest = Vec::new();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--session" => session = Some(it.next().ok_or("--session needs an id")?.clone()),
            flag if flag.starts_with("--") => return Err(format!("unknown option: {flag}")),
            _ if target.is_none() => target = Some(arg.clone()),
            _ => rest.push(arg.clone()),
        }
    }
    Ok((target.ok_or(USAGE)?, session, rest))
}

/// The host and the status of the session meant.
fn session_status(target: &str, session: Option<String>) -> Result<(Host, Value), String> {
    let hosts = discover();
    let (host, matched) = resolve(&hosts, target)?;
    let wanted = session.or(matched);
    let status = host.status.clone().ok_or("host is not answering")?;
    let sessions = status["sessions"].as_array().cloned().unwrap_or_default();
    let s = match (&wanted, &sessions[..]) {
        (Some(id), _) => sessions
            .iter()
            .find(|s| s["session_id"].as_str().is_some_and(|s| s.starts_with(id.as_str())))
            .cloned()
            .ok_or(format!("no session {id}"))?,
        (None, [one]) => one.clone(),
        (None, []) => return Err("no ACP session yet".into()),
        (None, _) => return Err("several sessions; pick one with --session".into()),
    };
    let host = Host { meta: host.meta.clone(), status: host.status.clone() };
    Ok((host, s))
}

fn agent_call(host: &Host, req: &Value) -> Result<Value, String> {
    let response = request_timeout(host, req, AGENT_TIMEOUT)
        .map_err(|e| format!("host {}: {e}", host.id()))?;
    if response["ok"].as_bool() != Some(true) {
        return Err(response["error"].as_str().unwrap_or("request failed").to_owned());
    }
    Ok(response)
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or("?")
}

pub(super) fn mode(args: &[String]) -> Result<ExitCode, String> {
    let (target, session, rest) = target_args(args)?;
    let (host, status) = session_status(&target, session)?;
    let id = s(&status["session_id"]).to_owned();
    match &rest[..] {
        [] => {
            let current = status["mode"].as_str();
            let modes = status["modes"].as_array().cloned().unwrap_or_default();
            if modes.is_empty() {
                // An agent with modes only as a config option.
                let option =
                    status["config"].as_array().into_iter().flatten().find(|o| o["id"] == "mode");
                return match option {
                    Some(option) => {
                        print_choices(option);
                        Ok(ExitCode::SUCCESS)
                    }
                    None => Err("the agent offers no modes".into()),
                };
            }
            for m in modes {
                let mark = if m["id"].as_str() == current { "*" } else { " " };
                let about = m["description"].as_str().map(|d| format!("  {d}")).unwrap_or_default();
                println!("{mark} {}{about}", s(&m["id"]));
            }
        }
        [mode] => {
            agent_call(&host, &json!({ "cmd": "set_mode", "session": id, "mode": mode }))?;
            println!("mode {mode} (session {id})");
        }
        _ => return Err(USAGE.to_owned()),
    }
    Ok(ExitCode::SUCCESS)
}

/// A select option's values, the current one starred.
fn print_choices(option: &Value) {
    let current = &option["currentValue"];
    for choice in option["options"].as_array().into_iter().flatten() {
        let mark = if &choice["value"] == current { "*" } else { " " };
        println!("{mark} {}  {}", s(&choice["value"]), choice["name"].as_str().unwrap_or(""));
    }
}

pub(super) fn config(args: &[String]) -> Result<ExitCode, String> {
    let (target, session, rest) = target_args(args)?;
    let (host, status) = session_status(&target, session)?;
    let id = s(&status["session_id"]).to_owned();
    if rest.is_empty() {
        let options = status["config"].as_array().cloned().unwrap_or_default();
        if options.is_empty() {
            return Err("the agent has no config options".into());
        }
        let mut rows = vec![["OPTION", "VALUE", "CHOICES", "NAME"].map(String::from)];
        for o in &options {
            let choices: Vec<&str> = o["options"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|c| c["value"].as_str())
                .collect();
            let value = match &o["currentValue"] {
                Value::String(v) => v.clone(),
                other => other.to_string(),
            };
            rows.push([
                s(&o["id"]).to_owned(),
                value,
                choices.join(" "),
                o["name"].as_str().unwrap_or("").to_owned(),
            ]);
        }
        print_table(rows);
        return Ok(ExitCode::SUCCESS);
    }
    for pair in &rest {
        let (option, value) =
            pair.split_once('=').ok_or(format!("<option>=<value>, not {pair}"))?;
        agent_call(
            &host,
            &json!({ "cmd": "set_config", "session": id, "option": option, "value": value }),
        )?;
        println!("{option}={value} (session {id})");
    }
    Ok(ExitCode::SUCCESS)
}

pub(super) fn model(args: &[String]) -> Result<ExitCode, String> {
    let (target, session, rest) = target_args(args)?;
    let (host, status) = session_status(&target, session)?;
    let id = s(&status["session_id"]).to_owned();
    match &rest[..] {
        [] => {
            let option = status["config"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|o| o["id"] == "model" || o["category"] == "model");
            if let Some(option) = option {
                print_choices(option);
            } else if let Some(models) = status["models"].as_array() {
                for m in models {
                    let mark = if m["modelId"] == status["model"] { "*" } else { " " };
                    println!("{mark} {}  {}", s(&m["modelId"]), m["name"].as_str().unwrap_or(""));
                }
            } else {
                return Err("the agent offers no model choice".into());
            }
        }
        [model] => {
            agent_call(&host, &json!({ "cmd": "set_model", "session": id, "model": model }))?;
            println!("model {model} (session {id})");
        }
        _ => return Err(USAGE.to_owned()),
    }
    Ok(ExitCode::SUCCESS)
}

pub(super) fn commands(args: &[String]) -> Result<ExitCode, String> {
    let (target, session, rest) = target_args(args)?;
    if !rest.is_empty() {
        return Err(USAGE.to_owned());
    }
    let (_, status) = session_status(&target, session)?;
    let commands = status["commands"].as_array().cloned().unwrap_or_default();
    if commands.is_empty() {
        println!("the agent has announced no commands");
    }
    for c in commands {
        let hint = c["input"]["hint"].as_str().map(|h| format!(" <{h}>")).unwrap_or_default();
        println!("/{}{hint}  {}", s(&c["name"]), c["description"].as_str().unwrap_or(""));
    }
    Ok(ExitCode::SUCCESS)
}

pub(super) fn sessions(args: &[String]) -> Result<ExitCode, String> {
    let [target] = args else { return Err(USAGE.to_owned()) };
    let hosts = discover();
    let (host, _) = resolve(&hosts, target)?;
    // What brnr knows of each: running, inactive (a transcript), or nothing.
    let running: Vec<(&str, &str)> = hosts
        .iter()
        .flat_map(|h| h.sessions().iter().map(move |x| (s(&x["session_id"]), h.id())))
        .collect();
    let past = inactive_sessions(&hosts);
    let brnr = |id: &str| match running.iter().find(|(r, _)| *r == id) {
        Some((_, host)) => format!("running ({host})"),
        None if past.iter().any(|p| p["session_id"] == id) => "inactive".to_owned(),
        None => "-".to_owned(),
    };
    let mut rows = vec![["SESSION", "UPDATED", "BRNR", "TITLE", "CWD"].map(String::from)];
    let mut cursor: Option<String> = None;
    loop {
        let req = json!({ "cmd": "sessions", "cursor": cursor });
        let response = agent_call(host, &req)?;
        for s in response["sessions"].as_array().into_iter().flatten() {
            let updated = s["updatedAt"].as_str().unwrap_or("");
            let updated = updated.get(..19).unwrap_or(updated).replace('T', " ");
            let id = s["sessionId"].as_str().unwrap_or("?");
            rows.push([
                id.to_owned(),
                updated,
                brnr(id),
                s["title"].as_str().unwrap_or("-").to_owned(),
                s["cwd"].as_str().unwrap_or("?").to_owned(),
            ]);
        }
        match response["next_cursor"].as_str() {
            Some(next) => cursor = Some(next.to_owned()),
            None => break,
        }
    }
    if rows.len() == 1 {
        println!("the agent knows no sessions here");
    } else {
        print_table(rows);
    }
    Ok(ExitCode::SUCCESS)
}

pub(super) fn fork(args: &[String]) -> Result<ExitCode, String> {
    let (target, session, rest) = target_args(args)?;
    if !rest.is_empty() {
        return Err(USAGE.to_owned());
    }
    let (host, status) = session_status(&target, session)?;
    let from = s(&status["session_id"]).to_owned();
    let response = agent_call(&host, &json!({ "cmd": "fork", "session": from }))?;
    println!("forked {from} into {} (host {})", s(&response["session"]), host.id());
    Ok(ExitCode::SUCCESS)
}

pub(super) fn close(args: &[String]) -> Result<ExitCode, String> {
    let (target, session, rest) = target_args(args)?;
    if !rest.is_empty() {
        return Err(USAGE.to_owned());
    }
    let (host, status) = session_status(&target, session)?;
    let id = s(&status["session_id"]).to_owned();
    agent_call(&host, &json!({ "cmd": "close", "session": id }))?;
    println!("closed {id}");
    Ok(ExitCode::SUCCESS)
}
