//! The control commands: start headless sessions and talk to running ones
//! over their processes' control sockets. Most are `brnr <group> <verb>`,
//! grouped by what they act on (ADR 63). `brnr --help` lists them; the
//! modules have the details:
//!
//! - talk.rs: `start`, `prompt send`, `prompt cancel`, `event wait`, `queue`
//! - history.rs: `event log`
//! - settings.rs: `mode`, `config`, `model`, `prompt commands`, `sessions`,
//!   `session fork`, `session close`
//! - show.rs: `permission show`; notify.rs: `event notify`; doctor.rs:
//!   `doctor`; skill.rs: `skill`
//! - here: `process list`, `process stop`, `list`, `session status`,
//!   `permission requests`, `approve`, `deny`, `event watch`
//!
//! A `<session>` is a session's id, as the agent gave it. A `<pid>` is a
//! brnr process, which runs one agent for one or more sessions. Commands that
//! act on a session need it running and say so when it isn't. Which process
//! serves a session is what the session's lock says (ADR 3, see lock.rs),
//! whether that process answers or not.
//!
//! `--json` prints what the text says, as one JSON value (one event per line
//! for `event log`, `event watch` and `start --foreground`).

use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::{self, BufRead, BufReader, ErrorKind, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use serde_json::{Value, json};

use brnr::host::{EVENTS, QUIET, alive};
use brnr::{lock, paths, render};

/// `println!` and `print!` for what the commands print: through
/// [`render::clean`], since so much of it is the agent's (titles, messages,
/// options), and quietly ending brnr when the reader goes away (`brnr list |
/// head -1`), as a filter does, rather than panicking.
macro_rules! outln {
    ($($arg:tt)*) => { $crate::ctl::out(&format!("{}\n", format_args!($($arg)*))) };
}
macro_rules! out {
    ($($arg:tt)*) => { $crate::ctl::out(&format!($($arg)*)) };
}
/// `eprintln!` for messages with the agent's words in them.
macro_rules! errln {
    ($($arg:tt)*) => { eprintln!("{}", brnr::render::clean(&format!($($arg)*))) };
}

mod doctor;
mod history;
mod notify;
mod settings;
mod show;
mod skill;
mod talk;

/// The command groups (ADR 63): `brnr <group> <verb>`, a group being what
/// its commands act on.
const GROUPS: &[&str] = &["process", "session", "prompt", "queue", "permission", "event"];

const USAGE: &str = "usage:
  brnr acp [--profile <p>] [--strict] [-- <agent> [args...]]
             what an editor runs as its ACP agent

  brnr start [--profile <p>] [--cwd <dir>] [--prompt <text> | -] [--file <path>]...
             [--image <path>]... [--mode <m>] [--model <m>] [--set <option>=<value>]...
             [--stop-when-idle <s>] [--auth <method>] [--resume <session> [--take-over]]
             [--strict] [--wait [--timeout <s>] | --foreground [--quiet]] [--json]
             [-- <agent> [args...]]
             a headless session, in the background (or the foreground)

process
  brnr process list [--json]
  brnr process stop <pid>

session
  brnr session status <session> [--json]
  brnr session fork <session> [--json]
  brnr session close <session>
  brnr list [--inactive | --all] [--json]
  brnr sessions [--profile <p>] [--cwd <dir>] [--json] [-- <agent> [args...]]

prompt
  brnr prompt send <session> [--steer | --interrupt | --context [--replace]]
                   [--file <path>]... [--image <path>]... [--wait [--timeout <s>]] [--json]
                   (<text>... | -)
  brnr prompt cancel <session> [--keep-held] [--json]
  brnr prompt commands <session> [--json]

queue
  brnr queue list <session> [--clear] [--clear-context] [--json]
  brnr queue drop <session> <message> [--json]

permission
  brnr permission requests [<session>] [--json]
  brnr permission show <session> <request> [--json]
  brnr approve <session> <request> [--option <id>] [--json]
  brnr deny <session> <request> [--option <id>] [--json]

event
  brnr event log <session> [--last <n>] [--follow] [--events <default|all|event>,...] [--json]
  brnr event watch (<session> | --pid <pid>) [--events <default|all|event>,...] [--json]
  brnr event notify (<session> | --pid <pid> | --stdin) [--events <default|all|event>,...]
                    -- <command> [args...]
  brnr event wait <session> [--for idle|turn|permission|exit] [--timeout <s>] [--json]

settings
  brnr mode <session> [<mode>] [--json]
  brnr model <session> [<model>] [--json]
  brnr config <session> [<option>=<value>...] [--json]

brnr
  brnr doctor [--fix | --report] [--json]
  brnr skill [<reference> | install [--dir <dir>]...]
             the skill for agents that use brnr: print it, or install it
  brnr --version

brnr <group> --help lists a group's commands. <session> is a session's id, as brnr list shows
it. <request> is a pending approval's handle, as brnr permission requests shows it. <pid> is a
brnr process, which runs one agent for one or more sessions, as brnr process list shows them.
--json prints the same data as the text: one JSON value, or one event per line for event log,
event watch and start --foreground. --strict is stable ACP only: no --steer into a running
turn, no session fork.";

/// How long a start may take until it commits, in seconds, unless
/// `BRNR_START_TIMEOUT` says otherwise. It goes in the start request: the
/// process fails the start when it passes, and `start` gives up a little
/// later, for a process stuck too badly to say so.
const START_TIMEOUT: u64 = 120;
const START_GRACE: Duration = Duration::from_secs(10);

/// The control commands: everything but `acp` and `host`. A group's
/// commands are `brnr <group> <verb>`; the others, one word.
pub fn main(args: Vec<String>) -> ExitCode {
    let cmd = args.first().map_or("", String::as_str);
    let grouped = GROUPS.contains(&cmd);
    let verb = if grouped { args.get(1).map(String::as_str) } else { None };
    let rest = args.get(if grouped { 2 } else { 1 }..).unwrap_or_default();
    let done = |r: Result<(), String>| r.map(|()| ExitCode::SUCCESS);
    let result = match (cmd, verb) {
        ("process", Some("list")) => done(ps(rest)),
        ("process", Some("stop")) => done(stop(rest)),
        ("session", Some("status")) => done(status(rest)),
        ("session", Some("fork")) => settings::fork(rest),
        ("session", Some("close")) => settings::close(rest),
        ("prompt", Some("send")) => talk::send(rest),
        ("prompt", Some("cancel")) => talk::cancel(rest),
        ("prompt", Some("commands")) => settings::commands(rest),
        ("queue", Some("list")) => talk::queue_list(rest),
        ("queue", Some("drop")) => talk::queue_drop(rest),
        ("permission", Some("requests")) => done(pending(rest)),
        ("permission", Some("show")) => show::show(rest),
        ("event", Some("log")) => history::log(rest),
        ("event", Some("watch")) => done(watch(rest)),
        ("event", Some("notify")) => notify::notify(rest),
        ("event", Some("wait")) => talk::wait(rest),
        (group, Some("-h" | "--help")) if grouped => {
            outln!("{}", usage_of(group, None));
            Ok(ExitCode::SUCCESS)
        }
        (group, Some(verb)) if grouped => {
            Err(format!("unknown command: {group} {verb}\n{}", usage_of(group, None)))
        }
        (_, None) if grouped => Err(USAGE.to_owned()),
        ("start", _) => talk::start(rest),
        ("mode", _) => settings::mode(rest),
        ("model", _) => settings::model(rest),
        ("config", _) => settings::config(rest),
        ("sessions", _) => settings::sessions(rest),
        ("list", _) => done(list(rest)),
        ("approve", _) => done(answer(rest, "approve")),
        ("deny", _) => done(answer(rest, "deny")),
        ("doctor", _) => done(doctor::main(rest)),
        ("skill", _) => done(skill::skill(rest)),
        ("-h" | "--help", _) => {
            outln!("{USAGE}");
            Ok(ExitCode::SUCCESS)
        }
        ("-V" | "--version", _) => {
            outln!("brnr {}", env!("CARGO_PKG_VERSION"));
            Ok(ExitCode::SUCCESS)
        }
        ("", _) => Err(USAGE.to_owned()),
        (cmd, _) => Err(format!("unknown command: {cmd} (brnr --help for every command)")),
    };
    match result {
        Ok(code) => code,
        Err(msg) if msg == USAGE => {
            eprintln!("{}", usage_of(cmd, verb));
            ExitCode::FAILURE
        }
        Err(msg) => {
            errln!("brnr: {msg}");
            ExitCode::FAILURE
        }
    }
}

/// The command `args` are, for a bug report: a group's with its verb.
pub fn command(args: &[String]) -> String {
    let n = match args.first() {
        Some(group) if GROUPS.contains(&group.as_str()) => 2,
        _ => 1,
    };
    args.iter().take(n).cloned().collect::<Vec<_>>().join(" ")
}

/// What [`outln!`] and [`out!`] print with.
fn out(text: &str) {
    let mut stdout = io::stdout().lock();
    match stdout.write_all(render::clean(text).as_bytes()).and_then(|()| stdout.flush()) {
        Ok(()) => {}
        Err(e) if e.kind() == ErrorKind::BrokenPipe => std::process::exit(0),
        Err(e) => panic!("writing to stdout: {e}"),
    }
}

/// The usage of one command (its lines in [`USAGE`]): of `cmd`, or of
/// group `cmd`'s `verb`, or of every command in the group without one (or
/// with one it doesn't have). All of it for a command there is none of.
fn usage_of(cmd: &str, verb: Option<&str>) -> String {
    let in_group = |verb: Option<&str>| {
        let mut lines = Vec::new();
        let mut ours = false;
        for line in USAGE.lines() {
            let rest = line.trim_start();
            if rest.starts_with("brnr ") {
                let mut words = rest.split_whitespace().skip(1);
                ours = words.next() == Some(cmd) && (verb.is_none() || words.next() == verb);
            } else if !line.starts_with("    ") {
                ours = false;
            }
            if ours {
                lines.push(line);
            }
        }
        lines
    };
    let mut lines = in_group(verb);
    if lines.is_empty() && verb.is_some() {
        lines = in_group(None);
    }
    if lines.is_empty() {
        return USAGE.to_owned();
    }
    format!("usage:\n{}\n(brnr --help for every command)", lines.join("\n"))
}

/// A running brnr process: its metadata file and, if it answered, its live
/// status.
struct Host {
    meta: Value,
    status: Option<Value>,
}

impl Host {
    fn info(&self) -> &Value {
        self.status.as_ref().unwrap_or(&self.meta)
    }

    /// Its pid, which names its socket and metadata.
    fn id(&self) -> &str {
        self.meta["id"].as_str().unwrap_or("?")
    }

    fn sessions(&self) -> &[Value] {
        self.status.as_ref().and_then(|s| s["sessions"].as_array()).map_or(&[], Vec::as_slice)
    }

    /// The sessions whose locks it holds, as `locks` has them: what it
    /// serves, asked of nobody.
    fn held(&self, locks: &[lock::Entry]) -> Vec<String> {
        let pid = self.id().parse().ok();
        let held = locks.iter().filter(|e| e.pid.is_some() && e.pid == pid);
        held.filter_map(|e| e.session.clone()).collect()
    }
}

// ---- naming sessions and processes ---------------------------------------

/// What a `<session>` argument names.
enum Found<'a> {
    /// A running session: its process and id.
    Running(&'a Host, String),
    /// One that has ended, as `inactive_sessions` has it.
    Inactive(Value),
}

/// The session with id `arg`, running or not: in the process holding its
/// lock, or one that serves it shared, without the lock (ADR 3).
fn find_session<'a>(hosts: &'a [Host], arg: &str) -> Result<Found<'a>, String> {
    let serves = |host: &Host| host.sessions().iter().any(|s| s["session_id"] == arg);
    if let Some(pid) = lock::holder(arg) {
        return match hosts.iter().find(|h| h.id() == pid.to_string()) {
            Some(host) if serves(host) => Ok(Found::Running(host, arg.to_owned())),
            Some(host) if host.status.is_none() => {
                Err(format!("{arg} is running in process {pid}, which is not answering"))
            }
            _ => Err(format!("{arg} is opening in process {pid}")),
        };
    }
    if let Some(host) = hosts.iter().find(|h| serves(h)) {
        return Ok(Found::Running(host, arg.to_owned()));
    }
    if let Some(p) = inactive_sessions(hosts).into_iter().find(|p| p["session_id"] == arg) {
        return Ok(Found::Inactive(p));
    }
    Err(format!("no session {arg} (see brnr list --all)"))
}

/// The running session `arg` names, for what needs one running.
fn running_session<'a>(hosts: &'a [Host], arg: &str) -> Result<(&'a Host, String), String> {
    match find_session(hosts, arg)? {
        Found::Running(host, id) => Ok((host, id)),
        Found::Inactive(_) => Err(format!("{arg} isn't running: brnr start --resume {arg}")),
    }
}

/// The process with pid `pid`.
fn process<'a>(hosts: &'a [Host], pid: &str) -> Result<&'a Host, String> {
    hosts
        .iter()
        .find(|h| h.id() == pid)
        .ok_or(format!("no brnr process {pid} (see brnr process list)"))
}

/// `<session>` or `--pid <pid>`, for `watch` and `notify`: the process, and
/// the session if one was named.
fn session_or_pid<'a>(
    hosts: &'a [Host],
    session: Option<&str>,
    pid: Option<&str>,
) -> Result<(&'a Host, Option<String>), String> {
    match (session, pid) {
        (Some(session), None) => running_session(hosts, session).map(|(h, id)| (h, Some(id))),
        (None, Some(pid)) => process(hosts, pid).map(|h| (h, None)),
        _ => Err("give a <session> or --pid <pid>".into()),
    }
}

// ---- processes -----------------------------------------------------------

fn ps(args: &[String]) -> Result<(), String> {
    let json_out = match args {
        [] => false,
        [flag] if flag == "--json" => true,
        _ => return Err(USAGE.to_owned()),
    };
    let hosts = discover()?;
    let locks = lock::all();
    let rows: Vec<Value> = hosts
        .iter()
        .map(|host| {
            let info = host.info();
            // One that doesn't answer serves what it holds the locks of.
            let sessions: Vec<Value> = match host.status {
                Some(_) => host.sessions().iter().map(|s| s["session_id"].clone()).collect(),
                None => host.held(&locks).into_iter().map(Value::from).collect(),
            };
            json!({
                "pid": host.id().parse::<u64>().ok(),
                "owner": if host.status.is_some() { info["owner"].clone() } else { json!("unreachable") },
                "agent": agent_name(&info["agent"]),
                "sessions": sessions,
                "uptime_seconds": info["uptime_seconds"],
                "cwd": info["cwd"],
            })
        })
        .collect();
    if json_out {
        return print_json(&json!(rows));
    }
    if rows.is_empty() {
        outln!("no brnr processes");
        return Ok(());
    }
    // Each session has a cwd of its own (brnr list); the process's is only
    // where the editor happened to start it, so the table leaves it to
    // --json.
    let mut table = vec![["PID", "OWNER", "AGENT", "SESSIONS", "UP"].map(String::from)];
    for r in &rows {
        let sessions: Vec<String> =
            r["sessions"].as_array().into_iter().flatten().map(text).collect();
        table.push([
            r["pid"].to_string(),
            text(&r["owner"]),
            text(&r["agent"]),
            if sessions.is_empty() { "-".to_owned() } else { sessions.join(", ") },
            r["uptime_seconds"].as_u64().map_or("?".to_owned(), duration),
        ]);
    }
    print_table(table);
    Ok(())
}

fn stop(args: &[String]) -> Result<(), String> {
    let [pid] = args else { return Err(USAGE.to_owned()) };
    let hosts = discover()?;
    let host = process(&hosts, pid)?;
    let response = call(host, &json!({ "cmd": "stop" }))?;
    outln!("{}", response["status"].as_str().unwrap_or("?"));
    Ok(())
}

// ---- sessions ------------------------------------------------------------

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
    let hosts = discover()?;
    let mut rows = if active { running_rows(&hosts, &lock::all()) } else { Vec::new() };
    if inactive {
        for p in inactive_sessions(&hosts) {
            rows.push(json!({
                "session": p["session_id"],
                "title": null,
                "state": "inactive",
                "pid": null,
                "agent": agent_name(&p["agent"]),
                "cwd": p["cwd"],
                "last_active": p["last_active"],
            }));
        }
    }
    // Most recently active first, like brnr sessions, however many processes.
    rows.sort_by(|a, b| b["last_active"].as_str().cmp(&a["last_active"].as_str()));
    if json_out {
        return print_json(&json!(rows));
    }
    if rows.is_empty() {
        outln!("no {}sessions", if active { "running " } else { "" });
        return Ok(());
    }
    let mut table =
        vec![["SESSION", "TITLE", "STATE", "PID", "AGENT", "LAST ACTIVE", "CWD"].map(String::from)];
    for r in &rows {
        table.push([
            text(&r["session"]),
            r["title"].as_str().unwrap_or("-").to_owned(),
            text(&r["state"]),
            r["pid"].as_u64().map_or("-".to_owned(), |p| p.to_string()),
            text(&r["agent"]),
            r["last_active"].as_str().map_or("?".to_owned(), when),
            text(&r["cwd"]),
        ]);
    }
    print_table(table);
    Ok(())
}

/// The running sessions, a row each, as `list` shows them: those each
/// process that answers says it serves, and those one that doesn't answer
/// holds the locks of (ADR 3), `unreachable`. `sessions` takes its STATE and
/// PID from these too (ADR 15).
fn running_rows(hosts: &[Host], locks: &[lock::Entry]) -> Vec<Value> {
    let mut rows = Vec::new();
    for host in hosts {
        for s in host.sessions() {
            rows.push(json!({
                "session": s["session_id"],
                "title": s["title"],
                "state": s["state"],
                "pid": host.id().parse::<u64>().ok(),
                "agent": agent_name(&host.info()["agent"]),
                "cwd": s["cwd"],
                "last_active": s["last_active"],
            }));
        }
        // One that doesn't answer: what it holds the locks of, and no more.
        if host.status.is_none() {
            for session in host.held(locks) {
                rows.push(json!({
                    "session": session,
                    "title": null,
                    "state": "unreachable",
                    "pid": host.id().parse::<u64>().ok(),
                    "agent": agent_name(&host.info()["agent"]),
                    "cwd": null,
                    "last_active": null,
                }));
            }
        }
    }
    rows
}

/// Sessions with a transcript that no running process is serving, most
/// recently active first. Only the first and last record of each events
/// file are read: the first names the cwd, the last says when and in which
/// process the session was last active. That process's log says which
/// agent it ran.
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
        .filter(|p| paths::is_events_log(p));

    // A session whose lock is held is running, whether its process answers
    // or not.
    let held: Vec<String> =
        lock::all().into_iter().filter(|e| e.pid.is_some()).filter_map(|e| e.session).collect();
    let mut started: HashMap<String, Value> = HashMap::new();
    let mut past = Vec::new();
    for file in files {
        let (Some(first), Some(last)) = (first_record(&file), last_record(&file)) else { continue };
        let Some(session) = last["session_id"].as_str() else { continue };
        if running.contains(&file) || held.iter().any(|s| s == session) {
            continue;
        }
        let host_id = last["host_id"].as_str().unwrap_or_default().to_owned();
        let start = started
            .entry(host_id.clone())
            .or_insert_with(|| first_record(&paths::host_log(&host_id)).unwrap_or_default());
        let info = &start["event"]["info"];
        past.push(json!({
            "session_id": session,
            "cwd": first["event"]["cwd"],
            "last_active": last["ts"],
            "profile": info["profile"],
            "agent": info["agent"],
            "log": file.to_string_lossy(),
        }));
    }
    past.sort_by(|a, b| b["last_active"].as_str().cmp(&a["last_active"].as_str()));
    past
}

/// The first record in a file that brnr can read (ADR 55).
fn first_record(path: &Path) -> Option<Value> {
    let lines = BufReader::new(fs::File::open(path).ok()?).split(b'\n');
    lines.map_while(Result::ok).find_map(|line| as_record(&line))
}

/// The last record in a file that brnr can read, read backwards so a long
/// transcript costs no more than its last records. A line brnr can't read
/// is passed over (ADR 55): one a process that died mid-write cut short,
/// at the end or, once another process has taken the file, before what it
/// wrote. A line cut short is never read as a record: an object ends only
/// at its last byte.
fn last_record(path: &Path) -> Option<Value> {
    use std::io::{Seek, SeekFrom};
    let mut file = fs::File::open(path).ok()?;
    let mut end = file.metadata().ok()?.len();
    // From `end` to the start of the lines already tried.
    let mut tail: Vec<u8> = Vec::new();
    loop {
        let start = end.saturating_sub(64 * 1024);
        let mut chunk = vec![0; (end - start) as usize];
        file.seek(SeekFrom::Start(start)).ok()?;
        file.read_exact(&mut chunk).ok()?;
        chunk.extend_from_slice(&tail);
        tail = chunk;
        // Its lines, last first; the first is whole only at the file's start.
        while let Some(i) = tail.iter().rposition(|&b| b == b'\n') {
            if let Some(found) = as_record(&tail[i + 1..]) {
                return Some(found);
            }
            tail.truncate(i);
        }
        if start == 0 {
            return as_record(&tail);
        }
        end = start;
    }
}

/// A line of a transcript as a record, if it is one.
fn as_record(line: &[u8]) -> Option<Value> {
    serde_json::from_slice(line).ok().filter(Value::is_object)
}

fn agent_name(argv: &Value) -> String {
    let Some(program) = argv.as_array().and_then(|a| a.first()).and_then(Value::as_str) else {
        return "?".to_owned();
    };
    Path::new(program).file_name().map_or(program.to_owned(), |f| f.to_string_lossy().into_owned())
}

fn status(args: &[String]) -> Result<(), String> {
    let (arg, json_out) = match args {
        [arg] => (arg, false),
        [arg, flag] | [flag, arg] if flag == "--json" => (arg, true),
        _ => return Err(USAGE.to_owned()),
    };
    let hosts = discover()?;
    let (host, id) = running_session(&hosts, arg)?;
    let st = host.status.as_ref().ok_or("the process is not answering")?;
    let x = host.sessions().iter().find(|s| s["session_id"] == id.as_str()).ok_or("no session")?;
    // Processes serving it without its lock, `shared_sessions` (ADR 3): what
    // happens there isn't in its transcript.
    let shares = |s: &Value| s["session_id"] == id.as_str() && s["shared"] == true;
    let shared_by: Vec<u64> = hosts
        .iter()
        .filter(|h| h.sessions().iter().any(shares))
        .filter_map(|h| h.id().parse().ok())
        .collect();
    let mut agent = json!({ "program": agent_name(&st["agent"]) });
    // What the agent says it is, in initialize: the npm package and version
    // of an adapter.
    if let (Some(name), Some(version)) =
        (st["agent_info"]["name"].as_str(), st["agent_info"]["version"].as_str())
    {
        agent["name"] = json!(name);
        agent["version"] = json!(version);
    }
    let report = json!({
        "session": id,
        "title": x["title"],
        "pid": host.id().parse::<u64>().ok(),
        "agent": agent,
        "owner": st["owner"],
        "held_by": lock::holder(&id),
        "shared_by": shared_by,
        // An editor's session served without its lock (ADR 50).
        "lock_error": x["lock_error"],
        "cwd": x["cwd"],
        "state": x["state"],
        "turn_seconds": x["turn_seconds"],
        "mode": x["mode"],
        "model": x["model"],
        "tools": x["tools"],
        "plan": x["plan"],
        "pending": x["pending"],
        "held": x["held"],
        "context": x["context"],
        "usage": x["usage"],
        "last_message": x["last_message"],
        "last_active": x["last_active"],
        "stop_when_idle": st["stop_when_idle"],
        "stopping": st["stopping"],
        "uptime_seconds": st["uptime_seconds"],
    });
    if json_out {
        return print_json(&report);
    }
    out!("{}", describe_status(&report, arg));
    Ok(())
}

/// The status report as text, line by line.
fn describe_status(x: &Value, arg: &str) -> String {
    let mut out = format!("session {}", text(&x["session"]));
    if let Some(title) = x["title"].as_str() {
        out.push_str(&format!(": {title}"));
    }
    out.push('\n');
    let a = &x["agent"];
    let mut agent = text(&a["program"]);
    if let (Some(name), Some(version)) = (a["name"].as_str(), a["version"].as_str()) {
        agent.push_str(&format!(" ({name} {version})"));
    }
    out.push_str(&format!(
        "process {}: {agent}, {}, up {}\n",
        x["pid"],
        text(&x["owner"]),
        duration(x["uptime_seconds"].as_u64().unwrap_or(0))
    ));
    let shared_by: Vec<String> =
        x["shared_by"].as_array().into_iter().flatten().map(Value::to_string).collect();
    if !shared_by.is_empty() {
        let held = match x["held_by"].as_u64() {
            Some(pid) => format!("process {pid}'s"),
            None => "no process's (none holds it now)".to_owned(),
        };
        out.push_str(&format!(
            "shared by process {}: the transcript is {held}, and has none of what happens there\n",
            shared_by.join(", ")
        ));
    }
    if x["lock_error"].is_string() {
        out.push_str(&format!(
            "not locked: {}; nothing stops another process serving it too (brnr doctor)\n",
            text(&x["lock_error"])
        ));
    }
    out.push_str(&format!("cwd {}\n", text(&x["cwd"])));
    if x["stopping"] == true {
        out.push_str("stopping\n");
    } else if let Some(secs) = x["stop_when_idle"].as_u64() {
        out.push_str(&format!("closes when idle for {}\n", duration(secs)));
    }
    let settings: Vec<String> = [("mode", &x["mode"]), ("model", &x["model"])]
        .iter()
        .filter_map(|(k, v)| v.as_str().map(|v| format!("{k} {v}")))
        .collect();
    if !settings.is_empty() {
        out.push_str(&format!("{}\n", settings.join(", ")));
    }
    match (x["state"].as_str(), x["turn_seconds"].as_u64()) {
        (Some("idle"), _) | (_, None) => out.push_str("idle\n"),
        (_, Some(secs)) => out.push_str(&format!("working for {}\n", duration(secs))),
    }
    for tool in x["tools"].as_array().into_iter().flatten() {
        out.push_str(&format!(
            "tool: {} ({}, {})\n",
            text(&tool["title"]),
            text(&tool["kind"]),
            text(&tool["status"])
        ));
    }
    if x["plan"].is_array() {
        out.push_str(&format!("{}\n", render::plan(&x["plan"])));
    }
    if let Some(n) = x["pending"].as_u64().filter(|&n| n > 0) {
        out.push_str(&format!("{n} approval(s) waiting: brnr permission requests {arg}\n"));
    }
    let (held, context) = (x["held"].as_u64().unwrap_or(0), x["context"].as_u64().unwrap_or(0));
    if held + context > 0 {
        out.push_str(&format!(
            "held: {held} message(s), {context} context (brnr queue list {arg})\n"
        ));
    }
    if let Some(usage) = render::usage(&x["usage"]) {
        out.push_str(&format!("context window: {usage}\n"));
    }
    if let Some(last) = x["last_message"].as_str() {
        let last = last.trim().replace('\n', " ");
        let short: String = last.chars().take(200).collect();
        let more = if last.chars().count() > 200 { "…" } else { "" };
        out.push_str(&format!("last message: {short}{more}\n"));
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

/// `2026-10-01T19:34:25.959660Z` as `2026-10-01 19:34:25Z`.
fn when(ts: &str) -> String {
    match (ts.get(..10), ts.get(11..19)) {
        (Some(day), Some(time)) => format!("{day} {time}Z"),
        _ => ts.to_owned(),
    }
}

// ---- approvals -----------------------------------------------------------

/// The approvals waiting, in every process or in one session's.
fn pending(args: &[String]) -> Result<(), String> {
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
        None => (hosts.iter().collect(), None),
        Some(arg) => {
            let (host, id) = running_session(&hosts, arg)?;
            (vec![host], Some(id))
        }
    };
    let mut rows = Vec::new();
    for host in chosen {
        let response = request(host, &json!({ "cmd": "pending" }))
            .map_err(|e| format!("process {}: {e}", host.id()))?;
        for p in response["pending"].as_array().into_iter().flatten() {
            if only.as_deref().is_some_and(|id| p["session"] != id) {
                continue;
            }
            let options: Vec<Value> = p["options"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|o| json!({ "option": o["optionId"], "kind": o["kind"] }))
                .collect();
            rows.push(json!({
                "session": p["session"],
                "request": p["request"],
                "owner": p["owner"],
                "kind": p["kind"],
                "title": p["title"],
                "options": options,
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
    let mut table =
        vec![["SESSION", "REQUEST", "OWNER", "KIND", "TITLE", "OPTIONS"].map(String::from)];
    for r in &rows {
        let options: Vec<String> = r["options"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|o| format!("{}={}", text(&o["option"]), text(&o["kind"])))
            .collect();
        table.push([
            text(&r["session"]),
            text(&r["request"]),
            text(&r["owner"]),
            text(&r["kind"]),
            text(&r["title"]),
            options.join(" "),
        ]);
    }
    print_table(table);
    Ok(())
}

fn answer(args: &[String], cmd: &str) -> Result<(), String> {
    let mut positional = Vec::new();
    let mut option = None;
    let mut json_out = false;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--option" => option = Some(it.next().ok_or("--option needs an id")?.clone()),
            "--json" => json_out = true,
            flag if flag.starts_with("--") => return Err(format!("unknown option: {flag}")),
            _ => positional.push(arg.clone()),
        }
    }
    let [arg, request_id] = &positional[..] else { return Err(USAGE.to_owned()) };
    let hosts = discover()?;
    let (host, session) = running_session(&hosts, arg)?;
    let req = json!({ "cmd": cmd, "session": session, "request": request_id, "option": option });
    let response = call(host, &req)?;
    let outcome = &response["outcome"];
    if json_out {
        return print_json(
            &json!({ "session": session, "request": request_id, "outcome": outcome }),
        );
    }
    let what = outcome["optionId"].as_str().or(outcome["outcome"].as_str()).unwrap_or("?");
    outln!("{request_id} {what}");
    Ok(())
}

// ---- events --------------------------------------------------------------

/// Prints a session's events (`<session>`), or a process's (`--pid`), until
/// the process exits (or the session closes), as `brnr event log` shows them. A
/// session's are its own plus the process's (the agent exiting).
fn watch(args: &[String]) -> Result<(), String> {
    let (mut session, mut pid) = (None, None);
    let mut events = default_events();
    let mut json_out = false;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--events" => events = events_arg(it.next(), &default_events())?,
            "--pid" => pid = Some(it.next().ok_or("--pid needs a pid")?.clone()),
            "--json" => json_out = true,
            flag if flag.starts_with("--") => return Err(format!("unknown option: {flag}")),
            _ if session.is_none() => session = Some(arg.clone()),
            _ => return Err(USAGE.to_owned()),
        }
    }
    let hosts = discover()?;
    let (host, only_session) = session_or_pid(&hosts, session.as_deref(), pid.as_deref())?;
    // `exited` (and for a session, `session_closed`) is how watch tells the
    // end from the process cutting the connection; asked for even when not
    // shown.
    let shown = events.clone();
    let ends: &[&str] =
        if only_session.is_some() { &["exited", "session_closed"] } else { &["exited"] };
    for end in ends {
        if !events.iter().any(|e| e == end) {
            events.push(end.to_string());
        }
    }
    let mut conn = connect(host).map_err(|e| format!("process {}: {e}", host.id()))?;
    writeln!(conn, "{}", json!({ "cmd": "subscribe", "events": events }))
        .map_err(|e| e.to_string())?;
    let mut lines = BufReader::new(conn).lines();
    let first: Value = lines
        .next()
        .and_then(Result::ok)
        .and_then(|l| serde_json::from_str(&l).ok())
        .ok_or("no answer from the process")?;
    if first["ok"].as_bool() != Some(true) {
        return Err(first["error"].as_str().unwrap_or("subscribe failed").to_owned());
    }
    let mut out = io::stdout().lock();
    let options = render::Options { session: only_session.is_none(), time: true };
    for line in lines.map_while(Result::ok) {
        let Ok(event) = serde_json::from_str::<Value>(&line) else { continue };
        let name = event["event"].as_str().unwrap_or_default();
        if let (Some(only), Some(session)) = (&only_session, event["session"].as_str())
            && only != session
        {
            continue;
        }
        let end = ends.contains(&name);
        if shown.iter().any(|e| e == name) {
            let text = if json_out { Some(line) } else { render::event(&event, &options) };
            if let Some(text) = text
                && writeln!(out, "{}", render::clean(&text)).and_then(|()| out.flush()).is_err()
            {
                return Ok(());
            }
        }
        if end {
            return Ok(());
        }
    }
    Err("the process closed the connection (a watcher that falls behind is disconnected)".into())
}

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
/// quiet ones (`QUIET`: ACP messages, thoughts, usage, tool progress).
fn default_events() -> Vec<String> {
    EVENTS.iter().filter(|e| !QUIET.contains(e)).map(|e| e.to_string()).collect()
}

// ---- plumbing ------------------------------------------------------------

/// Every process with a metadata file, asking each for its live status.
/// Files left by a process that is gone are removed, even when its pid is
/// another process's now (see `gone`). The runtime directory must be
/// private, as the processes themselves require: anyone who could write to
/// it could list a process of their own, and be sent what brnr sends.
fn discover() -> Result<Vec<Host>, String> {
    let dir = paths::runtime_dir();
    match paths::check_private(&dir) {
        Ok(()) => {}
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("{}: {e}", dir.display())),
    }
    let Ok(entries) = fs::read_dir(&dir) else { return Ok(Vec::new()) };
    let mut hosts = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        let Some(meta) = read_meta(&path) else { continue };
        let mut host = Host { meta, status: None };
        match probe(&path, &host) {
            Probe::Answered(status) => host.status = Some(status),
            Probe::Silent(_) => {}
            Probe::Gone => {
                let _ = fs::remove_file(&path);
                let _ = fs::remove_file(path.with_extension("sock"));
                continue;
            }
        }
        hosts.push(host);
    }
    hosts.sort_by(|a, b| a.meta["started"].as_str().cmp(&b.meta["started"].as_str()));
    Ok(hosts)
}

/// What a process whose metadata is at `path` turned out to be, asked for
/// its status.
enum Probe {
    /// Running and answering: its status.
    Answered(Value),
    /// Running but not answering (ADR 3): why.
    Silent(String),
    /// Gone, whatever process has its pid now.
    Gone,
}

/// Asks the process whose metadata is at `path` for its status, and says
/// what it is.
fn probe(path: &Path, host: &Host) -> Probe {
    let pid = host.meta["host_pid"].as_i64().unwrap_or(0);
    let socket = path.with_extension("sock");
    match request(host, &json!({ "cmd": "status" })) {
        Ok(status) => Probe::Answered(status),
        Err(e) if gone(pid, &socket, path, &e) => Probe::Gone,
        // Running, but accepting nothing: its backlog is full (`made`).
        Err(e) if refused(&e) => Probe::Silent("not answering".into()),
        Err(e) => Probe::Silent(e.to_string()),
    }
}

/// How long a refused connection waits to be tried again: a process that
/// is starting binds its socket a moment before it listens on it.
const REFUSED_AGAIN: Duration = Duration::from_millis(20);

/// Whether the brnr process with `pid` that made `file` in the runtime
/// directory (its metadata, or its socket) is gone, a connection to its
/// `socket` having failed with `err`.
///
/// A brnr process binds its socket and listens on it before it writes
/// anything else there, and listens until it removes it as it ends. While
/// it lives, connecting succeeds, whether it answers or not (SIGSTOPped
/// too: the kernel queues the connection). A socket nobody listens on
/// refuses, as one that isn't there does: the process that made it is
/// gone, even when another process has its pid now (a reboot, or pids
/// wrapping around). A pid that isn't the user's isn't brnr's (`alive`).
fn gone(pid: i64, socket: &Path, file: &Path, err: &io::Error) -> bool {
    if !alive(pid) {
        return true;
    }
    if !refused(err) {
        return false;
    }
    std::thread::sleep(REFUSED_AGAIN);
    match UnixStream::connect(socket) {
        Err(e) if refused(&e) => !made(pid, file),
        _ => false,
    }
}

/// Whether a connection failed because nobody listens: refused, or no
/// socket there.
fn refused(err: &io::Error) -> bool {
    matches!(err.kind(), ErrorKind::ConnectionRefused | ErrorKind::NotFound)
}

/// Whether the process with `pid` now may be the one that made `file`,
/// though its socket refuses: macOS also refuses a connection to a socket
/// whose backlog is full, as a stopped process's fills up with every
/// `brnr list` that waited on it. One that started after `file` was
/// written is another process. What can't be told counts as "may be":
/// nothing is removed on a guess (P4).
#[cfg(target_vendor = "apple")]
fn made(pid: i64, file: &Path) -> bool {
    use std::time::UNIX_EPOCH;
    let Ok(written) = fs::metadata(file).and_then(|m| m.modified()) else { return true };
    // SAFETY: proc_bsdinfo is plain data, for which all zeros is a valid
    // value.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = size_of::<libc::proc_bsdinfo>() as libc::c_int;
    let buffer = (&raw mut info).cast();
    // SAFETY: buffer points at info, and size is its size, as proc_pidinfo
    // fills it.
    let got =
        unsafe { libc::proc_pidinfo(pid as libc::c_int, libc::PROC_PIDTBSDINFO, 0, buffer, size) };
    if got != size {
        return true;
    }
    let started = UNIX_EPOCH
        + Duration::from_secs(info.pbi_start_tvsec)
        + Duration::from_micros(info.pbi_start_tvusec);
    started <= written
}

/// Linux refuses a connection only when nobody listens: a full backlog
/// makes it wait.
#[cfg(not(target_vendor = "apple"))]
fn made(_pid: i64, _file: &Path) -> bool {
    false
}

/// A process's metadata file, if it is one: the socket it names must be the
/// one next to it in the runtime directory.
fn read_meta(path: &Path) -> Option<Value> {
    let meta: Value = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    let socket = meta["socket"].as_str().map(Path::new);
    (socket == Some(&path.with_extension("sock"))).then_some(meta)
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
    let response = request(host, req).map_err(|e| format!("process {}: {e}", host.id()))?;
    if response["ok"].as_bool() != Some(true) {
        return Err(response["error"].as_str().unwrap_or("request failed").to_owned());
    }
    Ok(response)
}

/// A response as `--json` prints it: what it says, without `ok` and
/// `req_id`.
fn response_json(mut response: Value) -> Value {
    if let Some(map) = response.as_object_mut() {
        map.shift_remove("ok");
        map.shift_remove("req_id");
    }
    response
}

fn print_json(value: &Value) -> Result<(), String> {
    outln!("{}", serde_json::to_string_pretty(value).unwrap());
    Ok(())
}

/// A string, or `?`.
fn text(v: &Value) -> String {
    v.as_str().unwrap_or("?").to_owned()
}

fn read_stdin() -> Result<String, String> {
    let mut text = String::new();
    io::stdin().read_to_string(&mut text).map_err(|e| format!("stdin: {e}"))?;
    Ok(text)
}

fn print_table<const N: usize>(rows: Vec<[String; N]>) {
    // Escaped first, so the columns line up as shown.
    let rows: Vec<[String; N]> =
        rows.into_iter().map(|r| r.map(|cell| render::clean(&cell).into_owned())).collect();
    let widths: Vec<usize> =
        (0..N).map(|c| rows.iter().map(|r| r[c].chars().count()).max().unwrap()).collect();
    for row in rows {
        let cells: Vec<String> =
            row.iter().zip(&widths).map(|(cell, w)| format!("{cell:<w$}")).collect();
        outln!("{}", cells.join("  ").trim_end());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What `list` and `--resume` take from a transcript, whatever its end
    /// (ADR 55): ending in a newline or not; cut short; a whole line that
    /// isn't a record, last or before the last; empty; nothing readable.
    #[test]
    fn adr_0055_the_first_and_last_records_are_ones_brnr_can_read() {
        let path = env::temp_dir().join(format!("brnr-records-{}.jsonl", std::process::id()));
        let cases: [(&str, Option<i64>, Option<i64>); 9] = [
            ("{\"n\":1}\n{\"n\":2}\n", Some(1), Some(2)),
            ("{\"n\":1}\n{\"n\":2}", Some(1), Some(2)),
            ("{\"n\":1}\n{\"n\":2}\n{\"ts\":\"partial", Some(1), Some(2)),
            ("{\"n\":1}\n{\"n\":2}\nnot a record\n\n", Some(1), Some(2)),
            ("{\"n\":1}\n{\"n\":2}\n{\"ts\":\"part\n{\"n\":3}\n", Some(1), Some(3)),
            ("{\"ts\":\"part\n{\"n\":1}\n", Some(1), Some(1)),
            ("{\"n\":1}\n42\n", Some(1), Some(1)),
            ("", None, None),
            ("{\"ts\":\"partial", None, None),
        ];
        let n = |r: Option<Value>| r.and_then(|r| r["n"].as_i64());
        for (text, first, last) in cases {
            fs::write(&path, text).unwrap();
            assert_eq!((n(first_record(&path)), n(last_record(&path))), (first, last), "{text:?}");
        }
        // Past the 64 KiB a read backwards takes at a time.
        let long = format!(r#"{{"n":3,"pad":"{}"}}"#, "x".repeat(100 * 1024));
        let junk = "y".repeat(70_000);
        fs::write(&path, format!("{{\"n\":1}}\n{long}\n{junk}\n{{\"ts\":\"cut")).unwrap();
        assert_eq!(n(last_record(&path)), Some(3));
        let _ = fs::remove_file(path);
    }
}
