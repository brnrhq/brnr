//! The control commands: start headless sessions and talk to running hosts
//! over their control sockets.
//!
//! ```text
//! brnr start [--profile <p>] [--name <n>] [--cwd <dir>] [--prompt <text> | --prompt -]
//!            [-- <agent> [args...]]
//! brnr list [--all | --inactive] [--json]
//! brnr status <target>
//! brnr send <target> [--session <id>] [--after-turn | --interrupt | --context [--replace]]
//!           (<text>... | -)
//! brnr pending [<target>]
//! brnr approve <target> [<request>] [--option <id>]
//! brnr deny <target> [<request>] [--option <id>]
//! brnr watch <target> [--events <a,b,...>] [--json]
//! brnr stop <target>
//! ```
//!
//! `<target>` is a host id (from `list`), a `--name`, or an ACP session id
//! or unique prefix of one.
//!
//! `list` shows running hosts; `--all` adds inactive sessions (ones no
//! running host serves, found from their transcripts) and `--inactive`
//! shows only those.
//!
//! `send` timing:
//! - default: send now; starts a turn if the agent is idle, otherwise the
//!   agent decides (claude-agent-acp folds it into the running turn).
//! - `--after-turn`: the host holds it until no turn is running.
//! - `--interrupt`: the host cancels the running turn, then sends it.
//! - `--context`: no turn; appended to the next prompt, whoever sends it.
//!   `--replace` replaces the last held context instead of adding to it.

use std::env;
use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{ExitCode, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};

use brnr::{config, paths, spawn};

const USAGE: &str = "usage:
  brnr proxy [--profile <p>] [--name <n>] [--on-disconnect direct|headless] [-- <agent> [args...]]
             what an editor runs as its agent
  brnr host [--profile <p>] [--name <n>] [--cwd <dir>] [--prompt <text> | -] [-- <agent> [args...]]
             a headless session in the foreground (brnr host --help)
  brnr start [--profile <p>] [--name <n>] [--cwd <dir>] [--prompt <text> | --prompt -] [-- <agent> [args...]]
  brnr list [--all | --inactive] [--json]
  brnr status <target>
  brnr send <target> [--session <id>] [--after-turn | --interrupt | --context [--replace]] (<text>... | -)
  brnr pending [<target>]
  brnr approve <target> [<request>] [--option <id>]
  brnr deny <target> [<request>] [--option <id>]
  brnr watch <target> [--events <a,b,...>] [--json]
  brnr stop <target>
  brnr --version";

/// How long `start` waits for the agent to open its session.
const START_TIMEOUT: Duration = Duration::from_secs(120);

/// The control commands: everything but `proxy` and `host`.
pub fn main(args: Vec<String>) -> ExitCode {
    let rest = args.get(1..).unwrap_or_default();
    let result = match args.first().map(String::as_str) {
        Some("start") => start(rest),
        Some("list") => list(rest),
        Some("status") => status(rest),
        Some("send") => send(rest),
        Some("pending") => pending(rest),
        Some("approve") => answer(rest, "approve"),
        Some("deny") => answer(rest, "deny"),
        Some("watch") => watch(rest),
        Some("stop") => stop(rest),
        Some("-h" | "--help") => {
            println!("{USAGE}");
            Ok(())
        }
        Some("-V" | "--version") => {
            println!("brnr {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        _ => Err(USAGE.to_owned()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("brnr: {msg}");
            ExitCode::FAILURE
        }
    }
}

/// A running host: its metadata file and, if it answered, its live status.
struct Host {
    meta: Value,
    status: Option<Value>,
}

impl Host {
    fn info(&self) -> &Value {
        self.status.as_ref().unwrap_or(&self.meta)
    }

    fn id(&self) -> &str {
        self.meta["id"].as_str().unwrap_or("?")
    }

    fn sessions(&self) -> &[Value] {
        self.status.as_ref().and_then(|s| s["sessions"].as_array()).map_or(&[], Vec::as_slice)
    }
}

// ---- start ---------------------------------------------------------------

fn start(args: &[String]) -> Result<(), String> {
    let mut profile = None;
    let mut name = None;
    let mut cwd = None;
    let mut prompt = None;
    let mut agent = Vec::new();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let mut value = |flag: &str| it.next().cloned().ok_or(format!("{flag} needs a value"));
        match arg.as_str() {
            "--profile" => profile = Some(value("--profile")?),
            "--name" => name = Some(value("--name")?),
            "--cwd" => cwd = Some(value("--cwd")?),
            "--prompt" => prompt = Some(value("--prompt")?),
            "--" => {
                agent = it.by_ref().cloned().collect();
                break;
            }
            other => return Err(format!("unknown option: {other}")),
        }
    }
    if prompt.as_deref() == Some("-") {
        prompt = Some(read_stdin()?);
    }
    let cfg = config::load(profile.as_deref())?;
    let cwd = match cwd.or(cfg.cwd) {
        Some(dir) => paths::expand(&dir),
        None => env::current_dir().map_err(|e| format!("cwd: {e}"))?,
    };
    let cwd = std::path::absolute(&cwd).map_err(|e| format!("{}: {e}", cwd.display()))?;
    if !cwd.is_dir() {
        return Err(format!("{}: not a directory", cwd.display()));
    }

    let (ready_rx, ready_tx) = io::pipe().map_err(|e| format!("pipe: {e}"))?;
    let mut cmd = spawn::host_command().map_err(|e| e.to_string())?;
    cmd.arg("--ready-fd").arg("3");
    for (flag, value) in [("--profile", &profile), ("--name", &name), ("--prompt", &prompt)] {
        if let Some(value) = value {
            cmd.arg(flag).arg(value);
        }
    }
    if !agent.is_empty() {
        cmd.arg("--").args(&agent);
    }
    cmd.current_dir(&cwd).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    spawn::detached(&mut cmd, ready_tx.as_raw_fd(), 3)
        .map_err(|e| format!("starting host: {e}"))?;
    drop(ready_tx);

    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut line = String::new();
        let _ = BufReader::new(ready_rx).read_line(&mut line);
        let _ = tx.send(line);
    });
    let line = rx
        .recv_timeout(START_TIMEOUT)
        .map_err(|_| "timed out waiting for the session".to_owned())?;
    let ready: Value =
        serde_json::from_str(&line).map_err(|_| "the host exited without starting a session")?;
    if ready["ok"].as_bool() != Some(true) {
        return Err(ready["error"].as_str().unwrap_or("start failed").to_owned());
    }
    println!(
        "started {} (session {})",
        ready["id"].as_str().unwrap_or("?"),
        ready["session"].as_str().unwrap_or("?")
    );
    Ok(())
}

// ---- inspecting ----------------------------------------------------------

fn list(args: &[String]) -> Result<(), String> {
    let (mut json_out, mut active, mut inactive) = (false, true, false);
    for arg in args {
        match arg.as_str() {
            "--json" => json_out = true,
            "--all" => inactive = true,
            "--inactive" => (active, inactive) = (false, true),
            _ => return Err(USAGE.to_owned()),
        }
    }
    let hosts = discover();
    let past = if inactive { inactive_sessions(&hosts) } else { Vec::new() };
    if json_out {
        let running: Vec<&Value> = hosts.iter().map(Host::info).collect();
        let out = match (active, inactive) {
            (true, false) => json!(running),
            (false, _) => json!(past),
            (true, true) => json!({ "active": running, "inactive": past }),
        };
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
        return Ok(());
    }
    if active {
        list_active(&hosts);
    }
    if inactive {
        if active {
            println!();
        }
        list_inactive(&past);
    }
    Ok(())
}

fn list_active(hosts: &[Host]) {
    if hosts.is_empty() {
        println!("no running sessions");
        return;
    }
    let mut rows = vec![["ID", "NAME", "OWNER", "AGENT", "SESSIONS", "CWD"].map(String::from)];
    for host in hosts {
        let info = host.info();
        let sessions: Vec<String> = host
            .sessions()
            .iter()
            .map(|s| {
                let id = s["session_id"].as_str().unwrap_or("?");
                let mut flags = Vec::new();
                if s["busy"].as_bool() == Some(true) {
                    flags.push("busy".to_owned());
                }
                for key in ["held", "context"] {
                    if let Some(n) = s[key].as_u64().filter(|&n| n > 0) {
                        flags.push(format!("{key}:{n}"));
                    }
                }
                if flags.is_empty() { id.to_owned() } else { format!("{id} ({})", flags.join(" ")) }
            })
            .collect();
        let mut owner = if host.status.is_some() {
            info["owner"].as_str().unwrap_or("?")
        } else {
            "unreachable"
        }
        .to_owned();
        if let Some(n) = info["pending"].as_u64().filter(|&n| n > 0) {
            owner.push_str(&format!(" ({n} pending)"));
        }
        rows.push([
            host.id().to_owned(),
            info["name"].as_str().unwrap_or("-").to_owned(),
            owner,
            agent_name(&info["agent"]),
            if sessions.is_empty() { "-".to_owned() } else { sessions.join(", ") },
            info["cwd"].as_str().unwrap_or("?").to_owned(),
        ]);
    }
    print_table(rows);
}

fn list_inactive(past: &[Value]) {
    if past.is_empty() {
        println!("no inactive sessions");
        return;
    }
    let mut rows =
        vec![["SESSION", "LAST ACTIVE", "ENDED", "NAME", "AGENT", "CWD"].map(String::from)];
    for s in past {
        let last = s["last_active"].as_str().unwrap_or("?");
        // 2026-10-01T19:34:25.959660Z -> 2026-10-01 19:34:25Z
        let last = match (last.get(..10), last.get(11..19)) {
            (Some(day), Some(time)) => format!("{day} {time}Z"),
            _ => last.to_owned(),
        };
        rows.push([
            s["session_id"].as_str().unwrap_or("?").to_owned(),
            last,
            s["ended"].as_str().unwrap_or("?").to_owned(),
            s["name"].as_str().unwrap_or("-").to_owned(),
            agent_name(&s["agent"]),
            s["cwd"].as_str().unwrap_or("?").to_owned(),
        ]);
    }
    print_table(rows);
}

/// Sessions with a transcript that no running host is serving, most
/// recently active first. Only the first and last record of each file are
/// read: the first names the cwd and host log, the last says when and under
/// which host the session was last active. That host's log says which agent
/// it ran and how it ended.
fn inactive_sessions(hosts: &[Host]) -> Vec<Value> {
    // A transcript is identified by its file (cwd folder + session id); a
    // session id alone can repeat across folders.
    let running: Vec<PathBuf> = hosts
        .iter()
        .flat_map(|h| h.sessions().iter().filter_map(|s| s["log"].as_str().map(PathBuf::from)))
        .collect();
    let projects = paths::state_dir().join("projects");
    let files = fs::read_dir(&projects)
        .into_iter()
        .flatten()
        .flatten()
        .flat_map(|dir| fs::read_dir(dir.path()).into_iter().flatten().flatten())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "jsonl"));

    let mut host_logs: std::collections::HashMap<String, (Value, Value)> = Default::default();
    let mut past = Vec::new();
    for file in files {
        let (Some(first), Some(last)) = (first_record(&file), last_record(&file)) else { continue };
        let Some(session) = last["session_id"].as_str() else { continue };
        if running.contains(&file) {
            continue;
        }
        let host_id = last["host_id"].as_str().unwrap_or_default().to_owned();
        let (started, exited) = host_logs
            .entry(host_id.clone())
            .or_insert_with(|| {
                let log = paths::host_log(&host_id);
                (first_record(&log).unwrap_or_default(), last_record(&log).unwrap_or_default())
            })
            .clone();
        let info = &started["event"]["info"];
        let ended = match &exited["event"] {
            e if e["event"] == "exited" => {
                match (e["status"]["code"].as_i64(), e["status"]["signal"].as_i64()) {
                    (Some(code), _) => format!("exit {code}"),
                    (_, Some(sig)) => format!("signal {sig}"),
                    _ => "exited".to_owned(),
                }
            }
            _ if alive(last["host_pid"].as_i64().unwrap_or(0)) => "closed".to_owned(),
            _ => "host lost".to_owned(),
        };
        past.push(json!({
            "session_id": session,
            "cwd": first["event"]["cwd"],
            "last_active": last["ts"],
            "ended": ended,
            "host_id": host_id,
            "name": info["name"],
            "agent": info["agent"],
            "log": file.to_string_lossy(),
        }));
    }
    past.sort_by(|a, b| b["last_active"].as_str().cmp(&a["last_active"].as_str()));
    past
}

fn first_record(path: &Path) -> Option<Value> {
    let mut line = String::new();
    BufReader::new(fs::File::open(path).ok()?).read_line(&mut line).ok()?;
    serde_json::from_str(&line).ok()
}

/// The last line of a file, read backwards so a long transcript costs no
/// more than its last record.
fn last_record(path: &Path) -> Option<Value> {
    use std::io::{Seek, SeekFrom};
    let mut file = fs::File::open(path).ok()?;
    let mut end = file.metadata().ok()?.len();
    let mut tail: Vec<u8> = Vec::new();
    loop {
        let start = end.saturating_sub(64 * 1024);
        let mut chunk = vec![0; (end - start) as usize];
        file.seek(SeekFrom::Start(start)).ok()?;
        file.read_exact(&mut chunk).ok()?;
        chunk.extend_from_slice(&tail);
        tail = chunk;
        let body = tail.strip_suffix(b"\n").unwrap_or(&tail);
        if let Some(i) = body.iter().rposition(|&b| b == b'\n') {
            return serde_json::from_slice(&body[i + 1..]).ok();
        }
        if start == 0 {
            return serde_json::from_slice(body).ok();
        }
        end = start;
    }
}

fn agent_name(argv: &Value) -> String {
    let Some(program) = argv.as_array().and_then(|a| a.first()).and_then(Value::as_str) else {
        return "?".to_owned();
    };
    Path::new(program).file_name().map_or(program.to_owned(), |f| f.to_string_lossy().into_owned())
}

fn status(args: &[String]) -> Result<(), String> {
    let [target] = args else { return Err(USAGE.to_owned()) };
    let hosts = discover();
    let (host, _) = resolve(&hosts, target)?;
    let status = host.status.as_ref().ok_or("host is not answering")?;
    println!("{}", serde_json::to_string_pretty(status).unwrap());
    Ok(())
}

fn pending(args: &[String]) -> Result<(), String> {
    let hosts = discover();
    let chosen: Vec<&Host> = match args {
        [] => hosts.iter().collect(),
        [target] => vec![resolve(&hosts, target)?.0],
        _ => return Err(USAGE.to_owned()),
    };
    let mut rows =
        vec![["HOST", "REQUEST", "OWNER", "SESSION", "TITLE", "OPTIONS"].map(String::from)];
    for host in chosen {
        let response = request(host, &json!({ "cmd": "pending" }))
            .map_err(|e| format!("host {}: {e}", host.id()))?;
        for p in response["pending"].as_array().into_iter().flatten() {
            let options: Vec<String> = p["options"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|o| {
                    format!(
                        "{}={}",
                        o["optionId"].as_str().unwrap_or("?"),
                        o["kind"].as_str().unwrap_or("?")
                    )
                })
                .collect();
            rows.push([
                host.id().to_owned(),
                p["request"].as_str().unwrap_or("?").to_owned(),
                p["owner"].as_str().unwrap_or("?").to_owned(),
                p["session"].as_str().unwrap_or("-").to_owned(),
                p["title"].as_str().unwrap_or("-").to_owned(),
                options.join(" "),
            ]);
        }
    }
    if rows.len() == 1 {
        println!("nothing waiting");
    } else {
        print_table(rows);
    }
    Ok(())
}

// ---- acting --------------------------------------------------------------

fn send(args: &[String]) -> Result<(), String> {
    let mut target = None;
    let mut session = None;
    let mut mode = None;
    let mut replace = false;
    let mut words = Vec::new();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let set_mode = |mode: &mut Option<&str>, m| match mode.replace(m) {
            Some(prev) if prev != m => Err(format!("--{prev} and --{m} don't go together")),
            _ => Ok(()),
        };
        match arg.as_str() {
            "--session" => session = Some(it.next().ok_or("--session needs an id")?.clone()),
            "--after-turn" => set_mode(&mut mode, "after-turn")?,
            "--interrupt" => set_mode(&mut mode, "interrupt")?,
            "--context" => set_mode(&mut mode, "context")?,
            "--replace" => replace = true,
            "--" => words.extend(it.by_ref().cloned()),
            flag if flag.starts_with("--") => return Err(format!("unknown option: {flag}")),
            _ if target.is_none() => target = Some(arg.clone()),
            _ => words.push(arg.clone()),
        }
    }
    let target = target.ok_or(USAGE)?;
    if replace && mode != Some("context") {
        return Err("--replace only applies to --context".into());
    }
    let text = if words == ["-"] { read_stdin()? } else { words.join(" ") };
    if text.trim().is_empty() {
        return Err("nothing to send".into());
    }

    let hosts = discover();
    let (host, matched) = resolve(&hosts, &target)?;
    let req = json!({
        "cmd": "send",
        "session": session.or(matched),
        "text": text,
        "mode": mode.unwrap_or("now"),
        "replace": replace,
    });
    let response = call(host, &req)?;
    println!(
        "{} (session {})",
        response["status"].as_str().unwrap_or("?"),
        response["session"].as_str().unwrap_or("?")
    );
    Ok(())
}

fn answer(args: &[String], cmd: &str) -> Result<(), String> {
    let mut positional = Vec::new();
    let mut option = None;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--option" => option = Some(it.next().ok_or("--option needs an id")?.clone()),
            flag if flag.starts_with("--") => return Err(format!("unknown option: {flag}")),
            _ => positional.push(arg.clone()),
        }
    }
    let (target, request_id) = match &positional[..] {
        [target] => (target, None),
        [target, request_id] => (target, Some(request_id.clone())),
        _ => return Err(USAGE.to_owned()),
    };
    let hosts = discover();
    let (host, session) = resolve(&hosts, target)?;
    let req = json!({ "cmd": cmd, "request": request_id, "option": option, "session": session });
    let response = call(host, &req)?;
    let outcome = &response["outcome"];
    let what = outcome["optionId"].as_str().or(outcome["outcome"].as_str()).unwrap_or("?");
    println!("{} {what}", response["request"].as_str().unwrap_or("?"));
    Ok(())
}

fn stop(args: &[String]) -> Result<(), String> {
    let [target] = args else { return Err(USAGE.to_owned()) };
    let hosts = discover();
    let (host, _) = resolve(&hosts, target)?;
    let response = call(host, &json!({ "cmd": "stop" }))?;
    println!("{}", response["status"].as_str().unwrap_or("?"));
    Ok(())
}

/// Prints the host's events until it exits: by default every event,
/// including each ACP message in both directions, one line each. A target
/// that names a session shows only that session's events (plus the host's
/// own, such as the agent exiting).
fn watch(args: &[String]) -> Result<(), String> {
    let mut target = None;
    let mut events = json!("all");
    let mut json_out = false;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--events" => {
                let list = it.next().ok_or("--events needs a list")?;
                events = json!(list.split(',').map(str::trim).collect::<Vec<_>>());
            }
            "--json" => json_out = true,
            flag if flag.starts_with("--") => return Err(format!("unknown option: {flag}")),
            _ if target.is_none() => target = Some(arg.clone()),
            _ => return Err(USAGE.to_owned()),
        }
    }
    let target = target.ok_or(USAGE)?;
    let hosts = discover();
    let (host, only_session) = resolve(&hosts, &target)?;
    let mut conn = connect(host).map_err(|e| format!("host {}: {e}", host.id()))?;
    writeln!(conn, "{}", json!({ "cmd": "subscribe", "events": events }))
        .map_err(|e| e.to_string())?;
    let mut lines = BufReader::new(conn).lines();
    let first: Value = lines
        .next()
        .and_then(Result::ok)
        .and_then(|l| serde_json::from_str(&l).ok())
        .ok_or("no answer from the host")?;
    if first["ok"].as_bool() != Some(true) {
        return Err(first["error"].as_str().unwrap_or("subscribe failed").to_owned());
    }
    let mut out = io::stdout().lock();
    for line in lines.map_while(Result::ok) {
        let Ok(event) = serde_json::from_str::<Value>(&line) else { continue };
        if let (Some(only), Some(session)) = (&only_session, event["session"].as_str())
            && only != session
        {
            continue;
        }
        let text = if json_out { line } else { describe(&event) };
        if writeln!(out, "{text}").and_then(|()| out.flush()).is_err() {
            break;
        }
    }
    Ok(())
}

/// One event as `HH:MM:SS.mmm  <session>  <what happened>`, continuation
/// lines indented under the description.
fn describe(e: &Value) -> String {
    let s = |v: &Value| v.as_str().unwrap_or("?").to_owned();
    let time = e["ts"].as_str().and_then(|t| t.get(11..23)).unwrap_or("");
    let session: String = e["session"].as_str().unwrap_or("-").chars().take(8).collect();
    let what = match e["event"].as_str().unwrap_or("?") {
        "user_message" => format!("user ({}): {}", s(&e["by"]), s(&e["text"])),
        "agent_message" => format!("agent: {}", s(&e["text"])),
        "permission_request" => {
            let options: Vec<String> =
                e["options"].as_array().into_iter().flatten().map(|o| s(&o["optionId"])).collect();
            format!(
                "permission {} ({} answers): {} [{}]",
                s(&e["request"]),
                s(&e["owner"]),
                s(&e["title"]),
                options.join(" ")
            )
        }
        "permission_resolved" => {
            let outcome = &e["outcome"];
            let chosen =
                outcome["optionId"].as_str().or(outcome["outcome"].as_str()).unwrap_or("?");
            format!("permission {} -> {chosen} (by {})", s(&e["request"]), s(&e["by"]))
        }
        "turn_ended" => match e["error"].as_object() {
            Some(error) => format!("turn failed: {} ({})", error["message"], s(&e["by"])),
            None => format!("turn ended: {} ({})", s(&e["stop_reason"]), s(&e["by"])),
        },
        "owner_changed" => format!("owner -> {} ({})", s(&e["owner"]), s(&e["reason"])),
        "exited" => format!("agent exited: {}", e["status"]),
        "acp" => format!("{:<15} {}", s(&e["dir"]), e["msg"]),
        other => format!("{other}: {e}"),
    };
    let indent = " ".repeat(time.len() + 2 + 8 + 2);
    let what = what.replace('\n', &format!("\n{indent}"));
    format!("{time}  {session:<8}  {what}")
}

// ---- plumbing ------------------------------------------------------------

/// The host `target` names, and the ACP session if it named one.
fn resolve<'a>(hosts: &'a [Host], target: &str) -> Result<(&'a Host, Option<String>), String> {
    if let Some(host) = hosts.iter().find(|h| h.id() == target || h.meta["name"] == target) {
        return Ok((host, None));
    }
    let mut matches = Vec::new();
    for host in hosts {
        for s in host.sessions() {
            let id = s["session_id"].as_str().unwrap_or_default();
            if id == target {
                return Ok((host, Some(id.to_owned())));
            }
            if id.starts_with(target) {
                matches.push((host, id.to_owned()));
            }
        }
    }
    match matches.len() {
        0 => Err(format!("no host or session matches {target} (see brnr list)")),
        1 => {
            let (host, id) = matches.pop().unwrap();
            Ok((host, Some(id)))
        }
        _ => Err(format!("{target} matches several sessions")),
    }
}

/// Every host with a metadata file, asking each for its live status. Files
/// left by a host that no longer exists are removed.
fn discover() -> Vec<Host> {
    let dir = paths::runtime_dir();
    let Ok(entries) = fs::read_dir(&dir) else { return Vec::new() };
    let mut hosts = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        let Some(meta) =
            fs::read(&path).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        else {
            continue;
        };
        let mut host = Host { meta, status: None };
        host.status = request(&host, &json!({ "cmd": "status" })).ok();
        if host.status.is_none() && !alive(host.meta["host_pid"].as_i64().unwrap_or(0)) {
            let _ = fs::remove_file(&path);
            let _ = fs::remove_file(path.with_extension("sock"));
            continue;
        }
        hosts.push(host);
    }
    hosts.sort_by(|a, b| a.meta["started"].as_str().cmp(&b.meta["started"].as_str()));
    hosts
}

fn connect(host: &Host) -> io::Result<UnixStream> {
    let socket = host.meta["socket"].as_str().ok_or(io::Error::other("no socket in metadata"))?;
    UnixStream::connect(PathBuf::from(socket))
}

fn request(host: &Host, req: &Value) -> io::Result<Value> {
    let mut conn = connect(host)?;
    conn.set_read_timeout(Some(Duration::from_secs(5)))?;
    writeln!(conn, "{req}")?;
    let mut line = String::new();
    BufReader::new(conn).read_line(&mut line)?;
    serde_json::from_str(&line).map_err(io::Error::other)
}

/// A request whose failure is the command's failure.
fn call(host: &Host, req: &Value) -> Result<Value, String> {
    let response = request(host, req).map_err(|e| format!("host {}: {e}", host.id()))?;
    if response["ok"].as_bool() != Some(true) {
        return Err(response["error"].as_str().unwrap_or("request failed").to_owned());
    }
    Ok(response)
}

fn read_stdin() -> Result<String, String> {
    let mut text = String::new();
    io::stdin().read_to_string(&mut text).map_err(|e| format!("stdin: {e}"))?;
    Ok(text)
}

fn print_table<const N: usize>(rows: Vec<[String; N]>) {
    let widths: Vec<usize> =
        (0..N).map(|c| rows.iter().map(|r| r[c].chars().count()).max().unwrap()).collect();
    for row in rows {
        let cells: Vec<String> =
            row.iter().zip(&widths).map(|(cell, w)| format!("{cell:<w$}")).collect();
        println!("{}", cells.join("  ").trim_end());
    }
}

fn alive(pid: i64) -> bool {
    pid > 0
        && (unsafe { libc::kill(pid as libc::pid_t, 0) } == 0
            || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM))
}
