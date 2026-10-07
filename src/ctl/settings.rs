//! A session's settings and the sessions themselves: `mode`, `config`,
//! `model`, `commands`, `sessions`, `fork` and `close`.

use std::env;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::process::CommandExt;
use std::process::{Command, ExitCode, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use brnr::{config, json, paths, spawn};

use super::{
    Host, USAGE, discover, inactive_sessions, print_json, print_table, request_timeout,
    running_session, text, when,
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

pub(super) fn mode(args: &[String]) -> Result<ExitCode, String> {
    let (arg, rest, json_out) = session_args(args)?;
    let (host, status) = session_status(&arg)?;
    let id = s(&status["session_id"]).to_owned();
    match &rest[..] {
        [] => {
            let current = status["mode"].as_str();
            let mut modes: Vec<Value> = status["modes"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|m| json!({ "mode": m["id"], "name": m["name"], "description": m["description"] }))
                .collect();
            let mut current = current.map(str::to_owned);
            if modes.is_empty() {
                // An agent with modes only as a config option.
                let option =
                    status["config"].as_array().into_iter().flatten().find(|o| o["id"] == "mode");
                let option = option.ok_or("the agent offers no modes")?;
                modes = choices(option)
                    .into_iter()
                    .map(|(value, name)| json!({ "mode": value, "name": name, "description": null }))
                    .collect();
                current = option["currentValue"].as_str().map(str::to_owned);
            }
            if json_out {
                print_json(&json!({ "session": id, "mode": current, "modes": modes }))?;
                return Ok(ExitCode::SUCCESS);
            }
            for m in &modes {
                let mark = if m["mode"].as_str() == current.as_deref() { "*" } else { " " };
                let about = m["description"].as_str().or(m["name"].as_str());
                let about = about.map(|d| format!("  {d}")).unwrap_or_default();
                outln!("{mark} {}{about}", s(&m["mode"]));
            }
        }
        [mode] => {
            agent_call(&host, &json!({ "cmd": "set_mode", "session": id, "mode": mode }))?;
            if json_out {
                print_json(&json!({ "session": id, "mode": mode }))?;
            } else {
                outln!("mode {mode}");
            }
        }
        _ => return Err(USAGE.to_owned()),
    }
    Ok(ExitCode::SUCCESS)
}

/// A select option's values and their names.
fn choices(option: &Value) -> Vec<(Value, Value)> {
    option["options"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|c| (c["value"].clone(), c["name"].clone()))
        .collect()
}

pub(super) fn config(args: &[String]) -> Result<ExitCode, String> {
    let (arg, rest, json_out) = session_args(args)?;
    let (host, status) = session_status(&arg)?;
    let id = s(&status["session_id"]).to_owned();
    if rest.is_empty() {
        let options: Vec<Value> = status["config"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|o| {
                let choices: Vec<Value> = choices(o).into_iter().map(|(v, _)| v).collect();
                json!({ "option": o["id"], "value": o["currentValue"], "choices": choices, "name": o["name"] })
            })
            .collect();
        if json_out {
            print_json(&json!({ "session": id, "options": options }))?;
            return Ok(ExitCode::SUCCESS);
        }
        if options.is_empty() {
            return Err("the agent has no config options".into());
        }
        let mut rows = vec![["OPTION", "VALUE", "CHOICES", "NAME"].map(String::from)];
        for o in &options {
            let value = match &o["value"] {
                Value::String(v) => v.clone(),
                other => other.to_string(),
            };
            let choices: Vec<&str> =
                o["choices"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
            rows.push([
                s(&o["option"]).to_owned(),
                value,
                choices.join(" "),
                o["name"].as_str().unwrap_or("").to_owned(),
            ]);
        }
        print_table(rows);
        return Ok(ExitCode::SUCCESS);
    }
    let mut set = Vec::new();
    for pair in &rest {
        let (option, value) =
            pair.split_once('=').ok_or(format!("<option>=<value>, not {pair}"))?;
        agent_call(
            &host,
            &json!({ "cmd": "set_config", "session": id, "option": option, "value": value }),
        )?;
        if !json_out {
            outln!("{option}={value}");
        }
        set.push(json!({ "option": option, "value": value }));
    }
    if json_out {
        print_json(&json!({ "session": id, "set": set }))?;
    }
    Ok(ExitCode::SUCCESS)
}

pub(super) fn model(args: &[String]) -> Result<ExitCode, String> {
    let (arg, rest, json_out) = session_args(args)?;
    let (host, status) = session_status(&arg)?;
    let id = s(&status["session_id"]).to_owned();
    match &rest[..] {
        [] => {
            let option = status["config"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|o| o["id"] == "model" || o["category"] == "model");
            let models: Vec<Value> = if let Some(option) = option {
                choices(option)
                    .into_iter()
                    .map(|(value, name)| json!({ "model": value, "name": name }))
                    .collect()
            } else if let Some(models) = status["models"].as_array() {
                models.iter().map(|m| json!({ "model": m["modelId"], "name": m["name"] })).collect()
            } else {
                return Err("the agent offers no model choice".into());
            };
            if json_out {
                print_json(&json!({ "session": id, "model": status["model"], "models": models }))?;
                return Ok(ExitCode::SUCCESS);
            }
            for m in &models {
                let mark = if m["model"] == status["model"] { "*" } else { " " };
                outln!("{mark} {}  {}", s(&m["model"]), m["name"].as_str().unwrap_or(""));
            }
        }
        [model] => {
            agent_call(&host, &json!({ "cmd": "set_model", "session": id, "model": model }))?;
            if json_out {
                print_json(&json!({ "session": id, "model": model }))?;
            } else {
                outln!("model {model}");
            }
        }
        _ => return Err(USAGE.to_owned()),
    }
    Ok(ExitCode::SUCCESS)
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

    // What brnr knows of each, in brnr list's terms: a running session's
    // state and process, `inactive` for one with a transcript, nothing for
    // one only the agent knows.
    let hosts = discover()?;
    let past = inactive_sessions(&hosts);
    let known = |id: &str| -> (Value, Value, Value) {
        for host in &hosts {
            if let Some(x) = host.sessions().iter().find(|x| x["session_id"] == id) {
                return (x["state"].clone(), json!(host.id().parse::<u64>().ok()), x["last_active"].clone());
            }
        }
        match past.iter().find(|p| p["session_id"] == id) {
            Some(p) => (json!("inactive"), Value::Null, p["last_active"].clone()),
            None => (Value::Null, Value::Null, Value::Null),
        }
    };
    let mut rows: Vec<Value> = listed
        .iter()
        .map(|x| {
            let id = s(&x["sessionId"]);
            let (state, pid, active) = known(id);
            json!({
                "session": id,
                "title": x["title"],
                "state": state,
                "pid": pid,
                // The agent's updatedAt; brnr's own when it has none.
                "last_active": if x["updatedAt"].is_string() { x["updatedAt"].clone() } else { active },
                "cwd": x["cwd"],
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
    let mut table = vec![["SESSION", "TITLE", "STATE", "PID", "LAST ACTIVE", "CWD"].map(String::from)];
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
fn list_sessions(agent: &[String], cwd: &str) -> Result<Vec<Value>, String> {
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
            let line = rx.recv_timeout(wait).map_err(|_| format!("the agent didn't answer {method}"))?;
            // As the host reads it (see json.rs): a title cut mid-emoji is no
            // reason to miss the answer.
            let line = line.as_bytes();
            let Some(Some(msg)) = json::on_stack(json::depth(line), || json::parse::<Value>(line))
            else {
                continue;
            };
            if msg["id"] == id && msg.get("method").is_none() {
                if let Some(error) = msg.get("error") {
                    let what = error["message"].as_str().map_or(error.to_string(), str::to_owned);
                    return Err(format!("{method} failed: {what}"));
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
                "protocolVersion": 1,
                "clientCapabilities": {},
                "clientInfo": { "name": "brnr", "version": env!("CARGO_PKG_VERSION") },
            }),
        )?;
        if init["agentCapabilities"]["sessionCapabilities"]["list"].is_null() {
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
            sessions.extend(page["sessions"].as_array().cloned().unwrap_or_default());
            match page["nextCursor"].as_str() {
                Some(next) => cursor = Some(next.to_owned()),
                None => break,
            }
        }
        Ok(sessions)
    })();
    drop(ask);
    drop(stdin);
    unsafe { libc::kill(-pid, libc::SIGTERM) };
    let until = Instant::now() + STOP_WAIT;
    loop {
        match child.try_wait() {
            Ok(None) if Instant::now() < until => thread::sleep(Duration::from_millis(20)),
            // Not reaped yet, so the group id is still the agent's.
            Ok(None) => {
                unsafe { libc::kill(-pid, libc::SIGKILL) };
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
