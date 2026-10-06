//! `brnr notify [<target>] [--events <a,b,...>] -- <command> [args...]`:
//! runs a command once per event, for notifications.
//!
//! The event is in the command's environment and, as JSON, on its stdin;
//! nothing is substituted into the command line, so what an agent writes
//! can't become arguments:
//!
//! - `BRNR_EVENT`: the event name; `BRNR_TEXT`: it as `brnr watch` would
//!   show it; `BRNR_TITLE`: the session's title, else its id;
//! - `BRNR_SESSION`, `BRNR_REQUEST` (a permission request's handle),
//!   `BRNR_MESSAGE` (the agent's last message in the session);
//! - `BRNR_HOST`, `BRNR_HOST_ID`.
//!
//! Without a target it notifies for `$BRNR_HOST`, so it works as a bridge in
//! a profile. `--events` is read as for `watch`, but its default (and
//! `default`) is `permission_request`, `turn_ended`, `exited`. It exits when
//! the host does.

use std::collections::HashMap;
use std::env;
use std::io::Write;
use std::os::fd::AsFd;
use std::process::{Command, ExitCode, Stdio};

use serde_json::Value;

use brnr::render;

use super::talk::Conn;
use super::{USAGE, discover, events_arg, resolve};

const DEFAULT_EVENTS: &[&str] = &["permission_request", "turn_ended", "exited"];

pub(super) fn notify(args: &[String]) -> Result<ExitCode, String> {
    let mut target = None;
    let default: Vec<String> = DEFAULT_EVENTS.iter().map(|e| e.to_string()).collect();
    let mut events = default.clone();
    let mut command = Vec::new();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--events" => events = events_arg(it.next(), &default)?,
            "--" => {
                command = it.by_ref().cloned().collect();
                break;
            }
            flag if flag.starts_with("--") => return Err(format!("unknown option: {flag}")),
            _ if target.is_none() => target = Some(arg.clone()),
            _ => return Err(USAGE.to_owned()),
        }
    }
    if command.is_empty() {
        return Err("notify needs a command after --".into());
    }
    let target = match target.or_else(|| env::var("BRNR_HOST").ok()) {
        Some(target) => target,
        None => return Err("notify needs a target (or $BRNR_HOST, as a bridge)".into()),
    };
    let hosts = discover();
    let (host, only) = resolve(&hosts, &target)?;
    let mut conn = Conn::open(host)?;
    // Agent messages and titles are tracked for the environment, not run for.
    let mut wanted: Vec<&str> = events.iter().map(String::as_str).collect();
    for extra in ["agent_message", "session_changed", "exited"] {
        if !wanted.contains(&extra) {
            wanted.push(extra);
        }
    }
    conn.subscribe(&wanted)?;
    let mut last_message: HashMap<String, String> = HashMap::new();
    // Titles the agent gave before we subscribed.
    let status = conn.call(serde_json::json!({ "cmd": "status" }))?;
    let mut titles: HashMap<String, String> = status["sessions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| {
            Some((s["session_id"].as_str()?.to_owned(), s["title"].as_str()?.to_owned()))
        })
        .collect();
    let options = render::Options { session: false, time: false };
    loop {
        let e = match conn.next_event(None) {
            Ok(Some(e)) => e,
            Ok(None) => continue,
            Err(_) => return Ok(ExitCode::SUCCESS), // The host is gone.
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
            run(
                &command,
                &e,
                &[
                    ("BRNR_EVENT", name.clone()),
                    ("BRNR_TEXT", text.unwrap_or_default()),
                    ("BRNR_TITLE", title),
                    ("BRNR_SESSION", session.clone()),
                    ("BRNR_REQUEST", e["request"].as_str().unwrap_or_default().to_owned()),
                    ("BRNR_MESSAGE", last_message.get(&session).cloned().unwrap_or_default()),
                    ("BRNR_HOST", host.id().to_owned()),
                    ("BRNR_HOST_ID", e["host_id"].as_str().unwrap_or_default().to_owned()),
                ],
            );
        }
        if name == "exited" {
            return Ok(ExitCode::SUCCESS);
        }
    }
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
