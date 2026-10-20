//! The control commands: start headless sessions and talk to running ones
//! over their processes' control sockets. Most are `brnr <group> <verb>`,
//! grouped by what they act on (ADR 8.1). `brnr --help` lists them; the
//! modules have the details:
//!
//! - talk.rs: `session new`, `session resume`, `prompt send`, `prompt cancel`,
//!   `event wait`, `queue`
//! - history.rs: `event log`
//! - settings.rs: `config get`, `config set`, `prompt commands`,
//!   `session fork`, `session close`
//! - delete.rs: `session delete`
//! - sessions.rs: `session list`
//! - show.rs: `permission show`; elicitation.rs: `elicitation requests`,
//!   `show`, `accept`, `decline` and `cancel`; notify.rs: `event notify`;
//!   doctor.rs: `doctor`; skill.rs: `skill`
//! - here: `process list`, `process stop`, `session status`,
//!   `permission requests`, `permission allow` and `reject`, `event watch`
//! - args.rs: every command's arguments, read from one table before it runs
//!
//! A `<session>` is a session's id, as the agent gave it. A `<pid>` is a
//! brnr process, which runs one agent for one or more sessions. Commands that
//! act on a session need it running and say so when it isn't. Which process
//! serves a session is what the session's lock says (ADR 3.9), whether that
//! process answers or not; ownership.rs says where a session is, and what a
//! command may do with it there.
//!
//! `--json` prints what the text says, as one JSON value (one event per line
//! for `event log`, `event watch` and `session new --foreground`).

use std::collections::HashMap;
use std::env;
use std::io::{self, ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use serde_json::{Value, json};

use crate::host::{EVENTS, NOTES, QUIET};
use crate::{json, lock, log, paths, render, transcript};

/// `println!` and `print!` for what the commands print: through
/// [`render::clean`], since so much of it is the agent's (titles, messages,
/// options), and quietly ending brnr when the reader goes away (`brnr session
/// list | head -1`), as a filter does, rather than panicking.
macro_rules! outln {
    ($($arg:tt)*) => { $crate::ctl::out(&format!("{}\n", format_args!($($arg)*))) };
}
macro_rules! out {
    ($($arg:tt)*) => { $crate::ctl::out(&format!($($arg)*)) };
}
/// `eprintln!` for messages with the agent's words in them.
macro_rules! errln {
    ($($arg:tt)*) => { eprintln!("{}", $crate::render::clean(&format!($($arg)*))) };
}

mod args;
mod catch_up;
mod client;
mod delete;
mod doctor;
mod elicitation;
mod history;
mod notify;
mod ownership;
mod sessions;
mod settings;
mod show;
mod skill;
mod talk;

use args::Args;
use client::{Conn, Host, discover};
use ownership::running_session;

/// The command groups (ADR 8.1): `brnr <group> <verb>`, a group being what
/// its commands act on.
const GROUPS: &[&str] =
    &["process", "session", "prompt", "queue", "permission", "elicitation", "config", "event"];

const USAGE: &str = "usage:
  brnr acp [--profile <p>] [--strict] [-- <agent> [args...]]
             what an editor runs as its ACP agent

process
  brnr process list [--json]
  brnr process stop <pid> [--json]

session
  brnr session new [--pid <pid>] [<new flags>] [-- <agent> [args...]]
                   a process for a headless session, in the background (or the foreground);
                   --pid: a session in that running process, without the process flags
  brnr session resume [--pid <pid>] <session> [--take-over] [<new flags>] [-- <agent> [args...]]
                   the same, carrying on a session
  brnr session status <session> [--json]
  brnr session fork <session> [--json]
  brnr session close <session> [--json]
  brnr session delete <session> [--purge] [--profile <p>] [--json] [-- <agent> [args...]]
  brnr session list [--include active,inactive] [--profile <p>] [--cwd <dir>] [--json]
                    [-- <agent> [args...]]

<new flags>
  process: [--profile <p>] [--auth <method>] [--strict] [--stop-when-idle <s>]
           [--response-timeout <s>] [--foreground [--quiet]]
  session: [--cwd <dir>] [--mode <m>] [--model <m>] [--thought-level <l>]
           [--option <o>=<v>]...
  prompt:  [--prompt <text> | -] [--file <path>]... [--image <path>]...
           [--wait [--timeout <s>]] [--json]

prompt
  brnr prompt send <session> [--steer | --interrupt | --context [--replace]]
                   [--file <path>]... [--image <path>]... [--wait [--timeout <s>]] [--json]
                   (<text>... | -)
  brnr prompt cancel <session> [--keep-held] [--json]
  brnr prompt commands <session> [--json]

queue
  brnr queue list <session> [--json]
  brnr queue show <session> <message> [--json]
  brnr queue drop <session> <message> [--json]
  brnr queue clear <session> [--messages] [--context] [--json]

permission
  brnr permission requests [<session>] [--json]
  brnr permission show <session> <request> [--json]
  brnr permission allow <session> <request> [--always] [--option <id>] [--json]
  brnr permission reject <session> <request> [--always] [--option <id>] [--json]

elicitation
  brnr elicitation requests [<session>] [--json]
  brnr elicitation show (<session> | --pid <pid>) <request> [--json]
  brnr elicitation accept (<session> | --pid <pid>) <request> [--field <k>=<v>]... [-] [--json]
  brnr elicitation decline (<session> | --pid <pid>) <request> [--json]
  brnr elicitation cancel (<session> | --pid <pid>) <request> [--json]

config
  brnr config get <session> [--mode] [--model] [--thought-level] [--option <o>]... [--json]
  brnr config set <session> [--mode <m>] [--model <m>] [--thought-level <l>]
                  [--option <o>=<v>]... [--json]

event
  brnr event log <session> [--last <n>] [--follow] [--events <default|all|event>,...] [--json]
  brnr event watch (<session> | --pid <pid>) [--events <default|all|event>,...] [--json]
  brnr event notify (<session> | --pid <pid> | --stdin) [--events <default|all|event>,...]
                    -- <command> [args...]
  brnr event wait <session> [--for idle|turn|permission|elicitation|exit] [--timeout <s>]
                  [--json]

brnr
  brnr doctor [--fix | --report] [--json]
  brnr skill [<reference> | install [--dir <dir>]...]
             the skill for agents that use brnr: print it, or install it
  brnr --version

brnr <group> --help lists a group's commands. <session> is a session's id, as brnr session list
shows it. <request> is a pending approval's or elicitation's handle, as brnr permission requests
and brnr elicitation requests show them. <pid> is a brnr process, which runs one agent for one or
more sessions, as brnr process list shows them. --json prints the same data as the text: one JSON value, or one event per line for event
log, event watch and session new --foreground. --strict is stable ACP only: no --steer into a
running turn, no session fork.";

/// How long a start may take until it commits, in seconds, unless
/// `BRNR_START_TIMEOUT` says otherwise. It goes in the start request: the
/// process fails the start when it passes, and `session new` gives up a little
/// later, for a process stuck too badly to say so.
const START_TIMEOUT: u64 = 120;
const START_GRACE: Duration = Duration::from_secs(10);

/// The control commands: everything but `acp` and `host`. A group's
/// commands are `brnr <group> <verb>`; the others, one word. Each verb's
/// arguments are read as [`args::VERBS`] has them, before it runs. Help
/// asked for goes to stdout and exits 0; a usage error, the command's usage
/// on stderr, exits 2, nothing done; a failure exits 1 (ADR 8.3).
pub(crate) fn main(args: Vec<String>) -> ExitCode {
    let cmd = args.first().map_or("", String::as_str);
    let grouped = GROUPS.contains(&cmd);
    let verb = if grouped { args.get(1).map(String::as_str) } else { None };
    let rest = args.get(if grouped { 2 } else { 1 }..).unwrap_or_default();
    // `skill install` is a verb of a command outside the groups.
    let (verb, rest) = match rest {
        [install, rest @ ..] if cmd == "skill" && install == "install" => (Some("install"), rest),
        _ => (verb, rest),
    };
    let result = match args::spec(cmd, verb) {
        Some(spec) if spec.asks_help(rest) => {
            outln!("{}", usage_of(cmd, verb));
            Ok(ExitCode::SUCCESS)
        }
        Some(spec) => spec.parse(rest).and_then(|a| run(spec, &a)),
        None => match (cmd, verb) {
            (group, Some("-h" | "--help")) if grouped => {
                outln!("{}", usage_of(group, None));
                Ok(ExitCode::SUCCESS)
            }
            (group, Some(verb)) if grouped => {
                Err(args::Error::Invalid(format!("unknown command: {group} {verb}")))
            }
            ("-h" | "--help", _) => {
                outln!("{USAGE}");
                Ok(ExitCode::SUCCESS)
            }
            ("-V" | "--version", _) => {
                outln!("brnr {}", env!("CARGO_PKG_VERSION"));
                Ok(ExitCode::SUCCESS)
            }
            (_, None) if grouped => Err(args::Error::Usage),
            ("", _) => Err(args::Error::Usage),
            (cmd, _) => Err(args::Error::Invalid(format!("unknown command: {cmd}"))),
        },
    };
    match result {
        Ok(code) => code,
        Err(e) => {
            match &e {
                args::Error::Usage => eprintln!("{}", usage_of(cmd, verb)),
                args::Error::Invalid(msg) => {
                    errln!("brnr: {msg}");
                    eprintln!("{}", usage_of(cmd, verb));
                }
                args::Error::Failed(msg) => errln!("brnr: {msg}"),
            }
            ExitCode::from(e.code())
        }
    }
}

/// Runs the verb `spec` is, with its arguments `a`.
fn run(spec: &args::Spec, a: &Args) -> Result<ExitCode, args::Error> {
    match (spec.cmd, spec.verb) {
        ("process", Some("list")) => done(ps(a)),
        ("process", Some("stop")) => done(stop(a)),
        ("session", Some("new")) => code(talk::new(a)),
        ("session", Some("resume")) => code(talk::resume(a)),
        ("session", Some("list")) => code(sessions::list(a)),
        ("session", Some("status")) => done(status(a)),
        ("session", Some("fork")) => code(settings::fork(a)),
        ("session", Some("close")) => code(settings::close(a)),
        ("session", Some("delete")) => code(delete::delete(a)),
        ("prompt", Some("send")) => code(talk::send(a)),
        ("prompt", Some("cancel")) => code(talk::cancel(a)),
        ("prompt", Some("commands")) => code(settings::commands(a)),
        ("queue", Some("list")) => code(talk::queue_list(a)),
        ("queue", Some("show")) => code(talk::queue_show(a)),
        ("queue", Some("drop")) => code(talk::queue_drop(a)),
        ("queue", Some("clear")) => code(talk::queue_clear(a)),
        ("permission", Some("requests")) => done(pending(a)),
        ("permission", Some("show")) => code(show::show(a)),
        ("permission", Some(verb @ ("allow" | "reject"))) => done(answer(a, verb)),
        ("elicitation", Some("requests")) => done(elicitation::requests(a)),
        ("elicitation", Some("show")) => done(elicitation::show(a)),
        ("elicitation", Some(verb @ ("accept" | "decline" | "cancel"))) => {
            done(elicitation::answer(a, verb))
        }
        ("config", Some("get")) => code(settings::config_get(a)),
        ("config", Some("set")) => code(settings::config_set(a)),
        ("event", Some("log")) => code(history::log(a)),
        ("event", Some("watch")) => code(watch(a)),
        ("event", Some("notify")) => code(notify::notify(a)),
        ("event", Some("wait")) => code(talk::wait(a)),
        ("doctor", None) => done(doctor::main(a)),
        ("skill", None) => done(skill::skill(a)),
        ("skill", Some("install")) => done(skill::install(a)),
        (cmd, verb) => unreachable!("{cmd} {verb:?} is in the table, so runs"),
    }
}

/// A verb's result, its error a usage error or a failure.
fn code<E: Into<args::Error>>(r: Result<ExitCode, E>) -> Result<ExitCode, args::Error> {
    r.map_err(Into::into)
}

/// The result of a verb that exits 0 when it succeeds.
fn done<E: Into<args::Error>>(r: Result<(), E>) -> Result<ExitCode, args::Error> {
    code(r.map(|()| ExitCode::SUCCESS))
}

/// The command `args` are, for a bug report: a group's with its verb.
pub(crate) fn command(args: &[String]) -> String {
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
/// with one it doesn't have). All of it for no command, and where to find
/// it for a command there is none of.
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
    match (lines.is_empty(), cmd) {
        (true, "") => return USAGE.to_owned(),
        (true, _) => return "brnr --help lists every command".to_owned(),
        _ => {}
    }
    // `session new` and `resume` share their flags, listed once.
    if lines.iter().any(|l| l.contains("<new flags>")) {
        let flags = USAGE.lines().skip_while(|l| *l != "<new flags>");
        lines.extend(flags.take_while(|l| !l.is_empty()));
    }
    format!("usage:\n{}\n(brnr --help for every command)", lines.join("\n"))
}

// ---- naming sessions and processes ---------------------------------------

/// The process with pid `pid`.
fn process<'a>(hosts: &'a [Host], pid: &str) -> Result<&'a Host, String> {
    hosts
        .iter()
        .find(|h| h.id() == pid)
        .ok_or(format!("no brnr process {pid} (see brnr process list)"))
}

/// `<session>` or `--pid <pid>`, for `event watch` and `event notify`: the
/// process, and the session if one was named.
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

fn ps(a: &Args) -> Result<(), String> {
    let json_out = a.json;
    let hosts = discover()?;
    let locks = lock::all();
    let rows: Vec<Value> = hosts
        .iter()
        .map(|host| {
            let info = host.info();
            // One that doesn't answer serves what it holds the locks of.
            let sessions: Vec<Value> = match host.status {
                Some(_) => host.sessions().iter().map(|s| s["session"].clone()).collect(),
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
    // Each session has a cwd of its own (brnr session list); the process's
    // is only where the editor happened to start it, so the table leaves it
    // to --json.
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

fn stop(a: &Args) -> Result<(), String> {
    let (pid, json_out) = (&a.positional[0], a.json);
    let hosts = discover()?;
    let host = process(&hosts, pid)?;
    let response = host.call(&json!({ "cmd": "stop" }))?;
    if json_out {
        // Its pid as `process list` has it, as `prompt cancel` names its session.
        let pid = host.id().parse::<u64>().ok();
        return print_json(&json!({ "pid": pid, "status": response["status"] }));
    }
    outln!("{}", text(&response["status"]));
    Ok(())
}

// ---- sessions ------------------------------------------------------------

/// Sessions with a transcript that no running process is serving, most
/// recently active first. Only the first and last record of each events
/// file are read: the first names the cwd, and the session it was forked
/// from if it opened as a fork (ADR 3.12), the last says when and in which
/// process the session was last active, or, if it is a deletion's, a record
/// of no process's (ADR 8.1), the last that names a process does. That
/// process's log says which agent it ran.
fn inactive_sessions(hosts: &[Host]) -> Vec<Value> {
    // A transcript is identified by its file (cwd folder + session id); a
    // session id alone can repeat across folders.
    let running: Vec<PathBuf> = hosts
        .iter()
        .flat_map(|h| {
            h.sessions().iter().filter_map(|s| s["events_log"].as_str().map(PathBuf::from))
        })
        .collect();

    // A session whose lock is held is running, whether its process answers
    // or not.
    let held: Vec<String> =
        lock::all().into_iter().filter(|e| e.pid.is_some()).filter_map(|e| e.session).collect();
    let mut started: HashMap<String, Value> = HashMap::new();
    let mut past = Vec::new();
    for file in transcript::events_files() {
        let (Some(first), Some(last)) =
            (transcript::first(&file), transcript::last(&file, |_| true))
        else {
            continue;
        };
        let Some(session) = last["session"].as_str() else { continue };
        if running.contains(&file) || held.iter().any(|s| s == session) {
            continue;
        }
        // The process that last served it: a deletion's record is no
        // process's (ADR 8.1).
        let served = match last["host_id"].is_string() {
            true => Some(last.clone()),
            false => transcript::last(&file, |r| r["host_id"].is_string()),
        };
        let host_id = served.as_ref().and_then(|r| r["host_id"].as_str());
        let host_id = host_id.unwrap_or_default().to_owned();
        let start = started
            .entry(host_id.clone())
            .or_insert_with(|| transcript::first(&paths::host_log(&host_id)).unwrap_or_default());
        let info = &start["event"]["info"];
        past.push(json!({
            "session": session,
            "cwd": first["event"]["cwd"],
            "forked_from": first["event"]["forked_from"],
            // When a process last served it: a deletion isn't activity.
            "last_active": served.as_ref().map_or(&last["ts"], |r| &r["ts"]),
            "profile": info["profile"],
            "agent": info["agent"],
            "events_log": file.to_string_lossy(),
        }));
    }
    past.sort_by(|a, b| b["last_active"].as_str().cmp(&a["last_active"].as_str()));
    past
}

/// The session an open session `s` (as its process reports it) was forked
/// from: as the process says, or, where it doesn't know (the fork resumed
/// since, here or in another process), as its transcript's first record
/// says (ADR 3.12). Null for one that isn't a fork, or whose transcript isn't
/// kept.
fn forked_from(s: &Value) -> Value {
    if s["forked_from"].is_string() {
        return s["forked_from"].clone();
    }
    let first = s["events_log"].as_str().and_then(|log| transcript::first(Path::new(log)));
    first.map_or(Value::Null, |first| first["event"]["forked_from"].clone())
}

fn agent_name(argv: &Value) -> String {
    let Some(program) = argv.as_array().and_then(|a| a.first()).and_then(Value::as_str) else {
        return "?".to_owned();
    };
    Path::new(program).file_name().map_or(program.to_owned(), |f| f.to_string_lossy().into_owned())
}

fn status(a: &Args) -> Result<(), String> {
    let (arg, json_out) = (&a.positional[0], a.json);
    let hosts = discover()?;
    let (host, id) = running_session(&hosts, arg)?;
    let st = host.status.as_ref().ok_or("the process is not answering")?;
    let x = host.session(&id).ok_or("no session")?;
    // Processes serving it without its lock, `shared_sessions` (ADR 3.9): what
    // happens there isn't in its transcript.
    let shares = |s: &Value| s["session"] == id.as_str() && s["shared"] == true;
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
        // An editor's session served without its lock (ADR 3.9).
        "lock_error": x["lock_error"],
        "cwd": x["cwd"],
        "forked_from": forked_from(x),
        "state": x["state"],
        "turn_seconds": x["turn_seconds"],
        "mode": x["mode"],
        "model": x["model"],
        "tools": x["tools"],
        "plan": x["plan"],
        "pending": x["pending"],
        "elicitations": x["elicitations"],
        "held": x["held"],
        "context": x["context"],
        "usage": x["usage"],
        // How the last turn ended, and the tokens the agent said it used
        // (ADR 4.4).
        "last_turn": x["last_turn"],
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
    if let Some(from) = x["forked_from"].as_str() {
        out.push_str(&format!("forked from {from}\n"));
    }
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
    // The state as JSON has it: `busy` is working, `waiting` is a turn
    // waiting for an approval or for input (ADR 4.5); how long when the
    // turn's start is known.
    let working = x["turn_seconds"].as_u64().map(|secs| format!("working for {}", duration(secs)));
    let pending = |key: &str| x[key].as_u64().filter(|&n| n > 0);
    let input = pending("elicitations").is_some() && pending("pending").is_none();
    let waiting = if input { "input" } else { "an approval" };
    match (x["state"].as_str(), working) {
        (Some("busy"), Some(working)) => out.push_str(&format!("{working}\n")),
        (Some("busy"), None) => out.push_str("working\n"),
        (Some("waiting"), Some(working)) => {
            out.push_str(&format!("waiting for {waiting}, {working}\n"))
        }
        (Some("waiting"), None) => out.push_str(&format!("waiting for {waiting}\n")),
        (state, _) => out.push_str(&format!("{}\n", state.unwrap_or("?"))),
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
    if let Some(n) = pending("pending") {
        out.push_str(&format!("{n} approval(s) waiting: brnr permission requests {arg}\n"));
    }
    if let Some(n) = pending("elicitations") {
        out.push_str(&format!("{n} elicitation(s) waiting: brnr elicitation requests {arg}\n"));
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
    let last = &x["last_turn"];
    if last.is_object() {
        let tokens = render::turn_tokens(&last["tokens"]);
        out.push_str(&format!("{}{tokens}\n", render::turn(last, "last turn")));
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

/// `2026-10-01T21:34:25.959660+02:00` as `2026-10-01 19:34:25Z`: an RFC
/// 3339 time in UTC, to the second; anything else as it is.
fn when(ts: &str) -> String {
    match log::utc(ts) {
        Some(utc) => format!("{} {}Z", &utc[..10], &utc[11..19]),
        None => ts.to_owned(),
    }
}

// ---- approvals -----------------------------------------------------------

/// The approvals waiting, in every process that answers (saying which
/// don't) or in one session's.
fn pending(a: &Args) -> Result<(), String> {
    let (arg, json_out) = (a.positional.first(), a.json);
    let hosts = discover()?;
    let (chosen, only): (Vec<&Host>, Option<String>) = match arg {
        // Those that answered `discover`: one that didn't is passed over, as
        // process list and session list do (ADR 3.9), not waited for again,
        // and said to be (P3).
        None => {
            for host in hosts.iter().filter(|h| h.status.is_none()) {
                eprintln!(
                    "brnr: process {} is not answering; its approvals aren't listed",
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
    // A request nests as deep as the agent made it: read as the host reads
    // the agent's line, and listed and let go of on a stack that takes it
    // (ADR 2.5), also when a process asked after it fails.
    let mut rows = Vec::new();
    for host in chosen {
        let more = host.pending(|pending| {
            pending.approvals(only.as_deref()).map(pending_row).collect::<Vec<_>>()
        });
        match more {
            Ok(more) => rows.extend(more),
            Err(e) => {
                json::free(Value::Array(rows));
                return Err(e);
            }
        }
    }
    json::holding(Value::Array(rows), |rows| list_pending(&rows, json_out))?
}

/// A permission request as `permission requests` lists it.
fn pending_row(p: &Value) -> Value {
    let options: Vec<Value> = p["options"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|o| json!({ "option_id": o["optionId"], "kind": o["kind"] }))
        .collect();
    json!({
        "session": p["session"],
        "request": p["request"],
        "owner": p["owner"],
        "kind": p["kind"],
        "title": p["title"],
        "options": options,
    })
}

/// The permission requests' `rows`, as a table or `--json`.
fn list_pending(rows: &Value, json_out: bool) -> Result<(), String> {
    if json_out {
        return print_json(rows);
    }
    let rows = rows.as_array().map_or(&[][..], Vec::as_slice);
    if rows.is_empty() {
        outln!("nothing waiting");
        return Ok(());
    }
    let mut table =
        vec![["SESSION", "REQUEST", "OWNER", "KIND", "TITLE", "OPTIONS"].map(String::from)];
    for r in rows {
        let options: Vec<String> = r["options"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|o| format!("{}={}", text(&o["option_id"]), text(&o["kind"])))
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

/// `permission allow` and `reject` (`cmd`): the request's option of one
/// kind, or the one `--option` names; the host picks it, and says why it
/// can't (ADR 8.1).
fn answer(a: &Args, cmd: &str) -> Result<(), String> {
    let [arg, request_id] = &a.positional[..] else { unreachable!("the table has two") };
    let (option, always, json_out) = (a.value("--option"), a.has("--always"), a.json);
    let hosts = discover()?;
    let (host, session) = running_session(&hosts, arg)?;
    let req = json!({
        "cmd": cmd,
        "session": session,
        "request": request_id,
        "always": always,
        "option": option,
    });
    let response = host.call(&req)?;
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
/// session's are its own plus the process's (the agent exiting). Cut off for
/// falling behind, it catches up from the transcript and goes on; unable to,
/// it ends with what it missed, an `events_missed` note, and exits 3 (ADR
/// 69).
fn watch(a: &Args) -> Result<ExitCode, args::Error> {
    let (session, pid, json_out) = (a.positional.first(), a.value("--pid"), a.json);
    let mut events = events_arg(a, &default_events())?;
    if session.is_some() == pid.is_some() {
        return Err(args::Error::Invalid("give a <session> or --pid <pid>".into()));
    }
    let hosts = discover()?;
    let (host, only_session) = session_or_pid(&hosts, session.map(String::as_str), pid)?;
    // `process_exited` (and for a session, `session_closed`) is how watch tells the
    // end from the process cutting the connection; asked for even when not
    // shown.
    let shown = events.clone();
    let ends: &[&str] = if only_session.is_some() {
        &["process_exited", "session_closed"]
    } else {
        &["process_exited"]
    };
    for end in ends {
        if !events.iter().any(|e| e == end) {
            events.push(end.to_string());
        }
    }
    let mut conn = Conn::open(host)?;
    let names: Vec<&str> = events.iter().map(String::as_str).collect();
    conn.subscribe(&names, only_session.as_deref())?;
    let options = render::Options { session: only_session.is_none(), time: true };
    let show = |event: &Value| {
        let text = if json_out { Some(event.to_string()) } else { render::event(event, &options) };
        let mut out = io::stdout().lock();
        match text {
            Some(text) => writeln!(out, "{}", render::clean(&text)).and_then(|()| out.flush()),
            None => Ok(()),
        }
    };
    loop {
        let event = match conn.next_event(None) {
            Ok(Some(event)) => event,
            Ok(None) => continue,
            Err(why) => match conn.missed() {
                Some(note) => {
                    let _ = show(note);
                    return Ok(ExitCode::from(catch_up::MISSED));
                }
                None => return Err(why.into()),
            },
        };
        // Shown and let go of on a stack that takes it (ADR 2.5). Whether
        // it ends the watch: the end, or stdout closed.
        let ended = conn.deep(event, |_, event| {
            let name = event["event"].as_str().unwrap_or_default();
            if let (Some(only), Some(session)) = (&only_session, event["session"].as_str())
                && only != session
            {
                return false;
            }
            (shown.iter().any(|e| e == name) && show(&event).is_err()) || ends.contains(&name)
        })?;
        if ended {
            return Ok(ExitCode::SUCCESS);
        }
    }
}

/// What `--events` asks for (in `event watch`, `event log` and `event
/// notify`): event names, `default` for the command's own `default` (what it
/// does without `--events`), `all` for every event. A note's name is taken
/// too, and left out of what is asked for: a note is shown whatever was
/// chosen (ADR 5.9), so `--events records_skipped` alone shows only notes.
fn events_arg(a: &Args, default: &[String]) -> Result<Vec<String>, args::Error> {
    let events = a.value_as("--events", |list| events_in(list, default));
    let events = events.map_err(args::Error::Invalid)?;
    let mut events = events.unwrap_or_else(|| default.to_vec());
    events.retain(|e| !NOTES.contains(&e.as_str()));
    Ok(events)
}

/// The events one `--events` names.
fn events_in(list: &str, default: &[String]) -> Result<Vec<String>, String> {
    let mut events = Vec::new();
    for name in list.split(',').map(str::trim) {
        let names = match name {
            "all" => EVENTS.iter().map(|e| e.to_string()).collect(),
            "default" => default.to_vec(),
            _ if EVENTS.contains(&name) || NOTES.contains(&name) => vec![name.to_owned()],
            _ => {
                let (names, notes) = (EVENTS.join(", "), NOTES.join(", "));
                let all = format!("events: default, all, {names}; notes: {notes}");
                return Err(format!("unknown event {name:?} ({all})"));
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

/// What `event watch` and `event log` show without `--events`: every event
/// but the quiet ones (`QUIET`: ACP messages, thoughts, usage, tool progress).
fn default_events() -> Vec<String> {
    EVENTS.iter().filter(|e| !QUIET.contains(e)).map(|e| e.to_string()).collect()
}

// ---- plumbing ------------------------------------------------------------

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

    /// `session status` says the state its JSON has: idle, busy (working, and
    /// for how long when it knows) or waiting for an approval (ADR 8.2), or for
    /// input (ADR 4.5).
    #[test]
    fn adr_0008_status_says_the_sessions_state() {
        let state = |state: &str, secs: Value| {
            let x = json!({ "session": "s", "state": state, "turn_seconds": secs });
            let text = describe_status(&x, "s");
            text.lines().nth(3).unwrap_or_default().to_owned()
        };
        assert_eq!(state("idle", Value::Null), "idle");
        assert_eq!(state("busy", json!(192)), "working for 3m12s");
        assert_eq!(state("busy", Value::Null), "working");
        assert_eq!(state("waiting", json!(40)), "waiting for an approval, working for 40s");
        assert_eq!(state("waiting", Value::Null), "waiting for an approval");
        // Waiting for an elicitation only (ADR 4.5).
        let x = json!({ "session": "s", "state": "waiting", "pending": 0, "elicitations": 1 });
        assert_eq!(describe_status(&x, "s").lines().nth(3), Some("waiting for input"));
    }
}
