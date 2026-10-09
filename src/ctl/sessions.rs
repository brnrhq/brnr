//! `session list` (ADR 63, superseding 15): brnr's sessions and an agent's,
//! as one list joined on the session id, and the agent started just to be
//! asked.

use std::collections::HashSet;
use std::env;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::process::CommandExt;
use std::process::{Child, ChildStdin, Command, ExitCode, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use brnr::schema::{self, AgentCapabilities, Error, ErrorCode, ListSessionsResponse, SessionInfo};
use brnr::{config, json, lock, paths, spawn, sys};

use super::{Host, USAGE, agent_name, discover, inactive_sessions, print_json, print_table, when};

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
pub(super) fn list(args: &[String]) -> Result<ExitCode, String> {
    let (mut include, mut profile, mut cwd, mut json_out) = (STATES.to_vec(), None, None, false);
    let mut agent = Vec::new();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--include" => include = states(it.next().ok_or("--include needs a list")?)?,
            "--profile" => profile = Some(it.next().ok_or("--profile needs a name")?.clone()),
            "--cwd" => cwd = Some(it.next().ok_or("--cwd needs a directory")?.clone()),
            "--json" => json_out = true,
            "--" => {
                agent = it.by_ref().cloned().collect();
                if agent.is_empty() {
                    return Err("-- needs an agent".into());
                }
            }
            _ => return Err(USAGE.to_owned()),
        }
    }
    let asked = named_agent(profile.as_deref(), agent)?;
    // Here, once there is an agent to ask; every cwd without one, unless
    // --cwd names one.
    let cwd = match cwd {
        Some(dir) => Some(paths::expand(&dir)),
        None if asked.is_some() => Some(env::current_dir().map_err(|e| format!("cwd: {e}"))?),
        None => None,
    };
    let cwd = match cwd.map(|dir| std::path::absolute(&dir).map_err(|e| (dir, e))).transpose() {
        Ok(cwd) => cwd.map(|dir| dir.to_string_lossy().into_owned()),
        Err((dir, e)) => return Err(format!("{}: {e}", dir.display())),
    };
    let listed = match (&asked, &cwd) {
        (Some(agent), Some(cwd)) => agent_sessions(agent, cwd)?,
        _ => Vec::new(),
    };

    let hosts = discover()?;
    let mut rows = brnr_rows(&hosts);
    if let Some(cwd) = &cwd {
        // An unreachable session's cwd is unknown (its process doesn't say):
        // it is this one's if the agent lists it here, a session's lock being
        // one per id whatever its cwd (ADR 53).
        let ids: HashSet<String> = listed.iter().map(|x| x.session_id.to_string()).collect();
        let here = |r: &Value| match r["cwd"].as_str() {
            Some(dir) => dir == cwd,
            None => r["session"].as_str().is_some_and(|id| ids.contains(id)),
        };
        rows.retain(here);
    }
    // Joined on the id, exactly: never on which recorded agent the command
    // line might be (P4). The agent's title and time, where it gives them.
    let ours = rows.len();
    let program = asked.as_ref().map(|a| agent_name(&json!(a)));
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
                "cwd": x.cwd,
            }));
        }
    }
    rows.retain(|r| {
        include.contains(&if r["state"] == "inactive" { "inactive" } else { "active" })
    });
    // Most recently active first, however many processes and agents.
    rows.sort_by(|a, b| b["last_active"].as_str().cmp(&a["last_active"].as_str()));
    if json_out {
        print_json(&json!(rows))?;
        return Ok(ExitCode::SUCCESS);
    }
    if rows.is_empty() {
        let state = if include.len() == 1 { format!("{} ", include[0]) } else { String::new() };
        let place = cwd.map(|dir| format!(" in {dir}")).unwrap_or_default();
        outln!("no {state}sessions{place}");
        return Ok(ExitCode::SUCCESS);
    }
    let head = ["SESSION", "TITLE", "STATE", "PID", "AGENT", "SOURCE", "LAST ACTIVE", "CWD"];
    let mut table = vec![head.map(String::from)];
    let cell = |v: &Value| v.as_str().unwrap_or("-").to_owned();
    for r in &rows {
        table.push([
            cell(&r["session"]),
            cell(&r["title"]),
            cell(&r["state"]),
            r["pid"].as_u64().map_or("-".to_owned(), |p| p.to_string()),
            cell(&r["agent"]),
            cell(&r["source"]),
            r["last_active"].as_str().map_or("-".to_owned(), when),
            cell(&r["cwd"]),
        ]);
    }
    print_table(table);
    Ok(ExitCode::SUCCESS)
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
/// locks of, `unreachable`, ADR 3), then those it has transcripts of that
/// none has open, `inactive`.
fn brnr_rows(hosts: &[Host]) -> Vec<Value> {
    let locks = lock::all();
    let mut rows = Vec::new();
    for host in hosts {
        let pid = host.id().parse::<u64>().ok();
        let agent = agent_name(&host.info()["agent"]);
        for s in host.sessions() {
            rows.push(json!({
                "session": s["session_id"],
                "title": s["title"],
                "state": s["state"],
                "pid": pid,
                "agent": agent,
                "source": "brnr",
                "last_active": s["last_active"],
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
                "cwd": null,
            }));
        }
    }
    for p in inactive_sessions(hosts) {
        rows.push(json!({
            "session": p["session_id"],
            "title": null,
            "state": "inactive",
            "pid": null,
            "agent": agent_name(&p["agent"]),
            "source": "brnr",
            "last_active": p["last_active"],
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
    /// of ACP other than brnr's fails, with nothing more asked (ADR 54).
    pub(super) fn start(agent: &[String]) -> Result<(Asked, AgentCapabilities), String> {
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
