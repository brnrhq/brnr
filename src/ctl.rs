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
//! - delete.rs: `session delete`; prune.rs: `session prune`
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

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::env;
use std::io::{self, ErrorKind, Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use serde_json::{Value, json};

use crate::error::{self, Code};
use crate::host::{EVENTS, NOTES, QUIET};
use crate::{lock, log, render, transcript};

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

pub(crate) mod args;
mod catch_up;
mod client;
mod delete;
mod doctor;
mod elicitation;
mod history;
mod notify;
mod ownership;
mod prune;
mod remotes;
mod sessions;
mod settings;
mod show;
mod skill;
mod talk;

use args::Args;
use client::{Conn, Host, discover};
use ownership::running_session;
pub(crate) use prune::as_started as prune_as_started;

/// The command groups (ADR 8.1): `brnr <group> <verb>`, a group being what
/// its commands act on.
const GROUPS: &[&str] = &[
    "process",
    "session",
    "prompt",
    "queue",
    "permission",
    "elicitation",
    "config",
    "event",
    "remote",
];

const USAGE: &str = "usage:
  brnr acp [--profile <p>] [--strict] [--remote <target>] [-- <agent> [args...]]
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
  brnr session prune --older-than <s> [--dry-run] [--json]
                     delete brnr's transcripts of sessions, and the host logs of processes,
                     last active more than <s> seconds ago

<new flags>
  process: [--profile <p>] [--auth <method>] [--strict] [--stop-when-idle <s>]
           [--response-timeout <s>] [--foreground [--quiet]]
  session: [--cwd <dir>] [--mode <m>] [--model <m>] [--thought-level <l>]
           [--option <o>=<v>]...
  prompt:  [--prompt <text> | -] [--file <path>]... [--image <path>]...
           [--wait [--timeout <s>]] [--json]

prompt
  brnr prompt send <session> [--steer | --interrupt | --context [--replace]]
                   [--prompt <text> | -] [--file <path>]... [--image <path>]...
                   [--wait [--timeout <s>]] [--json]
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
  brnr permission allow <session> <request> [--always] [--option-id <id>] [--json]
  brnr permission reject <session> <request> [--always] [--option-id <id>] [--json]

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

remote
  brnr remote check (<target> | --profile <p>) [--json]

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
running turn, no session fork.
Execution commands accept --remote <[user@]host> before --: execute over SSH.
With --remote, --profile <p> is loaded on the destination; local brnr config is bypassed.
Without --remote, --profile <p> is loaded locally and can select a routing profile.
SSH uses existing host trust and keys.
Remote paths and event notify commands are on the destination. See brnr remote --help.";

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
    let (cmd, verb, rest) = split(&args);
    let grouped = GROUPS.contains(&cmd);
    let mut json = false;
    let result = match args::spec(cmd, verb) {
        Some(spec) if spec.asks_help(rest) => {
            outln!("{}", usage_of(cmd, verb));
            Ok(ExitCode::SUCCESS)
        }
        Some(spec) => spec.parse(rest).and_then(|a| {
            json = a.json;
            run(spec, &a)
        }),
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
                args::Error::Failed(error) => failed(error, json),
            }
            ExitCode::from(e.code())
        }
    }
}

/// `args` as a command, its verb and the words after them: `brnr <group>
/// <verb>`, `brnr skill install` or `brnr <command>`.
fn split(args: &[String]) -> (&str, Option<&str>, &[String]) {
    let cmd = args.first().map_or("", String::as_str);
    let grouped = GROUPS.contains(&cmd);
    let verb = if grouped { args.get(1).map(String::as_str) } else { None };
    let rest = args.get(if grouped { 2 } else { 1 }..).unwrap_or_default();
    // `skill install` is a verb of a command outside the groups.
    match rest {
        [install, rest @ ..] if cmd == "skill" && install == "install" => {
            (cmd, Some("install"), rest)
        }
        _ => (cmd, verb, rest),
    }
}

/// A command's failure (ADR 8.7): on stderr, its message and its code,
/// `brnr: no session s (see brnr session list) [session_not_found]`; with
/// `--json`, also as the command's JSON, `{"error": {code, message}}`, if it
/// printed none before it failed.
pub(crate) fn failed(error: &error::Error, json: bool) {
    errln!("brnr: {}", render::coded(&error.message, error.code.name()));
    if json && !PRINTED.load(Relaxed) {
        let _ = print_json(&json!({ "error": error.json() }));
    }
}

/// Whether the command has printed anything on stdout.
static PRINTED: AtomicBool = AtomicBool::new(false);

// ---- the library's facade ------------------------------------------------

thread_local! {
    /// The command this thread runs for the library's facade, if any (see
    /// [`call`]).
    static CAPTURE: RefCell<Option<Capture>> = const { RefCell::new(None) };
}

/// A command run for the library's typed facade (ADR 8.5): what it prints,
/// what it reads as its stdin, and what it missed.
struct Capture {
    out: Out,
    /// Its stdin: `--prompt -`'s prompt, `elicitation accept -`'s fields.
    stdin: Option<String>,
    /// The `events_missed` note of a command cut off that couldn't catch up
    /// (ADR 10.2), which it says on stderr, or as its last event.
    missed: Option<Value>,
    /// How the facade stops a stream it has stopped reading.
    stop: Option<Arc<Stop>>,
}

/// How the facade stops a stream's command once its caller has stopped
/// reading: the command's connections to the process are shut, so that it
/// doesn't wait on them for an event to write, holding them (ADR 10.4).
#[derive(Default)]
pub(crate) struct Stop {
    stopped: AtomicBool,
    /// The command's open connections, by the number [`stoppable`] gave
    /// each.
    conns: Mutex<Vec<(u64, UnixStream)>>,
    next: AtomicU64,
}

impl Stop {
    /// Stops the command: what it waits on ends, and it doesn't catch up.
    pub(crate) fn stop(&self) {
        let mut conns = self.conns.lock().unwrap_or_else(PoisonError::into_inner);
        self.stopped.store(true, Relaxed);
        for (_, conn) in conns.drain(..) {
            let _ = conn.shutdown(Shutdown::Both);
        }
    }
}

/// A connection [`Stop`] shuts while it is open: dropped with it, as one
/// that catching up replaces is, it goes from the list.
pub(super) struct Stoppable {
    stop: Arc<Stop>,
    id: u64,
}

impl Drop for Stoppable {
    fn drop(&mut self) {
        let mut conns = self.stop.conns.lock().unwrap_or_else(PoisonError::into_inner);
        conns.retain(|(id, _)| *id != self.id);
    }
}

/// Where a facade's command prints: kept until it ends, or each write sent
/// on as it comes, a stream's events.
enum Out {
    Kept(String),
    Sent(SyncSender<String>),
}

/// What a command run for the facade did: its exit status, what it printed
/// if it was kept, and the note of what it missed.
pub(crate) struct Ran {
    pub(crate) status: u8,
    pub(crate) out: String,
    pub(crate) missed: Option<Value>,
}

/// Runs `brnr <args>` for the library's facade, as [`main`] would, but that
/// what it prints is kept (or sent on `lines`, each write as it comes), its
/// stdin is `stdin`, and a usage error, which has no code on the command
/// line, is `usage_invalid` (ADR 8.5). What it says on stderr, it says. No
/// command line is written by a person here, so help isn't asked for.
pub(crate) fn call(
    args: &[String],
    stdin: Option<String>,
    lines: Option<(SyncSender<String>, Arc<Stop>)>,
) -> Result<Ran, error::Error> {
    let (out, stop) = match lines {
        Some((lines, stop)) => (Out::Sent(lines), Some(stop)),
        None => (Out::Kept(String::new()), None),
    };
    CAPTURE.set(Some(Capture { out, stdin, missed: None, stop }));
    // Taken however the command ends, a panic too: this thread's next
    // command, `brnr::run`'s, prints.
    struct Taken;
    impl Drop for Taken {
        fn drop(&mut self) {
            CAPTURE.take();
        }
    }
    let _taken = Taken;
    let (cmd, verb, rest) = split(args);
    let usage = |e: args::Error| match e {
        args::Error::Failed(error) => error,
        args::Error::Invalid(message) => error::Error::new(Code::usage_invalid, message),
        args::Error::Usage => error::Error::new(Code::usage_invalid, usage_of(cmd, verb)),
    };
    let result = match args::spec(cmd, verb) {
        Some(spec) => spec.parse(rest).and_then(|a| run(spec, &a)).map_err(usage),
        None => Err(usage(args::Error::Usage)),
    };
    let capture = CAPTURE.take().expect("set until now");
    let status = result?;
    let status = (0..=u8::MAX).find(|&n| ExitCode::from(n) == status).unwrap_or(1);
    let out = match capture.out {
        Out::Kept(out) => out,
        Out::Sent(_) => String::new(),
    };
    Ok(Ran { status, out, missed: capture.missed })
}

/// Whether this thread runs a command for the facade.
fn capturing() -> bool {
    CAPTURE.with_borrow(Option::is_some)
}

/// Whether the facade has stopped this thread's command (see [`Stop`]).
fn stopped() -> bool {
    CAPTURE.with_borrow(|c| {
        c.as_ref().and_then(|c| c.stop.as_ref()).is_some_and(|s| s.stopped.load(Relaxed))
    })
}

/// `conn`, a connection the command opened to a process, to be shut when
/// the facade stops the command, at once if it has, until the returned
/// [`Stoppable`] is dropped with the connection.
fn stoppable(conn: &UnixStream) -> Option<Stoppable> {
    let stop = CAPTURE.with_borrow(|c| c.as_ref().and_then(|c| c.stop.clone()))?;
    let conn = conn.try_clone().ok()?;
    let id = stop.next.fetch_add(1, Relaxed);
    let mut conns = stop.conns.lock().unwrap_or_else(PoisonError::into_inner);
    match stop.stopped.load(Relaxed) {
        true => drop(conn.shutdown(Shutdown::Both)),
        false => conns.push((id, conn)),
    }
    drop(conns);
    Some(Stoppable { stop, id })
}

/// For a test: this thread runs a stream's command, which `stop` stops
/// (`None`: none).
#[cfg(test)]
fn stopping(stop: Option<Arc<Stop>>) {
    let out = Out::Kept(String::new());
    CAPTURE.set(stop.map(|stop| Capture { out, stdin: None, missed: None, stop: Some(stop) }));
}

/// For a test: how many connections `stop` would shut.
#[cfg(test)]
fn stoppables(stop: &Stop) -> usize {
    stop.conns.lock().unwrap().len()
}

/// Keeps `note`, an `events_missed` (ADR 10.2), for the facade's command,
/// which returns it.
fn missed(note: &Value) {
    CAPTURE.with_borrow_mut(|c| c.iter_mut().for_each(|c| c.missed = Some(note.clone())));
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
        ("session", Some("prune")) => code(prune::prune(a)),
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
        ("doctor", None) => code(doctor::main(a)),
        ("remote", Some("check")) => code(remotes::check(a)),
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
    match write_out(text) {
        Ok(()) => {}
        // The facade's caller has stopped reading: the command finds out
        // where it writes a stream's next event (`write_out`).
        Err(e) if e.kind() == ErrorKind::BrokenPipe && capturing() => {}
        Err(e) if e.kind() == ErrorKind::BrokenPipe => std::process::exit(0),
        Err(e) => panic!("writing to stdout: {e}"),
    }
}

/// Writes `text`, [`render::clean`]ed, to stdout, and flushes it. All a
/// command prints goes through here, so that [`failed`] knows whether it has
/// printed any (a stream's events), and prints no `{"error"}` after it
/// (ADR 8.7). A command run for the facade writes to it instead (see
/// [`call`]): a caller gone is a reader gone, `BrokenPipe`.
fn write_out(text: &str) -> io::Result<()> {
    let text = render::clean(text);
    let captured = CAPTURE.with_borrow_mut(|c| {
        c.as_mut().map(|c| match &mut c.out {
            Out::Kept(kept) => {
                kept.push_str(&text);
                Ok(())
            }
            Out::Sent(lines) => {
                lines.send(text.to_string()).map_err(|_| io::Error::from(ErrorKind::BrokenPipe))
            }
        })
    });
    if let Some(written) = captured {
        return written;
    }
    PRINTED.store(true, Relaxed);
    let mut stdout = io::stdout().lock();
    stdout.write_all(text.as_bytes()).and_then(|()| stdout.flush())
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
fn process<'a>(hosts: &'a [Host], pid: &str) -> Result<&'a Host, error::Error> {
    hosts.iter().find(|h| h.id() == pid).ok_or_else(|| {
        let error = format!("no brnr process {pid} (see brnr process list)");
        error::Error::new(Code::process_not_found, error)
    })
}

/// `<session>` or `--pid <pid>`, for `event watch` and `event notify`: the
/// process, and the session if one was named.
fn session_or_pid<'a>(
    hosts: &'a [Host],
    session: Option<&str>,
    pid: Option<&str>,
) -> Result<(&'a Host, Option<String>), error::Error> {
    match (session, pid) {
        (Some(session), None) => running_session(hosts, session).map(|(h, id)| (h, Some(id))),
        (None, Some(pid)) => process(hosts, pid).map(|h| (h, None)),
        _ => Err("give a <session> or --pid <pid>".into()),
    }
}

// ---- processes -----------------------------------------------------------

fn ps(a: &Args) -> Result<(), error::Error> {
    let json_out = a.json;
    let hosts = discover()?;
    let locks = lock::all();
    let rows: Vec<Value> = hosts
        .iter()
        .map(|host| {
            let info = host.info();
            // One that doesn't answer serves what it holds the locks of; its
            // sessions are rows of `session list`, with their state only.
            let sessions: Vec<Value> = match host.status {
                Some(_) => host
                    .sessions()
                    .iter()
                    .map(|s| json!({ "session": s["session"], "state": s["state"] }))
                    .collect(),
                None => host
                    .held(&locks)
                    .into_iter()
                    .map(|s| json!({ "session": s, "state": "unreachable" }))
                    .collect(),
            };
            json!({
                "pid": host.id().parse::<u64>().ok(),
                "state": process_state(host),
                // Who owns its sessions; not known of one that doesn't answer.
                "owner": info["owner"],
                "agent": agent(host),
                "sessions": sessions,
                "uptime": info["uptime"],
                "cwd": info["cwd"],
            })
        })
        .collect();
    if json_out {
        return print_json(&json!({ "processes": rows }));
    }
    if rows.is_empty() {
        outln!("no brnr processes");
        return Ok(());
    }
    // Each session has a cwd of its own (brnr session list); the process's
    // is only where the editor happened to start it, so the table leaves it
    // to --json.
    let mut table = vec![["PID", "STATE", "OWNER", "AGENT", "SESSIONS", "UP"].map(String::from)];
    for r in &rows {
        let sessions: Vec<String> =
            r["sessions"].as_array().into_iter().flatten().map(|s| text(&s["session"])).collect();
        table.push([
            r["pid"].to_string(),
            text(&r["state"]),
            r["owner"].as_str().unwrap_or("-").to_owned(),
            r["agent"]["program"].as_str().unwrap_or("?").to_owned(),
            if sessions.is_empty() { "-".to_owned() } else { sessions.join(", ") },
            r["uptime"].as_u64().map_or("?".to_owned(), duration),
        ]);
    }
    print_table(table);
    Ok(())
}

/// A process's `state` in `process list`: `starting`, `running`,
/// `stopping`, or `unreachable` for one that doesn't answer, as `session
/// list` has a session in it (ADR 8.2).
fn process_state(host: &Host) -> &'static str {
    match &host.status {
        None => "unreachable",
        Some(st) if st["stopping"] == true => "stopping",
        Some(st) if st["starting"] == true => "starting",
        Some(_) => "running",
    }
}

fn stop(a: &Args) -> Result<(), error::Error> {
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
/// recently active first, a row each, whatever folders its files are in
/// (ADR 5.1). Only the first and last records of each events file, and its
/// last `session_opened`, are read:
/// the session began where its oldest file's first record says, which names
/// the session it was forked from if it opened as a fork (ADR 3.12); the
/// newest record of them all says when and in which process the session was
/// last active, or, if it is a deletion's, which isn't activity whoever
/// recorded it (ADR 8.1), the newest other record that names a process
/// does. The newest `session_opened` of them all, the last in each file,
/// says the cwd, profile and agent the session was last served with (ADR
/// 3.11): one process serves it at a time (ADR 3.9), and a process that
/// didn't open it (an editor's deletion, ADR 8.1) records into every file
/// of it. The transcript alone says it, not the host log (ADR 5.10). A file
/// whose newest record is in a format this brnr doesn't read isn't one, as
/// where the session was last served isn't known; `doctor` lists it (ADR
/// 5.10).
fn inactive_sessions(hosts: &[Host]) -> Vec<Value> {
    // One served by a process, or whose lock is held, is running, whatever
    // folder its files are in, and whether its process answers or not.
    let served =
        hosts.iter().flat_map(|h| h.sessions().iter().filter_map(|s| s["session"].as_str()));
    let mut running: HashSet<String> = served.map(str::to_owned).collect();
    running.extend(lock::all().into_iter().filter(|e| e.pid.is_some()).filter_map(|e| e.session));
    // By id: the oldest file's first record; when a process last recorded
    // in it, and in which file; and its files.
    type Found = (Value, (Value, PathBuf), Vec<PathBuf>);
    let mut found: HashMap<String, Found> = HashMap::new();
    for file in transcript::events_files() {
        let Some(last) = transcript::newest(&file).filter(transcript::readable) else { continue };
        let Some(first) = transcript::first(&file) else { continue };
        let Some(session) = last["session"].as_str() else { continue };
        if running.contains(session) {
            continue;
        }
        let at = active_at(&file, &last);
        let Some((oldest, newest, files)) = found.get_mut(session) else {
            found.insert(session.to_owned(), (first, (at, file.clone()), vec![file]));
            continue;
        };
        if first["ts"].as_str() < oldest["ts"].as_str() {
            *oldest = first;
        }
        files.push(file.clone());
        if at.as_str() > newest.0.as_str() {
            *newest = (at, file);
        }
    }
    let mut past = Vec::new();
    for (session, (oldest, (at, file), files)) in found {
        let opening = |r: &Value| r["event"]["event"] == "session_opened";
        let openings = files.iter().filter_map(|path| transcript::last(path, opening));
        let opened = openings.max_by(|a, b| a["ts"].as_str().cmp(&b["ts"].as_str()));
        let opened = opened.unwrap_or_default();
        let opened = &opened["event"];
        past.push(json!({
            "session": session,
            "cwd": opened["cwd"],
            "forked_from": oldest["event"]["forked_from"],
            "last_active": at,
            "profile": opened["profile"],
            "agent": opened["agent"],
            "events_log": file.to_string_lossy(),
        }));
    }
    past.sort_by(|a, b| b["last_active"].as_str().cmp(&a["last_active"].as_str()));
    past
}

/// When a process last served the session of the events file at `file`,
/// whose newest record is `last`: a deletion isn't serving, nor activity
/// (ADR 8.1). `session delete`'s record is no process's, and an editor's,
/// through a process that may never have opened the session, goes into
/// every file of it.
fn active_at(file: &Path, last: &Value) -> Value {
    let serving = |r: &Value| r["host_id"].is_string() && r["event"]["event"] != "session_deleted";
    let served = match serving(last) {
        true => Some(last.clone()),
        false => transcript::last(file, serving),
    };
    served.as_ref().map_or(&last["ts"], |r| &r["ts"]).clone()
}

/// The session an open session `s` (as its process reports it) was forked
/// from: as the process says, or, where it doesn't know (the fork resumed
/// since, here or in another process), as its transcript's first record
/// says, its oldest file's (ADR 3.12, ADR 5.1). Null for one that isn't a
/// fork, or whose transcript isn't kept.
fn forked_from(s: &Value) -> Value {
    if s["forked_from"].is_string() {
        return s["forked_from"].clone();
    }
    let oldest = s["session"].as_str().and_then(|id| transcript::files_of(id).into_iter().next());
    let first = oldest.or_else(|| s["events_log"].as_str().map(PathBuf::from));
    let first = first.and_then(|log| transcript::first(&log));
    first.map_or(Value::Null, |first| first["event"]["forked_from"].clone())
}

/// The file name of `argv`'s program, if it has one.
fn program(argv: &Value) -> Option<String> {
    let program = argv.as_array().and_then(|a| a.first()).and_then(Value::as_str)?;
    let name = Path::new(program).file_name();
    Some(name.map_or(program.to_owned(), |f| f.to_string_lossy().into_owned()))
}

/// The agent as every `--json` has it (ADR 8.2): `program`, the file name
/// of the command brnr ran, from `argv`; `name` and `version`, what it says
/// it is in `initialize` (`agentInfo`, `info`). Each null where brnr doesn't
/// know it.
fn agent_of(argv: &Value, info: &Value) -> Value {
    json!({
        "program": program(argv),
        "name": info["name"].as_str(),
        "version": info["version"].as_str(),
    })
}

/// The agent process `host` runs.
fn agent(host: &Host) -> Value {
    let info = host.info();
    agent_of(&info["agent"], &info["agent_info"])
}

fn status(a: &Args) -> Result<(), error::Error> {
    let (arg, json_out) = (&a.positional[0], a.json);
    let hosts = discover()?;
    let (host, id) = running_session(&hosts, arg)?;
    let unreachable =
        || error::Error::new(Code::process_unreachable, "the process is not answering");
    let st = host.status.as_ref().ok_or_else(unreachable)?;
    let x = host
        .session(&id)
        .ok_or_else(|| error::Error::new(Code::session_not_found, "no session"))?;
    // A count it doesn't have isn't 0 (P4).
    for key in client::COUNTS {
        client::count(x, key).map_err(|e| client::skewed(e, &host.meta))?;
    }
    // Processes serving it without its lock, `shared_sessions` (ADR 3.9): what
    // happens there isn't in its transcript.
    let shares = |s: &Value| s["session"] == id.as_str() && s["shared"] == true;
    let shared_pids: Vec<u64> = hosts
        .iter()
        .filter(|h| h.sessions().iter().any(shares))
        .filter_map(|h| h.id().parse().ok())
        .collect();
    let report = json!({
        "session": id,
        "title": x["title"],
        "pid": host.id().parse::<u64>().ok(),
        // What the agent says it is, in initialize: the npm package and
        // version of an adapter.
        "agent": agent(host),
        "owner": st["owner"],
        "lock_pid": lock::holder(&id),
        "shared_pids": shared_pids,
        // An editor's session served without its lock (ADR 3.9).
        "lock_error": x["lock_error"],
        "cwd": x["cwd"],
        "forked_from": forked_from(x),
        "state": x["state"],
        "turn_elapsed": x["turn_elapsed"],
        "mode": x["mode"],
        "model": x["model"],
        "tools": x["tools"],
        "plan": x["plan"],
        "pending_count": x["pending_count"],
        "elicitation_count": x["elicitation_count"],
        "held_count": x["held_count"],
        "context_count": x["context_count"],
        "usage": x["usage"],
        // How the last turn ended, and the tokens the agent said it used
        // (ADR 4.4).
        "last_turn": x["last_turn"],
        "last_message": x["last_message"],
        "last_active": x["last_active"],
        "stop_when_idle": st["stop_when_idle"],
        "stopping": st["stopping"],
        "uptime": st["uptime"],
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
    match (a["name"].as_str(), a["version"].as_str()) {
        (Some(name), Some(version)) => agent.push_str(&format!(" ({name} {version})")),
        (Some(name), None) => agent.push_str(&format!(" ({name})")),
        _ => {}
    }
    out.push_str(&format!(
        "process {}: {agent}, {}, up {}\n",
        x["pid"],
        text(&x["owner"]),
        duration(x["uptime"].as_u64().unwrap_or(0))
    ));
    let shared_pids: Vec<String> =
        x["shared_pids"].as_array().into_iter().flatten().map(Value::to_string).collect();
    if !shared_pids.is_empty() {
        let held = match x["lock_pid"].as_u64() {
            Some(pid) => format!("process {pid}'s"),
            None => "no process's (none holds it now)".to_owned(),
        };
        out.push_str(&format!(
            "shared by process {}: the transcript is {held}, and has none of what happens there\n",
            shared_pids.join(", ")
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
    let working = x["turn_elapsed"].as_u64().map(|secs| format!("working for {}", duration(secs)));
    let pending = |key: &str| x[key].as_u64().filter(|&n| n > 0);
    let input = pending("elicitation_count").is_some() && pending("pending_count").is_none();
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
    if let Some(n) = pending("pending_count") {
        out.push_str(&format!("{n} approval(s) waiting: brnr permission requests {arg}\n"));
    }
    if let Some(n) = pending("elicitation_count") {
        out.push_str(&format!("{n} elicitation(s) waiting: brnr elicitation requests {arg}\n"));
    }
    let count = |key: &str| x[key].as_u64().unwrap_or(0);
    let (held, context) = (count("held_count"), count("context_count"));
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
fn pending(a: &Args) -> Result<(), error::Error> {
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
    let mut rows = Vec::new();
    for host in chosen {
        rows.extend(host.pending()?.approvals(only.as_deref()).map(pending_row));
    }
    list_pending(&Value::Array(rows), json_out)
}

/// A permission request as `permission requests` lists it: `permission
/// show`'s object, with fewer fields, the agent's options whole as ACP has
/// them (ADR 8.2).
fn pending_row(p: &Value) -> Value {
    json!({
        "session": p["session"],
        "request": p["request"],
        "owner": p["owner"],
        "kind": p["kind"],
        "title": p["title"],
        "options": p["options"],
    })
}

/// The permission requests' `rows`, as a table or `--json`.
fn list_pending(rows: &Value, json_out: bool) -> Result<(), error::Error> {
    if json_out {
        return print_json(&json!({ "requests": rows }));
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
            .map(|o| format!("{}={}", text(&o["optionId"]), text(&o["kind"])))
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
/// kind, or the one `--option-id` names; the host picks it, and says why it
/// can't (ADR 8.1).
fn answer(a: &Args, cmd: &str) -> Result<(), error::Error> {
    let [arg, request_id] = &a.positional[..] else { unreachable!("the table has two") };
    let (option_id, always, json_out) = (a.value("--option-id"), a.has("--always"), a.json);
    let hosts = discover()?;
    let (host, session) = running_session(&hosts, arg)?;
    let mut req =
        json!({ "cmd": cmd, "session": session, "request": request_id, "always": always });
    // Absent, not null, without --option-id: the host refuses a null one.
    if let Some(id) = option_id {
        req["option_id"] = json!(id);
    }
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
        text.map_or(Ok(()), |text| write_out(&format!("{text}\n")))
    };
    loop {
        let event = match conn.next_event(None) {
            Ok(Some(event)) => event,
            Ok(None) => continue,
            Err(why) => match conn.missed() {
                Some(note) => {
                    missed(note);
                    let _ = show(note);
                    return Ok(ExitCode::from(catch_up::MISSED));
                }
                None => return Err(why.into()),
            },
        };
        let name = event["event"].as_str().unwrap_or_default();
        if let (Some(only), Some(session)) = (&only_session, event["session"].as_str())
            && only != session
        {
            continue;
        }
        // The end, or stdout closed, ends the watch.
        if (shown.iter().any(|e| e == name) && show(&event).is_err()) || ends.contains(&name) {
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

fn print_json(value: &Value) -> Result<(), error::Error> {
    outln!("{}", serde_json::to_string_pretty(value).unwrap());
    Ok(())
}

/// A string, or `?`.
fn text(v: &Value) -> String {
    v.as_str().unwrap_or("?").to_owned()
}

/// The command's stdin, read to its end; the facade's, for a command run
/// for it.
fn read_stdin() -> Result<String, error::Error> {
    if let Some(given) = CAPTURE.with_borrow_mut(|c| c.as_mut().map(|c| c.stdin.take())) {
        return Ok(given.unwrap_or_default());
    }
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

    /// A connection the facade would shut to stop a stream's command leaves
    /// its list as it closes, so a command that connects again and again,
    /// catching up, keeps no more than it has open (ADR 10.4); stopped, the
    /// command's next connection is shut as it opens.
    #[test]
    fn adr_0008_a_streams_connection_leaves_what_the_facade_shuts_as_it_closes() {
        let stop = Arc::new(Stop::default());
        let capture = |stop: &Arc<Stop>| Capture {
            out: Out::Kept(String::new()),
            stdin: None,
            missed: None,
            stop: Some(stop.clone()),
        };
        CAPTURE.set(Some(capture(&stop)));
        let open = || stop.conns.lock().unwrap().len();
        let (a, _a) = UnixStream::pair().unwrap();
        let (b, _b) = UnixStream::pair().unwrap();
        let (first, second) = (stoppable(&a).unwrap(), stoppable(&b).unwrap());
        assert_eq!(open(), 2);
        drop(first);
        assert_eq!(open(), 1);
        drop(second);
        assert_eq!(open(), 0);
        stop.stop();
        let (c, mut theirs) = UnixStream::pair().unwrap();
        let _c = stoppable(&c).unwrap();
        assert_eq!(open(), 0);
        let mut buf = [0];
        assert_eq!(io::Read::read(&mut theirs, &mut buf).unwrap(), 0, "shut as it opens");
        CAPTURE.take();
    }

    /// `session status` says the state its JSON has: idle, busy (working, and
    /// for how long when it knows) or waiting for an approval (ADR 8.2), or for
    /// input (ADR 4.5).
    #[test]
    fn adr_0008_status_says_the_sessions_state() {
        let state = |state: &str, secs: Value| {
            let x = json!({ "session": "s", "state": state, "turn_elapsed": secs });
            let text = describe_status(&x, "s");
            text.lines().nth(3).unwrap_or_default().to_owned()
        };
        assert_eq!(state("idle", Value::Null), "idle");
        assert_eq!(state("busy", json!(192)), "working for 3m12s");
        assert_eq!(state("busy", Value::Null), "working");
        assert_eq!(state("waiting", json!(40)), "waiting for an approval, working for 40s");
        assert_eq!(state("waiting", Value::Null), "waiting for an approval");
        // Waiting for an elicitation only (ADR 4.5).
        let x = json!({ "session": "s", "state": "waiting", "pending_count": 0, "elicitation_count": 1 });
        assert_eq!(describe_status(&x, "s").lines().nth(3), Some("waiting for input"));
    }
}
