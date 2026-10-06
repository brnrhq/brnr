//! The control commands: start headless sessions and talk to running hosts
//! over their control sockets. `brnr --help` lists them; the modules have
//! the details:
//!
//! - talk.rs: `start`, `send`, `wait`, `cancel`, `queue`
//! - history.rs: `log`
//! - settings.rs: `mode`, `config`, `model`, `commands`, `sessions`, `fork`,
//!   `close`
//! - show.rs: `show`; notify.rs: `notify`; doctor.rs: `doctor`
//! - here: `list`, `status`, `pending`, `approve`, `deny`, `watch`, `stop`
//!
//! `<target>` is a host id (from `list`), a `--name`, or an ACP session id
//! or unique prefix of one.
//!
//! `list` shows running hosts; `--all` adds inactive sessions (ones no
//! running host serves, found from their transcripts) and `--inactive`
//! shows only those.
//!
//! `start` waits `BRNR_START_TIMEOUT` seconds (default 120) for the session;
//! if it gives up or is interrupted, the host stops without sending the
//! prompt. A `--name` must be unique among running hosts.
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
use std::io::{self, BufRead, BufReader, ErrorKind, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use serde_json::{Value, json};

use brnr::host::{EVENTS, QUIET, alive};
use brnr::{paths, render};

mod doctor;
mod history;
mod notify;
mod settings;
mod show;
mod talk;

const USAGE: &str = "usage:
  brnr acp [--profile <p>] [--name <n>] [--on-disconnect direct|headless] [-- <agent> [args...]]
             what an editor runs as its ACP agent

  brnr start [--profile <p>] [--name <n>] [--cwd <dir>] [--prompt <text> | -] [--file <path>]...
             [--image <path>]... [--mode <m>] [--model <m>] [--set <option>=<value>]...
             [--permissions ask|auto-allow|auto-deny] [--stop-when-idle]
             [--resume <session>] [--wait [--timeout <s>]] [-- <agent> [args...]]
             a headless session in the background

  brnr host [--profile <p>] [--name <n>] [--cwd <dir>] [--prompt <text> | -] [-- <agent> [args...]]
             a headless session in the foreground (brnr host --help)

  brnr stop <target>
             ends a host, however it started, and its agent

chat
  brnr send <target> [--session <id>] [--after-turn | --interrupt | --context [--replace]]
            [--file <path>]... [--image <path>]... [--wait [--timeout <s>]] (<text>... | -)
  brnr wait <target> [--session <id>] [--for idle|turn|permission|exit] [--timeout <s>]
  brnr cancel <target> [--session <id>] [--keep-held]
  brnr queue <target> [--session <id>] [--drop <message>] [--clear] [--clear-context]

approvals
  brnr pending [<target>]
  brnr show <target> [<request>]
  brnr approve <target> [<request>] [--option <id>]
  brnr deny <target> [<request>] [--option <id>]

sessions
  brnr list [--all | --inactive] [--json]
  brnr status <target> [--json]
  brnr sessions <target>
  brnr fork <target> [--session <id>]
  brnr close <target> [--session <id>]

events
  brnr log <target> [--session <id>] [--last <n>] [--follow] [--events <default|all|event>,...]
           [--json]
  brnr watch <target> [--events <default|all|event>,...] [--json]
  brnr notify [<target>] [--events <default|all|event>,...] -- <command> [args...]

settings
  brnr mode <target> [--session <id>] [<mode>]
  brnr model <target> [--session <id>] [<model>]
  brnr config <target> [--session <id>] [<option>=<value>...]
  brnr commands <target> [--session <id>]

brnr
  brnr doctor [--fix]
  brnr --version

<target> is a host id, a --name, or an ACP session id (or a unique prefix of
one); --session <id> picks the session when the host has several.";

/// How long `start` waits for the agent to open its session, in seconds,
/// unless `BRNR_START_TIMEOUT` says otherwise. The host gives up at the same
/// time; `start` allows it a little longer to say so.
const START_TIMEOUT: u64 = 120;
const START_GRACE: Duration = Duration::from_secs(10);

/// The control commands: everything but `acp` and `host`.
pub fn main(args: Vec<String>) -> ExitCode {
    let rest = args.get(1..).unwrap_or_default();
    let done = |r: Result<(), String>| r.map(|()| ExitCode::SUCCESS);
    let result = match args.first().map(String::as_str) {
        Some("start") => talk::start(rest),
        Some("send") => talk::send(rest),
        Some("wait") => talk::wait(rest),
        Some("cancel") => talk::cancel(rest),
        Some("queue") => talk::queue(rest),
        Some("log") => history::log(rest),
        Some("mode") => settings::mode(rest),
        Some("model") => settings::model(rest),
        Some("config") => settings::config(rest),
        Some("commands") => settings::commands(rest),
        Some("sessions") => settings::sessions(rest),
        Some("fork") => settings::fork(rest),
        Some("close") => settings::close(rest),
        Some("show") => show::show(rest),
        Some("notify") => notify::notify(rest),
        Some("list") => done(list(rest)),
        Some("status") => done(status(rest)),
        Some("pending") => done(pending(rest)),
        Some("approve") => done(answer(rest, "approve")),
        Some("deny") => done(answer(rest, "deny")),
        Some("watch") => done(watch(rest)),
        Some("stop") => done(stop(rest)),
        Some("doctor") => done(doctor::main(rest)),
        Some("-h" | "--help") => {
            println!("{USAGE}");
            Ok(ExitCode::SUCCESS)
        }
        Some("-V" | "--version") => {
            println!("brnr {}", env!("CARGO_PKG_VERSION"));
            Ok(ExitCode::SUCCESS)
        }
        _ => Err(USAGE.to_owned()),
    };
    match result {
        Ok(code) => code,
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
            "profile": info["profile"],
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
    let (target, json_out) = match args {
        [target] => (target, false),
        [target, flag] | [flag, target] if flag == "--json" => (target, true),
        _ => return Err(USAGE.to_owned()),
    };
    let hosts = discover();
    let (host, only) = resolve(&hosts, target)?;
    let status = host.status.as_ref().ok_or("host is not answering")?;
    if json_out {
        println!("{}", serde_json::to_string_pretty(status).unwrap());
        return Ok(());
    }
    print!("{}", describe_status(status, only.as_deref(), target));
    Ok(())
}

/// The status report as a summary: the host, then each session's state.
fn describe_status(st: &Value, only: Option<&str>, target: &str) -> String {
    let s = |v: &Value| v.as_str().unwrap_or("?").to_owned();
    let mut out = format!("host {}", s(&st["id"]));
    if let Some(name) = st["name"].as_str() {
        out.push_str(&format!(" ({name})"));
    }
    let mut agent = agent_name(&st["agent"]);
    // What the agent says it is, in initialize: the npm package and version
    // of an adapter.
    if let (Some(name), Some(version)) =
        (st["agent_info"]["name"].as_str(), st["agent_info"]["version"].as_str())
    {
        agent.push_str(&format!(" ({name} {version})"));
    }
    out.push_str(&format!(
        ": {agent}, answered by the {}, up {}\n",
        s(&st["owner"]),
        duration(st["uptime_seconds"].as_u64().unwrap_or(0))
    ));
    out.push_str(&format!("cwd {}\n", s(&st["cwd"])));
    if st["stopping"] == true {
        out.push_str("stopping\n");
    } else if st["stop_when_idle"] == true {
        out.push_str("stops when idle\n");
    }
    let sessions: Vec<&Value> = st["sessions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|x| only.is_none_or(|id| x["session_id"] == id))
        .collect();
    if sessions.is_empty() {
        out.push_str("no session yet\n");
    }
    for x in sessions {
        out.push_str(&format!("\nsession {}", s(&x["session_id"])));
        if let Some(title) = x["title"].as_str() {
            out.push_str(&format!(": {title}"));
        }
        out.push('\n');
        let settings: Vec<String> = [("mode", &x["mode"]), ("model", &x["model"])]
            .iter()
            .filter_map(|(k, v)| v.as_str().map(|v| format!("{k} {v}")))
            .collect();
        if !settings.is_empty() {
            out.push_str(&format!("  {}\n", settings.join(", ")));
        }
        match x["turn_seconds"].as_u64() {
            Some(secs) if x["busy"] == true => {
                out.push_str(&format!("  working for {}\n", duration(secs)));
            }
            _ => out.push_str("  idle\n"),
        }
        for tool in x["tools"].as_array().into_iter().flatten() {
            out.push_str(&format!(
                "  tool: {} ({}, {})\n",
                s(&tool["title"]),
                s(&tool["kind"]),
                s(&tool["status"])
            ));
        }
        if x["plan"].is_array() {
            let plan = render::plan(&x["plan"]);
            for line in plan.lines() {
                out.push_str(&format!("  {line}\n"));
            }
        }
        if let Some(n) = x["pending"].as_u64().filter(|&n| n > 0) {
            out.push_str(&format!("  {n} permission request(s) waiting: brnr show {target}\n"));
        }
        let (held, context) = (x["held"].as_u64().unwrap_or(0), x["context"].as_u64().unwrap_or(0));
        if held + context > 0 {
            out.push_str(&format!(
                "  held: {held} message(s), {context} context (brnr queue {target})\n"
            ));
        }
        let usage = &x["usage"];
        if let (Some(used), Some(size)) = (usage["used"].as_u64(), usage["size"].as_u64()) {
            let mut line = format!("  context window: {} of {} tokens", tokens(used), tokens(size));
            if let (Some(amount), Some(currency)) =
                (usage["cost"]["amount"].as_f64(), usage["cost"]["currency"].as_str())
            {
                line.push_str(&format!(", cost {amount:.2} {currency}"));
            }
            out.push_str(&format!("{line}\n"));
        }
        if let Some(last) = x["last_message"].as_str() {
            let last = last.trim().replace('\n', " ");
            let short: String = last.chars().take(200).collect();
            let more = if last.chars().count() > 200 { "…" } else { "" };
            out.push_str(&format!("  last message: {short}{more}\n"));
        }
    }
    out
}

/// `2h05m`, `3m12s`, `40s`.
fn duration(secs: u64) -> String {
    match secs {
        s if s >= 3600 => format!("{}h{:02}m", s / 3600, s % 3600 / 60),
        s if s >= 60 => format!("{}m{:02}s", s / 60, s % 60),
        s => format!("{s}s"),
    }
}

/// `950`, `12.3k`, `1.2M`.
fn tokens(n: u64) -> String {
    match n {
        n if n >= 1_000_000 => format!("{:.1}M", n as f64 / 1e6),
        n if n >= 1_000 => format!("{:.1}k", n as f64 / 1e3),
        n => n.to_string(),
    }
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

/// Prints the host's events until it exits, as `brnr log` shows them. A
/// target that names a session shows only that session's events (plus the
/// host's own, such as the agent exiting).
fn watch(args: &[String]) -> Result<(), String> {
    let mut target = None;
    let mut events = default_events();
    let mut json_out = false;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--events" => events = events_arg(it.next(), &default_events())?,
            "--json" => json_out = true,
            flag if flag.starts_with("--") => return Err(format!("unknown option: {flag}")),
            _ if target.is_none() => target = Some(arg.clone()),
            _ => return Err(USAGE.to_owned()),
        }
    }
    let target = target.ok_or(USAGE)?;
    // `exited` is how watch tells the host ending from the host cutting it
    // off; it is asked for even when not shown.
    let show_exited = events.iter().any(|e| e == "exited");
    if !show_exited {
        events.push("exited".to_owned());
    }
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
    let options = render::Options { session: true, time: true };
    for line in lines.map_while(Result::ok) {
        let Ok(event) = serde_json::from_str::<Value>(&line) else { continue };
        let exited = event["event"] == "exited";
        if let (Some(only), Some(session)) = (&only_session, event["session"].as_str())
            && only != session
        {
            continue;
        }
        if !exited || show_exited {
            let text = if json_out {
                Some(line)
            } else {
                render::event(&event, &options)
            };
            if let Some(text) = text
                && writeln!(out, "{text}").and_then(|()| out.flush()).is_err()
            {
                return Ok(());
            }
        }
        if exited {
            return Ok(());
        }
    }
    Err("the host closed the connection (a watcher that falls behind is disconnected)".into())
}

// ---- plumbing ------------------------------------------------------------

/// The value of `--events` (for `watch`, `log` and `notify`): event names,
/// `default` for the command's own `default` (what it does without
/// `--events`), `all` for every event.
fn events_arg(list: Option<&String>, default: &[String]) -> Result<Vec<String>, String> {
    let list = list.ok_or("--events needs a list")?;
    let mut events = Vec::new();
    for name in list.split(',').map(str::trim) {
        let names = match name {
            "all" => EVENTS.iter().map(|e| e.to_string()).collect(),
            "default" => default.to_vec(),
            _ if EVENTS.contains(&name) => vec![name.to_owned()],
            _ => {
                let names = EVENTS.join(", ");
                return Err(format!("unknown event {name:?} (events: default, all, {names})"));
            }
        };
        for name in names {
            if !events.contains(&name) {
                events.push(name);
            }
        }
    }
    Ok(events)
}

/// What `watch` and `log` show without `--events`: every event but the
/// quiet ones (ACP messages and thoughts).
fn default_events() -> Vec<String> {
    EVENTS.iter().filter(|e| !QUIET.contains(e)).map(|e| e.to_string()).collect()
}

/// The host `target` names, and the ACP session if it named one.
fn resolve<'a>(hosts: &'a [Host], target: &str) -> Result<(&'a Host, Option<String>), String> {
    if let Some(host) = hosts.iter().find(|h| h.id() == target) {
        return Ok((host, None));
    }
    // brnr start refuses a name in use, but editors' hosts can share one.
    let named: Vec<&Host> = hosts.iter().filter(|h| h.meta["name"] == target).collect();
    match named[..] {
        [host] => return Ok((host, None)),
        [] => {}
        _ => {
            let ids: Vec<&str> = named.iter().map(|h| h.id()).collect();
            return Err(format!("several hosts are named {target}: {}", ids.join(", ")));
        }
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
    request_timeout(host, req, Duration::from_secs(5))
}

/// A request and its answer, waiting up to `timeout` for it.
fn request_timeout(host: &Host, req: &Value, timeout: Duration) -> io::Result<Value> {
    let mut conn = connect(host)?;
    conn.set_read_timeout(Some(timeout))?;
    writeln!(conn, "{req}")?;
    let mut line = String::new();
    BufReader::new(conn).read_line(&mut line).map_err(|e| match e.kind() {
        ErrorKind::WouldBlock | ErrorKind::TimedOut => io::Error::other("not answering"),
        _ => e,
    })?;
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
