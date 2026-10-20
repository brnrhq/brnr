//! `session list` (ADR 8.1, superseding 15): brnr's sessions and an agent's,
//! as one list joined on the session id, and the agent started just to be
//! asked.

use std::cmp::Reverse;
use std::collections::HashSet;
use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitCode, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::schema::{self, AgentCapabilities, Error, ErrorCode, ListSessionsResponse, SessionInfo};
use crate::{config, json, lock, log, paths, signals, spawn, sys, transcript};

use super::args::{self, Error::Invalid};
use super::client::{Host, discover};
use super::{Args, agent, agent_of, forked_from, inactive_sessions, print_json, print_table, when};

/// How long an agent started to be asked has to start and answer.
const ASK_TIMEOUT: Duration = Duration::from_secs(60);

/// How long that agent has to exit, after its stdin closes and it gets
/// SIGTERM, before its process group gets SIGKILL.
const STOP_WAIT: Duration = Duration::from_secs(5);

/// What `--include` takes: `active`, open in a process (`idle`, `busy`,
/// `waiting`, `unreachable`), and `inactive`, not.
const STATES: [&str; 2] = ["active", "inactive"];

/// `brnr session list`: brnr's index of sessions, every cwd's, with nothing
/// started; or, with an agent named, those in one cwd joined with the
/// agent's own list there.
pub(super) fn list(a: &Args) -> Result<ExitCode, args::Error> {
    let include = a.value_as("--include", states).map_err(Invalid)?.unwrap_or(STATES.to_vec());
    let (profile, cwd, json_out) = (a.value("--profile"), a.value("--cwd"), a.json);
    let agent = a.rest.clone().unwrap_or_default();
    if a.rest.as_ref().is_some_and(Vec::is_empty) {
        return Err(Invalid("-- needs an agent".into()));
    }
    let asked = named_agent(profile, agent)?;
    // Here, once there is an agent to ask; every cwd without one, unless
    // --cwd names one.
    let cwd = match cwd {
        Some(dir) => Some(paths::expand(dir)),
        None if asked.is_some() => Some(env::current_dir().map_err(|e| format!("cwd: {e}"))?),
        None => None,
    };
    let cwd = match cwd.map(|dir| std::path::absolute(&dir).map_err(|e| (dir, e))).transpose() {
        Ok(cwd) => cwd.map(|dir| dir.to_string_lossy().into_owned()),
        Err((dir, e)) => return Err(format!("{}: {e}", dir.display()).into()),
    };
    let listed = match (&asked, &cwd) {
        (Some(agent), Some(cwd)) => agent_sessions(agent, cwd)?,
        _ => Vec::new(),
    };

    let hosts = discover()?;
    let mut rows = brnr_rows(&hosts);
    if let Some(cwd) = &cwd {
        // The cwd's, however a row spells it (a trailing slash, a symlink).
        // One open in a process is the cwd's whatever its recorded cwd if the
        // agent lists it here, a session's lock being one per id (ADR 9.2);
        // an unreachable one's cwd is unknown (its process doesn't say), so it
        // is the cwd's if the agent lists it or brnr has its transcript here.
        let ids: HashSet<String> = listed.iter().map(|x| x.session_id.to_string()).collect();
        let dir = same_dir(cwd);
        rows.retain(|r| {
            let id = r["session"].as_str().unwrap_or_default();
            match r["cwd"].as_str() {
                _ if r["state"] != "inactive" && ids.contains(id) => true,
                Some(recorded) => same_dir(recorded) == dir,
                None => transcript_in(id, &dir),
            }
        });
    }
    // Joined on the id, exactly: never on which recorded agent the command
    // line might be (P4). The agent's title and time, where it gives them.
    let ours = rows.len();
    let program = agent_of(&json!(asked), &Value::Null);
    for x in listed {
        let id = x.session_id.to_string();
        let mut known = false;
        for r in rows[..ours].iter_mut().filter(|r| r["session"] == id.as_str()) {
            known = true;
            r["source"] = json!("both");
            if let Some(title) = &x.title {
                r["title"] = json!(title);
            }
            if let Some(at) = &x.updated_at {
                r["last_active"] = json!(at);
            }
            if r["cwd"].is_null() {
                r["cwd"] = json!(x.cwd);
            }
        }
        if !known {
            rows.push(json!({
                "session": id,
                "title": x.title,
                "state": "inactive",
                "pid": null,
                "agent": program,
                "source": "agent",
                "last_active": x.updated_at,
                "forked_from": null,
                "cwd": x.cwd,
            }));
        }
    }
    rows.retain(|r| {
        include.contains(&if r["state"] == "inactive" { "inactive" } else { "active" })
    });
    // Most recently active first, however many processes and agents, by the
    // time each names, to the nanosecond (before it is written to the
    // microsecond, below); then those with a time brnr can't read, then
    // those with none, each in the order they came.
    rows.sort_by_cached_key(|r| {
        let at = r["last_active"].as_str();
        (Reverse(at.and_then(log::parse_rfc3339)), at.is_none())
    });
    // In UTC, as brnr writes its own, so that each is shown as the time it
    // is. One that isn't an RFC 3339 time is passed on as it came, and said
    // (P4).
    for r in &mut rows {
        let Some(at) = r["last_active"].as_str().map(str::to_owned) else { continue };
        match log::utc(&at) {
            Some(utc) => r["last_active"] = json!(utc),
            None => errln!(
                "brnr: session {}: last active {at:?} isn't an RFC 3339 time, so it is shown as \
                 given, after the sessions with one",
                r["session"].as_str().unwrap_or_default()
            ),
        }
    }
    if json_out {
        print_json(&json!({ "sessions": rows }))?;
        return Ok(ExitCode::SUCCESS);
    }
    if rows.is_empty() {
        let state = if include.len() == 1 { format!("{} ", include[0]) } else { String::new() };
        let place = cwd.map(|dir| format!(" in {dir}")).unwrap_or_default();
        outln!("no {state}sessions{place}");
        return Ok(ExitCode::SUCCESS);
    }
    let head = [
        "SESSION",
        "TITLE",
        "STATE",
        "PID",
        "AGENT",
        "SOURCE",
        "LAST ACTIVE",
        "FORKED FROM",
        "CWD",
    ];
    let mut table = vec![head.map(String::from)];
    let cell = |v: &Value| v.as_str().unwrap_or("-").to_owned();
    for r in &rows {
        table.push([
            cell(&r["session"]),
            cell(&r["title"]),
            cell(&r["state"]),
            r["pid"].as_u64().map_or("-".to_owned(), |p| p.to_string()),
            cell(&r["agent"]["program"]),
            cell(&r["source"]),
            r["last_active"].as_str().map_or("-".to_owned(), when),
            cell(&r["forked_from"]),
            cell(&r["cwd"]),
        ]);
    }
    print_table(table);
    Ok(ExitCode::SUCCESS)
}

/// `dir` as the system has it, where it exists, so that two spellings of
/// one directory compare equal; as given where it doesn't (a [`PathBuf`]
/// ignores a trailing slash either way).
fn same_dir(dir: &str) -> PathBuf {
    fs::canonicalize(dir).unwrap_or_else(|_| PathBuf::from(dir))
}

/// Whether brnr has a transcript of session `id` that began in `dir`: the
/// file `inactive_sessions` reads, in whichever spelling of `dir` it was
/// opened with.
fn transcript_in(id: &str, dir: &Path) -> bool {
    transcript::of_session(id).iter().any(|log| {
        transcript::first(log)
            .and_then(|first| first["event"]["cwd"].as_str().map(same_dir))
            .is_some_and(|recorded| recorded == dir)
    })
}

/// `--include`'s states, each of [`STATES`].
fn states(list: &str) -> Result<Vec<&'static str>, String> {
    let mut states = Vec::new();
    for name in list.split(',').map(str::trim) {
        let Some(state) = STATES.iter().find(|s| **s == name) else {
            return Err(format!("unknown state {name:?} in --include (states: active, inactive)"));
        };
        if !states.contains(state) {
            states.push(*state);
        }
    }
    Ok(states)
}

/// The sessions brnr knows, a row each: those open in its processes (in
/// each that answers, as it says; in one that doesn't, those it holds the
/// locks of, `unreachable`, ADR 3.9), then those it has transcripts of that
/// none has open, `inactive`.
fn brnr_rows(hosts: &[Host]) -> Vec<Value> {
    let locks = lock::all();
    let mut rows = Vec::new();
    for host in hosts {
        let pid = host.id().parse::<u64>().ok();
        let agent = agent(host);
        for s in host.sessions() {
            rows.push(json!({
                "session": s["session"],
                "title": s["title"],
                "state": s["state"],
                "pid": pid,
                "agent": agent,
                "source": "brnr",
                "last_active": s["last_active"],
                "forked_from": forked_from(s),
                "cwd": s["cwd"],
            }));
        }
        for session in if host.status.is_none() { host.held(&locks) } else { Vec::new() } {
            rows.push(json!({
                "session": session,
                "title": null,
                "state": "unreachable",
                "pid": pid,
                "agent": agent,
                "source": "brnr",
                "last_active": null,
                "forked_from": null,
                "cwd": null,
            }));
        }
    }
    // Held by a process started with another runtime directory (ADR 3.9):
    // running, where the commands run here don't reach, so neither inactive
    // nor a process's listed here; its agent and cwd aren't known here.
    for held in locks.iter().filter(|e| e.elsewhere()) {
        rows.push(json!({
            "session": held.session,
            "title": null,
            "state": "unreachable",
            "pid": held.pid,
            "agent": agent_of(&Value::Null, &Value::Null),
            "source": "brnr",
            "last_active": null,
            "forked_from": null,
            "cwd": null,
        }));
    }
    for p in inactive_sessions(hosts) {
        rows.push(json!({
            "session": p["session"],
            "title": null,
            "state": "inactive",
            "pid": null,
            "agent": agent_of(&p["agent"], &Value::Null),
            "source": "brnr",
            "last_active": p["last_active"],
            "forked_from": p["forked_from"],
            "cwd": p["cwd"],
        }));
    }
    rows
}

/// The agent `--profile` and `-- <agent>` name, as a start's do: the one
/// after `--`, else the profile's. None when neither is given.
pub(super) fn named_agent(
    profile: Option<&str>,
    agent: Vec<String>,
) -> Result<Option<Vec<String>>, String> {
    if profile.is_none() && agent.is_empty() {
        return Ok(None);
    }
    let cfg = config::load(profile)?;
    let agent = if agent.is_empty() { cfg.agent.clone().unwrap_or_default() } else { agent };
    if agent.is_empty() {
        return Err("no agent: give one after -- or set agent in the profile".into());
    }
    Ok(Some(agent))
}

/// The agent's sessions in `cwd`, every page of `session/list`. An agent
/// that can't list fails (P7).
fn agent_sessions(agent: &[String], cwd: &str) -> Result<Vec<SessionInfo>, String> {
    let (mut asked, caps) = Asked::start(agent)?;
    if caps.session_capabilities.list.is_none() {
        return Err("the agent doesn't list its sessions".to_owned());
    }
    let mut sessions = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let mut params = json!({ "cwd": cwd });
        if let Some(cursor) = &cursor {
            params["cursor"] = json!(cursor);
        }
        let page = asked.ask("session/list", params)?;
        let page = schema::read::<ListSessionsResponse>(&page)
            .ok_or("session/list failed: the agent's answer isn't a list of sessions")?;
        sessions.extend(page.sessions);
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => return Ok(sessions),
        }
    }
}

/// An agent started just to be asked, with no process of brnr's: its
/// requests and their answers, over its stdin and stdout, within
/// [`ASK_TIMEOUT`] of its start. Dropping it stops it, and what it started:
/// its stdin closed, SIGTERM to its process group, then SIGKILL after
/// [`STOP_WAIT`].
pub(super) struct Asked {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: mpsc::Receiver<String>,
    deadline: Instant,
    next_id: u64,
}

impl Asked {
    /// Starts `agent` and initializes it, saying what it can do. A version
    /// of ACP other than brnr's fails, with nothing more asked (ADR 2.2).
    pub(super) fn start(agent: &[String]) -> Result<(Asked, AgentCapabilities), String> {
        let mut program = paths::expand(&agent[0]).into_os_string();
        if let Some(bundled) = spawn::bundled(&program) {
            program = bundled.into_os_string();
        }
        let mut cmd = Command::new(&program);
        cmd.args(&agent[1..])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .process_group(0);
        let mut child = signals::as_found(&mut cmd)
            .spawn()
            .map_err(|e| format!("{}: {e}", program.to_string_lossy()))?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().unwrap();
        let (tx, lines) = mpsc::channel::<String>();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    return;
                }
            }
        });
        let deadline = Instant::now() + ASK_TIMEOUT;
        let mut asked = Asked { child, stdin, lines, deadline, next_id: 1 };
        let init = asked.ask(
            "initialize",
            json!({
                "protocolVersion": schema::PROTOCOL_VERSION,
                "clientCapabilities": {},
                "clientInfo": { "name": "brnr", "version": env!("CARGO_PKG_VERSION") },
            }),
        )?;
        schema::check_protocol_version(&init)?;
        let caps = schema::read(&init["agentCapabilities"]).unwrap_or_default();
        Ok((asked, caps))
    }

    /// The agent's pid.
    pub(super) fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Sends a request and waits for its answer: its result, or its error.
    pub(super) fn ask(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.answer(method, params).map_err(|(_, why)| why)
    }

    /// As [`ask`](Self::ask), with the code of the agent's error when it
    /// answered with one: none when it didn't answer.
    pub(super) fn answer(
        &mut self,
        method: &str,
        params: Value,
    ) -> Result<Value, (Option<ErrorCode>, String)> {
        let id = self.next_id;
        self.next_id += 1;
        let req = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        let stdin = self.stdin.as_mut().ok_or((None, "the agent's stdin is closed".to_owned()))?;
        writeln!(stdin, "{req}").map_err(|e| (None, format!("the agent: {e}")))?;
        loop {
            let wait = self.deadline.saturating_duration_since(Instant::now());
            let line = self
                .lines
                .recv_timeout(wait)
                .map_err(|_| (None, format!("the agent didn't answer {method}")))?;
            // As the host reads it (see json.rs): a title cut mid-emoji is no
            // reason to miss the answer.
            let line = line.as_bytes();
            let Some(Some(msg)) = json::on_stack(json::depth(line), || json::parse::<Value>(line))
            else {
                continue;
            };
            if msg["id"] == id && msg.get("method").is_none() {
                if let Some(error) = msg.get("error") {
                    let code = schema::read::<Error>(error).map(|e| e.code);
                    return Err((
                        code,
                        format!("{method} failed: {}", schema::error_message(error)),
                    ));
                }
                return Ok(msg["result"].clone());
            }
        }
    }
}

impl Drop for Asked {
    fn drop(&mut self) {
        drop(self.stdin.take());
        let pid = self.child.id() as i32;
        sys::kill(-pid, libc::SIGTERM);
        let until = Instant::now() + STOP_WAIT;
        loop {
            match self.child.try_wait() {
                Ok(None) if Instant::now() < until => thread::sleep(Duration::from_millis(20)),
                // Not reaped yet, so the group id is still the agent's.
                Ok(None) => {
                    sys::kill(-pid, libc::SIGKILL);
                    let _ = self.child.wait();
                    return;
                }
                Ok(Some(_)) | Err(_) => return,
            }
        }
    }
}
