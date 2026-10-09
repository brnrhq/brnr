//! A session's settings and the sessions themselves: `config get`, `config
//! set`, `prompt commands`, `sessions`, `session fork` and `session close`.

use std::collections::BTreeMap;
use std::env;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::process::CommandExt;
use std::process::{Command, ExitCode, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use brnr::schema::{self, AgentCapabilities, ListSessionsResponse, SessionInfo};
use brnr::{config, json, lock, paths, spawn, sys};

use super::{
    Host, USAGE, discover, inactive_sessions, print_json, print_table, request_timeout,
    running_rows, running_session, text, when,
};

/// The agent may take a while to switch model or fork a session.
const AGENT_TIMEOUT: Duration = Duration::from_secs(120);

/// How long `brnr sessions` gives the agent to start and list.
const LIST_TIMEOUT: Duration = Duration::from_secs(60);

/// How long the agent `brnr sessions` started has to exit, after its stdin
/// closes and it gets SIGTERM, before its process group gets SIGKILL.
const STOP_WAIT: Duration = Duration::from_secs(5);

/// `<session>`, the other arguments, and whether `--json` was given.
fn session_args(args: &[String]) -> Result<(String, Vec<String>, bool), String> {
    let mut session = None;
    let mut rest = Vec::new();
    let mut json_out = false;
    for arg in args {
        match arg.as_str() {
            "--json" => json_out = true,
            flag if flag.starts_with("--") => return Err(format!("unknown option: {flag}")),
            _ if session.is_none() => session = Some(arg.clone()),
            _ => rest.push(arg.clone()),
        }
    }
    Ok((session.ok_or(USAGE)?, rest, json_out))
}

/// The running session `arg` names: its process and its status.
fn session_status(arg: &str) -> Result<(Host, Value), String> {
    let hosts = discover()?;
    let (host, id) = running_session(&hosts, arg)?;
    let status = host
        .sessions()
        .iter()
        .find(|s| s["session_id"] == id.as_str())
        .cloned()
        .ok_or("the process is not answering")?;
    Ok((Host { meta: host.meta.clone(), status: host.status.clone() }, status))
}

fn agent_call(host: &Host, req: &Value) -> Result<Value, String> {
    let response = request_timeout(host, req, AGENT_TIMEOUT)
        .map_err(|e| format!("process {}: {e}", host.id()))?;
    if response["ok"].as_bool() != Some(true) {
        return Err(response["error"].as_str().unwrap_or("request failed").to_owned());
    }
    Ok(response)
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
/// `config set` finds them (ADR 63).
pub(super) fn config_get(args: &[String]) -> Result<ExitCode, String> {
    let (mut session, mut json_out, mut wanted) = (None, false, Vec::new());
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--json" => json_out = true,
            "--mode" => wanted.push(Wanted::Category("mode")),
            "--model" => wanted.push(Wanted::Category("model")),
            "--thought-level" => wanted.push(Wanted::Category("thought_level")),
            "--option" => {
                wanted.push(Wanted::Id(it.next().ok_or("--option needs an option's id")?.clone()));
            }
            flag if flag.starts_with("--") => return Err(format!("unknown option: {flag}")),
            _ if session.is_none() => session = Some(arg.clone()),
            _ => return Err(USAGE.to_owned()),
        }
    }
    let (_, status) = session_status(&session.ok_or(USAGE)?)?;
    let id = s(&status["session_id"]).to_owned();
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
    // marked, as `brnr mode` and `brnr model` listed them.
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
/// start's settings are (ADR 58, ADR 63) and sent one at a time.
pub(super) fn config_set(args: &[String]) -> Result<ExitCode, String> {
    let (mut session, mut json_out, mut pairs) = (None, false, Vec::new());
    let mut by_category: [(&str, Option<String>); 3] =
        [("mode", None), ("model", None), ("thought-level", None)];
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let flag = arg.strip_prefix("--");
        if let Some((name, set)) = by_category.iter_mut().find(|(name, _)| Some(*name) == flag) {
            let value = it.next().ok_or(format!("--{name} needs a value"))?;
            if let Some(was) = set.replace(value.clone())
                && was != *value
            {
                return Err(format!("--{name} {was} and --{name} {value} disagree"));
            }
            continue;
        }
        match arg.as_str() {
            "--json" => json_out = true,
            "--option" => pairs.push(it.next().ok_or("--option needs <option>=<value>")?.clone()),
            flag if flag.starts_with("--") => return Err(format!("unknown option: {flag}")),
            _ if session.is_none() => session = Some(arg.clone()),
            _ => return Err(USAGE.to_owned()),
        }
    }
    let options = options("--option", &pairs)?;
    let [(_, mode), (_, model), (_, thought_level)] = by_category;
    if mode.is_none() && model.is_none() && thought_level.is_none() && options.is_empty() {
        return Err(USAGE.to_owned());
    }
    let (host, status) = session_status(&session.ok_or(USAGE)?)?;
    let id = s(&status["session_id"]).to_owned();
    let req = json!({
        "cmd": "set_config",
        "session": id,
        "mode": mode,
        "model": model,
        "thought_level": thought_level,
        "options": options,
    });
    let response = agent_call(&host, &req)?;
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
/// twice with two values fails (ADR 58).
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

pub(super) fn commands(args: &[String]) -> Result<ExitCode, String> {
    let (arg, rest, json_out) = session_args(args)?;
    if !rest.is_empty() {
        return Err(USAGE.to_owned());
    }
    let (_, status) = session_status(&arg)?;
    let commands: Vec<Value> = status["commands"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|c| json!({ "command": c["name"], "hint": c["input"]["hint"], "description": c["description"] }))
        .collect();
    if json_out {
        print_json(&json!({ "session": status["session_id"], "commands": commands }))?;
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

// ---- the agent's sessions ------------------------------------------------

/// `brnr sessions`: the agent's own list of sessions in a folder, asked of
/// an agent started just for that (no process of brnr's), with what brnr
/// knows of each.
pub(super) fn sessions(args: &[String]) -> Result<ExitCode, String> {
    let (mut profile, mut cwd, mut json_out, mut agent) = (None, None, false, Vec::new());
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--profile" => profile = Some(it.next().ok_or("--profile needs a name")?.clone()),
            "--cwd" => cwd = Some(it.next().ok_or("--cwd needs a directory")?.clone()),
            "--json" => json_out = true,
            "--" => {
                agent = it.by_ref().cloned().collect();
                break;
            }
            _ => return Err(USAGE.to_owned()),
        }
    }
    let cfg = config::load(profile.as_deref())?;
    if agent.is_empty() {
        agent = cfg.agent.clone().unwrap_or_default();
    }
    if agent.is_empty() {
        return Err("no agent: give one after -- or set agent in the profile".into());
    }
    let cwd = match cwd {
        Some(dir) => paths::expand(&dir),
        None => env::current_dir().map_err(|e| format!("cwd: {e}"))?,
    };
    let cwd = std::path::absolute(&cwd).map_err(|e| format!("{}: {e}", cwd.display()))?;
    let listed = list_sessions(&agent, &cwd.to_string_lossy())?;

    // What brnr knows of each, as brnr list says it: a running session's
    // state and process, from the locks too (`unreachable`, in a process
    // that doesn't answer), `inactive` for one with a transcript, nothing
    // for one only the agent knows.
    let hosts = discover()?;
    let locks = lock::all();
    let running = running_rows(&hosts, &locks);
    let past = inactive_sessions(&hosts);
    let known = |id: &str| -> (Value, Value, Value) {
        // One that two processes serve (`shared_sessions`, ADR 42) is the
        // lock holder's, as its transcript is.
        let holder = locks.iter().find(|e| e.pid.is_some() && e.session.as_deref() == Some(id));
        let holder = holder.and_then(|e| e.pid).map(u64::from);
        let mut rows = running.iter().filter(|r| r["session"] == id);
        let row = rows.clone().find(|r| r["pid"].as_u64() == holder).or_else(|| rows.next());
        if let Some(r) = row {
            return (r["state"].clone(), r["pid"].clone(), r["last_active"].clone());
        }
        match past.iter().find(|p| p["session_id"] == id) {
            Some(p) => (json!("inactive"), Value::Null, p["last_active"].clone()),
            None => (Value::Null, Value::Null, Value::Null),
        }
    };
    let mut rows: Vec<Value> = listed
        .iter()
        .map(|x| {
            let id = x.session_id.to_string();
            let (state, pid, active) = known(&id);
            json!({
                "session": id,
                "title": x.title,
                "state": state,
                "pid": pid,
                // The agent's updatedAt; brnr's own when it has none.
                "last_active": x.updated_at.as_ref().map_or(active, |t| json!(t)),
                "cwd": x.cwd,
            })
        })
        .collect();
    // Most recently active first, like brnr list.
    rows.sort_by(|a, b| b["last_active"].as_str().cmp(&a["last_active"].as_str()));
    if json_out {
        print_json(&json!(rows))?;
        return Ok(ExitCode::SUCCESS);
    }
    if rows.is_empty() {
        outln!("the agent knows no sessions in {}", cwd.display());
        return Ok(ExitCode::SUCCESS);
    }
    let mut table =
        vec![["SESSION", "TITLE", "STATE", "PID", "LAST ACTIVE", "CWD"].map(String::from)];
    for r in &rows {
        table.push([
            text(&r["session"]),
            r["title"].as_str().unwrap_or("-").to_owned(),
            r["state"].as_str().unwrap_or("-").to_owned(),
            r["pid"].as_u64().map_or("-".to_owned(), |p| p.to_string()),
            r["last_active"].as_str().map_or("-".to_owned(), when),
            text(&r["cwd"]),
        ]);
    }
    print_table(table);
    Ok(ExitCode::SUCCESS)
}

/// Starts `agent`, asks it for its sessions in `cwd` (`initialize`, then
/// `session/list` page by page) and stops it.
fn list_sessions(agent: &[String], cwd: &str) -> Result<Vec<SessionInfo>, String> {
    let mut program = paths::expand(&agent[0]).into_os_string();
    if let Some(bundled) = spawn::bundled(&program) {
        program = bundled.into_os_string();
    }
    let mut child = Command::new(&program)
        .args(&agent[1..])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .map_err(|e| format!("{}: {e}", program.to_string_lossy()))?;
    let pid = child.id() as i32;
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel::<String>();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                return;
            }
        }
    });
    let deadline = Instant::now() + LIST_TIMEOUT;
    let mut ask = |id: u64, method: &str, params: Value| -> Result<Value, String> {
        let req = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        writeln!(stdin, "{req}").map_err(|e| format!("the agent: {e}"))?;
        loop {
            let wait = deadline.saturating_duration_since(Instant::now());
            let line =
                rx.recv_timeout(wait).map_err(|_| format!("the agent didn't answer {method}"))?;
            // As the host reads it (see json.rs): a title cut mid-emoji is no
            // reason to miss the answer.
            let line = line.as_bytes();
            let Some(Some(msg)) = json::on_stack(json::depth(line), || json::parse::<Value>(line))
            else {
                continue;
            };
            if msg["id"] == id && msg.get("method").is_none() {
                if let Some(error) = msg.get("error") {
                    return Err(format!("{method} failed: {}", schema::error_message(error)));
                }
                return Ok(msg["result"].clone());
            }
        }
    };
    let result = (|| {
        let init = ask(
            1,
            "initialize",
            json!({
                "protocolVersion": schema::PROTOCOL_VERSION,
                "clientCapabilities": {},
                "clientInfo": { "name": "brnr", "version": env!("CARGO_PKG_VERSION") },
            }),
        )?;
        schema::check_protocol_version(&init)?;
        let caps: AgentCapabilities = schema::read(&init["agentCapabilities"]).unwrap_or_default();
        if caps.session_capabilities.list.is_none() {
            return Err("the agent doesn't list its sessions".to_owned());
        }
        let mut sessions = Vec::new();
        let mut cursor: Option<String> = None;
        for id in 2.. {
            let mut params = json!({ "cwd": cwd });
            if let Some(cursor) = &cursor {
                params["cursor"] = json!(cursor);
            }
            let page = ask(id, "session/list", params)?;
            let page = schema::read::<ListSessionsResponse>(&page)
                .ok_or("session/list failed: the agent's answer isn't a list of sessions")?;
            sessions.extend(page.sessions);
            match page.next_cursor {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
        Ok(sessions)
    })();
    drop(ask);
    drop(stdin);
    sys::kill(-pid, libc::SIGTERM);
    let until = Instant::now() + STOP_WAIT;
    loop {
        match child.try_wait() {
            Ok(None) if Instant::now() < until => thread::sleep(Duration::from_millis(20)),
            // Not reaped yet, so the group id is still the agent's.
            Ok(None) => {
                sys::kill(-pid, libc::SIGKILL);
                let _ = child.wait();
                break;
            }
            Ok(Some(_)) | Err(_) => break,
        }
    }
    result
}

// ---- forking, closing ---------------------------------------------------

pub(super) fn fork(args: &[String]) -> Result<ExitCode, String> {
    let (arg, rest, json_out) = session_args(args)?;
    if !rest.is_empty() {
        return Err(USAGE.to_owned());
    }
    let hosts = discover()?;
    let (host, from) = running_session(&hosts, &arg)?;
    let response = agent_call(host, &json!({ "cmd": "fork", "session": from }))?;
    let session = text(&response["session"]);
    if json_out {
        print_json(&json!({ "session": session, "from": from }))?;
    } else {
        outln!("forked {from} into {session}");
    }
    Ok(ExitCode::SUCCESS)
}

pub(super) fn close(args: &[String]) -> Result<ExitCode, String> {
    let [arg] = args else { return Err(USAGE.to_owned()) };
    let hosts = discover()?;
    let (host, id) = running_session(&hosts, arg)?;
    agent_call(host, &json!({ "cmd": "close", "session": id }))?;
    outln!("closed {id}");
    Ok(ExitCode::SUCCESS)
}

/// `session resume --take-over`: process `pid`, which holds `session`'s
/// lock, and the session as its transcript has it once closed there (its
/// cwd, the process's agent and profile), for the process that resumes it.
pub(super) fn held<'a>(
    hosts: &'a [Host],
    session: &str,
    pid: u32,
) -> Result<(&'a Host, Value), String> {
    let running = format!("{session} is running in process {pid}");
    let Some(host) = hosts.iter().find(|h| h.id() == pid.to_string()) else {
        return Err(format!("{running}, which brnr doesn't list"));
    };
    let Some(status) = &host.status else {
        return Err(format!("{running}, which is not answering"));
    };
    let Some(s) = host.sessions().iter().find(|s| s["session_id"] == session) else {
        return Err(format!("{session} is opening in process {pid}"));
    };
    let past = json!({
        "session_id": session,
        "cwd": s["cwd"],
        "agent": status["agent"],
        "profile": status["profile"],
    });
    Ok((host, past))
}

/// `session resume --take-over`: `owner`, which holds `session`'s lock, closes
/// it as `brnr session close` does, cancelling a running turn, and so lets go
/// of it (ADR 3) for process `to` to resume. An editor's process does only if
/// its profile enables the experimental `close`, and tells the editor where the
/// session went (ADR 4).
pub(super) fn take_over(owner: &Host, session: &str, to: u32) -> Result<(), String> {
    let pid = owner.id();
    let running = format!("{session} is running in process {pid}");
    let req = json!({ "cmd": "close", "session": session, "take_over": to });
    agent_call(owner, &req).map_err(|e| {
        if owner.info()["owner"] == "editor" {
            format!("{running}, an editor's: {e}")
        } else {
            format!("{running}, which didn't close it: {e}")
        }
    })?;
    errln!("brnr: closed {session} in process {pid}");
    Ok(())
}
