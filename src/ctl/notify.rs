//! `brnr notify (<session> | --pid <pid> | --stdin) [--events <a,b,...>] --
//! <command> [args...]`: runs a command once per event of a session, or of
//! every session in a process, for notifications.
//!
//! The event is in the command's environment and, as JSON, on its stdin;
//! nothing is substituted into the command line, so what an agent writes
//! can't become arguments:
//!
//! - `BRNR_EVENT`: the event name; `BRNR_TEXT`: it as `brnr watch` would
//!   show it; `BRNR_TITLE`: the session's title, else its id;
//! - `BRNR_SESSION_ID`, `BRNR_REQUEST` (an approval's handle),
//!   `BRNR_MESSAGE` (the agent's last message in the session);
//! - `BRNR_PID`, the process.
//!
//! The text, title and message have control characters escaped, as `watch`
//! shows them, and are cut at 32 KiB; the event on stdin has them whole.
//!
//! `--events` is read as for `watch`, but its default (and `default`) is
//! `permission_request`, `turn_ended`, `exited`. It exits when the process
//! does (or the session closes), and fails if the process cuts it off first.
//!
//! With `--stdin` it reads the events from its stdin, one per line, instead
//! of connecting: a started bridge's transport (ADR 35 and 36 in docs/adr).
//! As a bridge in a profile it is `command = ["brnr", "notify", "--stdin",
//! "--", …]`, and `BRNR_PID` is the one the process gives it. The bridge's
//! `events`, if the profile limits them, must include `agent_message`,
//! `session_changed` and `exited` for the environment and the end. It exits
//! when its stdin ends; ending before `exited` is being cut off.

use std::collections::HashMap;
use std::env;
use std::io::{self, BufRead, StdinLock, Write};
use std::os::fd::AsFd;
use std::process::{Command, ExitCode, Stdio};

use serde_json::Value;

use brnr::render;

use super::talk::Conn;
use super::{USAGE, discover, events_arg, session_or_pid};

const DEFAULT_EVENTS: &[&str] = &["permission_request", "turn_ended", "exited"];

/// The most of the agent's text one variable gets. Linux refuses to start a
/// program with a variable over 128 KiB, and macOS one whose environment and
/// arguments come to over 1 MiB.
const ENV_TEXT_MAX: usize = 32 << 10;

pub(super) fn notify(args: &[String]) -> Result<ExitCode, String> {
    let (mut session, mut pid, mut stdin) = (None, None, false);
    let default: Vec<String> = DEFAULT_EVENTS.iter().map(|e| e.to_string()).collect();
    let mut events = default.clone();
    let mut command = Vec::new();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--events" => events = events_arg(it.next(), &default)?,
            "--pid" => pid = Some(it.next().ok_or("--pid needs a pid")?.clone()),
            "--stdin" => stdin = true,
            "--" => {
                command = it.by_ref().cloned().collect();
                break;
            }
            flag if flag.starts_with("--") => return Err(format!("unknown option: {flag}")),
            _ if session.is_none() => session = Some(arg.clone()),
            _ => return Err(USAGE.to_owned()),
        }
    }
    if command.is_empty() {
        return Err("notify needs a command after --".into());
    }
    // Agent messages and titles are tracked for the environment, and the
    // ends watched for, not run for.
    let mut wanted: Vec<&str> = events.iter().map(String::as_str).collect();
    for extra in ["agent_message", "session_changed", "session_closed", "exited"] {
        if !wanted.contains(&extra) {
            wanted.push(extra);
        }
    }
    let Events { mut source, process, only, mut titles } =
        Events::open(stdin, session.as_deref(), pid.as_deref(), &wanted)?;
    let mut last_message: HashMap<String, String> = HashMap::new();
    let options = render::Options { session: false, time: false };
    loop {
        let e = match source.next() {
            Ok(Some(e)) => e,
            Ok(None) => continue,
            // Gone without an `exited`: notifications stop, which is a
            // failure (a notifier that falls behind is disconnected).
            Err(e) => return Err(format!("{e}; no more notifications")),
        };
        let name = e["event"].as_str().unwrap_or_default().to_owned();
        let session = e["session"].as_str().unwrap_or_default().to_owned();
        if only.as_deref().is_some_and(|s| !session.is_empty() && s != session) {
            continue;
        }
        match name.as_str() {
            "agent_message" => {
                last_message
                    .insert(session.clone(), e["text"].as_str().unwrap_or_default().to_owned());
            }
            "session_changed" if e["what"] == "title" => {
                titles.insert(session.clone(), e["value"].as_str().unwrap_or_default().to_owned());
            }
            _ => {}
        }
        if events.contains(&name) {
            let text = render::event(&e, &options);
            let title = titles.get(&session).cloned().unwrap_or_else(|| session.clone());
            let message = last_message.get(&session).cloned().unwrap_or_default();
            run(
                &command,
                &e,
                &[
                    ("BRNR_EVENT", name.clone()),
                    ("BRNR_TEXT", env_text(&text.unwrap_or_default())),
                    ("BRNR_TITLE", env_text(&title)),
                    ("BRNR_SESSION_ID", session.clone()),
                    ("BRNR_REQUEST", e["request"].as_str().unwrap_or_default().to_owned()),
                    ("BRNR_MESSAGE", env_text(&message)),
                    ("BRNR_PID", process.clone()),
                ],
            );
        }
        if name == "exited" || (name == "session_closed" && only.is_some()) {
            return Ok(ExitCode::SUCCESS);
        }
    }
}

/// Where the events come from, and what is known before the first.
struct Events {
    source: Source,
    /// The process, for `BRNR_PID`.
    process: String,
    /// The session they are of, if only one's.
    only: Option<String>,
    /// Titles the agent gave before.
    titles: HashMap<String, String>,
}

enum Source {
    Socket(Conn),
    /// A started bridge's (ADR 35 in docs/adr).
    Stdin(StdinLock<'static>),
}

impl Events {
    /// Subscribed to `wanted` on the socket of the process `session` or
    /// `pid` names; or, with `stdin`, what the process sends a started
    /// bridge.
    fn open(
        stdin: bool,
        session: Option<&str>,
        pid: Option<&str>,
        wanted: &[&str],
    ) -> Result<Events, String> {
        if stdin {
            if session.is_some() || pid.is_some() {
                return Err("--stdin takes no <session> or --pid".into());
            }
            // Subscribed from the process's start, so every title is to come.
            let process = env::var("BRNR_PID").unwrap_or_default();
            let source = Source::Stdin(io::stdin().lock());
            return Ok(Events { source, process, only: None, titles: HashMap::new() });
        }
        let hosts = discover()?;
        let (host, only) = session_or_pid(&hosts, session, pid)?;
        let mut conn = Conn::open(host)?;
        conn.subscribe(wanted)?;
        // Titles the agent gave before we subscribed.
        let status = conn.call(serde_json::json!({ "cmd": "status" }))?;
        let titles: HashMap<String, String> = status["sessions"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|s| {
                Some((s["session_id"].as_str()?.to_owned(), s["title"].as_str()?.to_owned()))
            })
            .collect();
        Ok(Events { source: Source::Socket(conn), process: host.id().to_owned(), only, titles })
    }
}

impl Source {
    /// The next event; `Ok(None)` for a line that isn't one, `Err` once
    /// there are no more.
    fn next(&mut self) -> Result<Option<Value>, String> {
        match self {
            Source::Socket(conn) => conn.next_event(None),
            Source::Stdin(stdin) => {
                let mut line = Vec::new();
                match stdin.read_until(b'\n', &mut line) {
                    Ok(0) => Err("stdin closed".into()),
                    Ok(_) => Ok(serde_json::from_slice::<Value>(&line)
                        .ok()
                        .filter(|e| e.get("event").is_some())),
                    Err(e) => Err(format!("stdin: {e}")),
                }
            }
        }
    }
}

/// The agent's text for the environment: as `watch` would show it (see
/// `render::clean`), and cut at [`ENV_TEXT_MAX`] bytes, with `…`. The
/// event on stdin has all of it.
fn env_text(text: &str) -> String {
    let text = render::clean(text);
    if text.len() <= ENV_TEXT_MAX {
        return text.into_owned();
    }
    let mut end = ENV_TEXT_MAX;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

/// Runs the command for one event and waits for it. Its stdout goes to our
/// stderr: as a bridge, our stdout is read by the host as requests.
fn run(command: &[String], event: &Value, vars: &[(&str, String)]) {
    let stdout = Stdio::from(std::io::stderr().as_fd().try_clone_to_owned().expect("stderr"));
    let child = Command::new(&command[0])
        .args(&command[1..])
        .envs(vars.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::piped())
        .stdout(stdout)
        .spawn();
    let mut child = match child {
        Ok(child) => child,
        Err(err) => return eprintln!("brnr notify: {}: {err}", command[0]),
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = writeln!(stdin, "{event}");
    }
    if let Ok(status) = child.wait()
        && !status.success()
    {
        eprintln!("brnr notify: {} exited with {status}", command[0]);
    }
}
