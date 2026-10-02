//! Starting sessions and talking to them: `start`, `send`, `wait`,
//! `cancel` and `queue`.
//!
//! `start --wait` and `send --wait` print the agent's reply and exit with the
//! turn's result; `wait` waits for a session to be idle (or for the next
//! turn, a permission request, or the host's exit). Exit status: 0 when the
//! turn ended normally (`end_turn`), 1 if it failed or stopped for another
//! reason, 124 on `--timeout`.

use std::collections::VecDeque;
use std::env;
use std::fs;
use std::io::{self, BufRead, BufReader, ErrorKind, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{ExitCode, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use brnr::{config, paths, spawn};

use super::{
    Host, START_GRACE, START_TIMEOUT, USAGE, call, connect, discover, inactive_sessions,
    read_stdin, resolve,
};

/// Images bigger than this aren't sent: the whole prompt is one JSON line.
const MAX_IMAGE: u64 = 10 << 20;

/// Exit status for `--timeout`, as timeout(1) has it.
const TIMED_OUT: u8 = 124;

// ---- a connection that subscribes and calls ------------------------------

/// One control connection that is subscribed to events and also makes
/// requests: responses are matched by `req_id`, and events that arrive in
/// between are kept for `next_event`.
pub(super) struct Conn {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
    events: VecDeque<Value>,
    next_req: u64,
}

impl Conn {
    pub(super) fn open(host: &Host) -> Result<Conn, String> {
        let stream = connect(host).map_err(|e| format!("host {}: {e}", host.id()))?;
        let writer = stream.try_clone().map_err(|e| e.to_string())?;
        Ok(Conn { reader: BufReader::new(stream), writer, events: VecDeque::new(), next_req: 0 })
    }

    pub(super) fn subscribe(&mut self, events: &[&str]) -> Result<(), String> {
        self.call(json!({ "cmd": "subscribe", "events": events })).map(drop)
    }

    /// A request whose failure is an error.
    pub(super) fn call(&mut self, mut req: Value) -> Result<Value, String> {
        self.next_req += 1;
        let id = format!("r{}", self.next_req);
        req["req_id"] = json!(id);
        writeln!(self.writer, "{req}").map_err(|e| e.to_string())?;
        loop {
            let msg = self.read(None)?.ok_or("the host closed the connection")?;
            if msg["req_id"] == id.as_str() {
                if msg["ok"].as_bool() != Some(true) {
                    return Err(msg["error"].as_str().unwrap_or("request failed").to_owned());
                }
                return Ok(msg);
            }
            if msg.get("event").is_some() {
                self.events.push_back(msg);
            }
        }
    }

    /// The next event; `Ok(None)` when `deadline` passes, `Err` when the
    /// connection closes.
    pub(super) fn next_event(
        &mut self,
        deadline: Option<Instant>,
    ) -> Result<Option<Value>, String> {
        if let Some(event) = self.events.pop_front() {
            return Ok(Some(event));
        }
        loop {
            match self.read(deadline)? {
                None => return Ok(None),
                Some(msg) if msg.get("event").is_some() => return Ok(Some(msg)),
                Some(_) => {} // A response nobody waits for.
            }
        }
    }

    fn read(&mut self, deadline: Option<Instant>) -> Result<Option<Value>, String> {
        let timeout = deadline.map(|d| d.saturating_duration_since(Instant::now()));
        if timeout == Some(Duration::ZERO) {
            return Ok(None);
        }
        self.reader.get_ref().set_read_timeout(timeout).map_err(|e| e.to_string())?;
        let mut line = String::new();
        match self.reader.read_line(&mut line) {
            Ok(0) => Err("the host closed the connection".into()),
            Ok(_) => Ok(Some(serde_json::from_str(&line).unwrap_or(Value::Null))),
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }
}

// ---- waiting for a turn --------------------------------------------------

/// Events `wait_for_message` needs.
const TURN_EVENTS: &[&str] =
    &["user_message", "agent_message", "permission_request", "turn_ended", "exited"];

/// Prints the agent's messages in the turn that answers `message` until it
/// ends, and returns the turn's exit status. Permission requests on the way
/// are announced on stderr, with how to answer them.
pub(super) fn wait_for_message(
    conn: &mut Conn,
    target: &str,
    session: &str,
    message: &str,
    deadline: Option<Instant>,
) -> Result<ExitCode, String> {
    let mut started = false;
    loop {
        let Some(e) = conn.next_event(deadline)? else {
            eprintln!("brnr: timed out waiting for the reply");
            return Ok(ExitCode::from(TIMED_OUT));
        };
        let ours = e["session"] == session;
        match e["event"].as_str().unwrap_or_default() {
            "user_message" if e["message"] == message => started = true,
            "agent_message" if ours && started => {
                println!("{}", e["text"].as_str().unwrap_or_default());
                io::stdout().flush().ok();
            }
            "permission_request" if ours => {
                let request = e["request"].as_str().unwrap_or("?");
                eprintln!(
                    "brnr: waiting for permission {request}: {} (brnr show {target} {request}; brnr approve {target} {request})",
                    e["title"].as_str().unwrap_or("?")
                );
            }
            "turn_ended" if e["message"] == message => return Ok(turn_status(&e)),
            "exited" => return Err("the agent exited before the turn ended".into()),
            _ => {}
        }
    }
}

/// 0 for a turn that ended normally, else 1 (and why, on stderr).
fn turn_status(e: &Value) -> ExitCode {
    if let Some(error) = e["error"].as_object() {
        eprintln!("brnr: turn failed: {}", error["message"].as_str().unwrap_or("?"));
        return ExitCode::FAILURE;
    }
    match e["stop_reason"].as_str() {
        Some("end_turn") => ExitCode::SUCCESS,
        reason => {
            eprintln!("brnr: turn stopped: {}", reason.unwrap_or("?"));
            ExitCode::FAILURE
        }
    }
}

fn deadline(timeout: Option<u64>) -> Option<Instant> {
    timeout.map(|s| Instant::now() + Duration::from_secs(s))
}

fn seconds(flag: &str, value: &str) -> Result<u64, String> {
    value.parse().map_err(|_| format!("{flag}: not a number of seconds: {value}"))
}

// ---- attachments ---------------------------------------------------------

/// `--file` as a resource link and `--image` as an image block.
fn attachments(files: &[String], images: &[String]) -> Result<Vec<Value>, String> {
    let mut blocks = Vec::new();
    for file in files {
        let path = fs::canonicalize(paths::expand(file)).map_err(|e| format!("{file}: {e}"))?;
        let name = path.file_name().map_or(file.clone(), |n| n.to_string_lossy().into_owned());
        blocks.push(json!({ "type": "resource_link", "uri": file_uri(&path), "name": name }));
    }
    for image in images {
        let path = paths::expand(image);
        let mime = match path.extension().and_then(|e| e.to_str()).map(str::to_lowercase).as_deref()
        {
            Some("png") => "image/png",
            Some("jpg" | "jpeg") => "image/jpeg",
            Some("gif") => "image/gif",
            Some("webp") => "image/webp",
            _ => return Err(format!("{image}: not a png, jpeg, gif or webp image")),
        };
        let size = fs::metadata(&path).map_err(|e| format!("{image}: {e}"))?.len();
        if size > MAX_IMAGE {
            return Err(format!("{image}: {size} bytes; images go up to {MAX_IMAGE}"));
        }
        let data = fs::read(&path).map_err(|e| format!("{image}: {e}"))?;
        blocks.push(json!({ "type": "image", "mimeType": mime, "data": base64(&data) }));
    }
    Ok(blocks)
}

/// `file://` and the path, percent-encoded where a URI needs it.
fn file_uri(path: &Path) -> String {
    let mut uri = String::from("file://");
    for &b in path.as_os_str().as_encoded_bytes() {
        if b.is_ascii_alphanumeric() || b"/-._~".contains(&b) {
            uri.push(b as char);
        } else {
            uri.push_str(&format!("%{b:02X}"));
        }
    }
    uri
}

fn base64(data: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, &b)| n | (b as u32) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(TABLE[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

// ---- start ---------------------------------------------------------------

#[derive(Default)]
struct StartArgs {
    profile: Option<String>,
    name: Option<String>,
    cwd: Option<String>,
    prompt: Option<String>,
    files: Vec<String>,
    images: Vec<String>,
    mode: Option<String>,
    set: Vec<String>,
    permissions: Option<String>,
    resume: Option<String>,
    stop_when_idle: bool,
    wait: bool,
    timeout: Option<u64>,
    agent: Vec<String>,
}

fn parse_start(args: &[String]) -> Result<StartArgs, String> {
    let mut a = StartArgs::default();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let mut value = |flag: &str| it.next().cloned().ok_or(format!("{flag} needs a value"));
        match arg.as_str() {
            "--profile" => a.profile = Some(value("--profile")?),
            "--name" => a.name = Some(value("--name")?),
            "--cwd" => a.cwd = Some(value("--cwd")?),
            "--prompt" => a.prompt = Some(value("--prompt")?),
            "--file" => a.files.push(value("--file")?),
            "--image" => a.images.push(value("--image")?),
            "--mode" => a.mode = Some(value("--mode")?),
            "--model" => a.set.push(format!("model={}", value("--model")?)),
            "--set" => a.set.push(value("--set")?),
            "--permissions" => a.permissions = Some(value("--permissions")?),
            "--resume" => a.resume = Some(value("--resume")?),
            "--timeout" => a.timeout = Some(seconds("--timeout", &value("--timeout")?)?),
            "--stop-when-idle" => a.stop_when_idle = true,
            "--wait" => a.wait = true,
            "--" => {
                a.agent = it.by_ref().cloned().collect();
                break;
            }
            other => return Err(format!("unknown option: {other}")),
        }
    }
    if a.prompt.as_deref() == Some("-") {
        a.prompt = Some(read_stdin()?);
    }
    if a.prompt.as_deref().is_some_and(|p| p.trim().is_empty()) {
        return Err("--prompt is empty".into());
    }
    if let Some(bad) = a.set.iter().find(|s| !s.contains('=')) {
        return Err(format!("--set takes <option>=<value>, not {bad}"));
    }
    let has_prompt = a.prompt.is_some() || !a.files.is_empty() || !a.images.is_empty();
    if a.wait && !has_prompt {
        return Err("--wait needs a --prompt (or --file, --image)".into());
    }
    Ok(a)
}

pub(super) fn start(args: &[String]) -> Result<ExitCode, String> {
    let mut a = parse_start(args)?;
    let blocks = attachments(&a.files, &a.images)?;
    let timeout = match env::var("BRNR_START_TIMEOUT") {
        Ok(secs) => secs.parse().map_err(|_| format!("BRNR_START_TIMEOUT: not seconds: {secs}"))?,
        Err(_) => START_TIMEOUT,
    };
    let mut resume_cwd = None;
    if let Some(wanted) = a.resume.clone() {
        let past = resumable(&wanted)?;
        a.resume = past["session_id"].as_str().map(str::to_owned);
        resume_cwd = past["cwd"].as_str().map(str::to_owned);
        if a.agent.is_empty() {
            a.agent = past["agent"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|s| s.as_str().map(str::to_owned))
                .collect();
        }
        if a.name.is_none() {
            a.name = past["name"].as_str().map(str::to_owned);
        }
        if a.profile.is_none() {
            a.profile = past["profile"].as_str().map(str::to_owned);
        }
    }
    let cfg = config::load(a.profile.as_deref())?;
    let cwd = match a.cwd.clone().or(resume_cwd).or(cfg.cwd) {
        Some(dir) => paths::expand(&dir),
        None => env::current_dir().map_err(|e| format!("cwd: {e}"))?,
    };
    let cwd = std::path::absolute(&cwd).map_err(|e| format!("{}: {e}", cwd.display()))?;
    if !cwd.is_dir() {
        return Err(format!("{}: not a directory", cwd.display()));
    }

    let (ready_rx, ready_tx) = io::pipe().map_err(|e| format!("pipe: {e}"))?;
    let mut cmd = spawn::host_command().map_err(|e| e.to_string())?;
    cmd.arg("--ready-fd").arg("3").arg("--start-timeout").arg(timeout.to_string());
    for (flag, value) in [
        ("--profile", &a.profile),
        ("--name", &a.name),
        ("--mode", &a.mode),
        ("--permissions", &a.permissions),
        ("--resume", &a.resume),
    ] {
        if let Some(value) = value {
            cmd.arg(flag).arg(value);
        }
    }
    for set in &a.set {
        cmd.arg("--set").arg(set);
    }
    if a.stop_when_idle {
        cmd.arg("--stop-when-idle");
    }
    if !a.agent.is_empty() {
        cmd.arg("--").args(&a.agent);
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
    // Exiting closes the pipe, which tells a host that is still starting
    // that nobody is waiting: it stops instead of carrying on.
    let line = rx
        .recv_timeout(Duration::from_secs(timeout) + START_GRACE)
        .map_err(|_| "timed out waiting for the session".to_owned())?;
    let ready: Value =
        serde_json::from_str(&line).map_err(|_| "the host exited without starting a session")?;
    if ready["ok"].as_bool() != Some(true) {
        return Err(ready["error"].as_str().unwrap_or("start failed").to_owned());
    }
    let (id, session) = (s(&ready["id"]), s(&ready["session"]));
    let started = format!("started {id} (session {session})");
    if a.wait {
        eprintln!("{started}");
    } else {
        println!("{started}");
    }
    if a.prompt.is_none() && blocks.is_empty() {
        return Ok(ExitCode::SUCCESS);
    }

    // The prompt goes over the control socket, as `brnr send` sends it.
    let hosts = discover();
    let host = hosts.iter().find(|h| h.id() == id).ok_or("the host went away")?;
    let sent = (|| {
        let mut conn = Conn::open(host)?;
        if a.wait {
            conn.subscribe(TURN_EVENTS)?;
        }
        let req = json!({ "cmd": "send", "session": session, "text": a.prompt, "blocks": blocks });
        let message = s(&conn.call(req)?["message"]);
        Ok::<_, String>((conn, message))
    })();
    let (mut conn, message) = match sent {
        Ok(sent) => sent,
        Err(err) => {
            let _ = call(host, &json!({ "cmd": "stop" }));
            return Err(format!("sending the prompt: {err} (the host was stopped)"));
        }
    };
    if !a.wait {
        return Ok(ExitCode::SUCCESS);
    }
    wait_for_message(&mut conn, &id, &session, &message, deadline(a.timeout))
}

/// The inactive session `wanted` names (an id or unique prefix).
fn resumable(wanted: &str) -> Result<Value, String> {
    let hosts = discover();
    let running = hosts.iter().flat_map(|h| h.sessions().iter().map(move |s| (h, s)));
    for (host, s) in running {
        if s["session_id"].as_str().is_some_and(|id| id.starts_with(wanted)) {
            return Err(format!("{wanted} is running in host {}", host.id()));
        }
    }
    let past = inactive_sessions(&hosts);
    let matches: Vec<&Value> = past
        .iter()
        .filter(|p| p["session_id"].as_str().is_some_and(|id| id.starts_with(wanted)))
        .collect();
    match matches[..] {
        [one] => Ok(one.clone()),
        [] => Err(format!("no inactive session {wanted} (see brnr list --inactive)")),
        _ => Err(format!("{wanted} matches several sessions")),
    }
}

fn s(v: &Value) -> String {
    v.as_str().unwrap_or("?").to_owned()
}

// ---- send ----------------------------------------------------------------

pub(super) fn send(args: &[String]) -> Result<ExitCode, String> {
    let mut target = None;
    let mut session = None;
    let mut mode = None;
    let mut replace = false;
    let mut wait = false;
    let mut timeout = None;
    let mut files = Vec::new();
    let mut images = Vec::new();
    let mut words = Vec::new();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let set_mode = |mode: &mut Option<&str>, m| match mode.replace(m) {
            Some(prev) if prev != m => Err(format!("--{prev} and --{m} don't go together")),
            _ => Ok(()),
        };
        let mut value = |flag: &str| it.next().cloned().ok_or(format!("{flag} needs a value"));
        match arg.as_str() {
            "--session" => session = Some(value("--session")?),
            "--after-turn" => set_mode(&mut mode, "after-turn")?,
            "--interrupt" => set_mode(&mut mode, "interrupt")?,
            "--context" => set_mode(&mut mode, "context")?,
            "--replace" => replace = true,
            "--wait" => wait = true,
            "--timeout" => timeout = Some(seconds("--timeout", &value("--timeout")?)?),
            "--file" => files.push(value("--file")?),
            "--image" => images.push(value("--image")?),
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
    if wait && mode == Some("context") {
        return Err("--wait needs a turn; --context doesn't start one".into());
    }
    let text = if words == ["-"] { read_stdin()? } else { words.join(" ") };
    let blocks = attachments(&files, &images)?;
    if text.trim().is_empty() && blocks.is_empty() {
        return Err("nothing to send".into());
    }

    let hosts = discover();
    let (host, matched) = resolve(&hosts, &target)?;
    let req = json!({
        "cmd": "send",
        "session": session.or(matched),
        "text": text,
        "blocks": blocks,
        "mode": mode.unwrap_or("now"),
        "replace": replace,
    });
    if !wait {
        let response = call(host, &req)?;
        let message =
            response["message"].as_str().map(|m| format!(", message {m}")).unwrap_or_default();
        println!("{} (session {}{message})", s(&response["status"]), s(&response["session"]));
        return Ok(ExitCode::SUCCESS);
    }
    let mut conn = Conn::open(host)?;
    conn.subscribe(TURN_EVENTS)?;
    let response = conn.call(req)?;
    eprintln!(
        "{} (session {}, message {})",
        s(&response["status"]),
        s(&response["session"]),
        s(&response["message"])
    );
    wait_for_message(
        &mut conn,
        &target,
        &s(&response["session"]),
        &s(&response["message"]),
        deadline(timeout),
    )
}

// ---- wait ----------------------------------------------------------------

pub(super) fn wait(args: &[String]) -> Result<ExitCode, String> {
    let mut target = None;
    let mut session = None;
    let mut what = "idle".to_owned();
    let mut timeout = None;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let mut value = |flag: &str| it.next().cloned().ok_or(format!("{flag} needs a value"));
        match arg.as_str() {
            "--for" => what = value("--for")?,
            "--session" => session = Some(value("--session")?),
            "--timeout" => timeout = Some(seconds("--timeout", &value("--timeout")?)?),
            flag if flag.starts_with("--") => return Err(format!("unknown option: {flag}")),
            _ if target.is_none() => target = Some(arg.clone()),
            _ => return Err(USAGE.to_owned()),
        }
    }
    if !matches!(what.as_str(), "idle" | "turn" | "permission" | "exit") {
        return Err(format!("--for idle|turn|permission|exit, not {what}"));
    }
    let target = target.ok_or(USAGE)?;
    let hosts = discover();
    let (host, matched) = resolve(&hosts, &target)?;
    let session = session.or(matched);
    let deadline = deadline(timeout);
    let mut conn = Conn::open(host)?;
    // Subscribed before looking, so nothing happens unseen in between.
    conn.subscribe(&["turn_ended", "permission_request", "exited"])?;
    let mine = |e: &Value| session.as_deref().is_none_or(|s| e["session"] == s);
    let timed_out = || {
        eprintln!("brnr: timed out");
        Ok(ExitCode::from(TIMED_OUT))
    };
    match what.as_str() {
        "permission" => {
            let pending = conn.call(json!({ "cmd": "pending" }))?;
            if let Some(p) = pending["pending"].as_array().into_iter().flatten().find(|p| mine(p)) {
                println!("{}", describe_permission(p));
                return Ok(ExitCode::SUCCESS);
            }
        }
        "idle" if idle(&mut conn, session.as_deref())? => {
            println!("idle");
            return Ok(ExitCode::SUCCESS);
        }
        _ => {}
    }
    loop {
        let e = match conn.next_event(deadline) {
            Ok(Some(e)) => e,
            Ok(None) => return timed_out(),
            Err(_) if what == "exit" => {
                println!("exited");
                return Ok(ExitCode::SUCCESS);
            }
            Err(e) => return Err(e),
        };
        match (e["event"].as_str().unwrap_or_default(), what.as_str()) {
            ("exited", "exit") => {
                println!("exited: {}", e["status"]);
                return Ok(ExitCode::SUCCESS);
            }
            ("exited", _) => return Err("the agent exited".into()),
            ("permission_request", "permission") if mine(&e) => {
                println!("{}", describe_permission(&e));
                return Ok(ExitCode::SUCCESS);
            }
            ("turn_ended", "turn") if mine(&e) => {
                println!("turn ended: {}", e["stop_reason"].as_str().unwrap_or("error"));
                return Ok(turn_status(&e));
            }
            ("turn_ended", "idle") if mine(&e) && idle(&mut conn, session.as_deref())? => {
                println!("idle: {}", e["stop_reason"].as_str().unwrap_or("error"));
                return Ok(turn_status(&e));
            }
            _ => {}
        }
    }
}

/// Whether the session (or every session) has no turn running and nothing
/// held.
fn idle(conn: &mut Conn, session: Option<&str>) -> Result<bool, String> {
    let status = conn.call(json!({ "cmd": "status" }))?;
    Ok(status["sessions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|s| session.is_none_or(|id| s["session_id"] == id))
        .all(|s| s["busy"] != true && s["held"].as_u64().unwrap_or(0) == 0))
}

fn describe_permission(p: &Value) -> String {
    format!("permission {}: {}", s(&p["request"]), s(&p["title"]))
}

// ---- cancel and queue ----------------------------------------------------

pub(super) fn cancel(args: &[String]) -> Result<ExitCode, String> {
    let mut target = None;
    let mut session = None;
    let mut keep_held = false;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--keep-held" => keep_held = true,
            "--session" => session = Some(it.next().ok_or("--session needs an id")?.clone()),
            flag if flag.starts_with("--") => return Err(format!("unknown option: {flag}")),
            _ if target.is_none() => target = Some(arg.clone()),
            _ => return Err(USAGE.to_owned()),
        }
    }
    let target = target.ok_or(USAGE)?;
    let hosts = discover();
    let (host, matched) = resolve(&hosts, &target)?;
    let req = json!({ "cmd": "cancel", "session": session.or(matched), "keep_held": keep_held });
    let response = call(host, &req)?;
    println!("{} (session {})", s(&response["status"]), s(&response["session"]));
    for held in response["dropped"].as_array().into_iter().flatten() {
        println!("dropped {}: {}", s(&held["message"]), s(&held["text"]));
    }
    Ok(ExitCode::SUCCESS)
}

pub(super) fn queue(args: &[String]) -> Result<ExitCode, String> {
    let mut target = None;
    let mut session = None;
    let mut req = json!({ "cmd": "queue" });
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--drop" => req["drop"] = json!(it.next().ok_or("--drop needs a message id")?),
            "--clear" => req["clear"] = json!(true),
            "--clear-context" => req["clear_context"] = json!(true),
            "--session" => session = Some(it.next().ok_or("--session needs an id")?.clone()),
            flag if flag.starts_with("--") => return Err(format!("unknown option: {flag}")),
            _ if target.is_none() => target = Some(arg.clone()),
            _ => return Err(USAGE.to_owned()),
        }
    }
    let target = target.ok_or(USAGE)?;
    let hosts = discover();
    let (host, matched) = resolve(&hosts, &target)?;
    req["session"] = json!(session.or(matched));
    let response = call(host, &req)?;
    for held in response["dropped"].as_array().into_iter().flatten() {
        println!("dropped {}: {}", s(&held["message"]), s(&held["text"]));
    }
    let held = response["held"].as_array().map_or(&[][..], Vec::as_slice);
    let context = response["context"].as_array().map_or(&[][..], Vec::as_slice);
    if held.is_empty() && context.is_empty() {
        println!("nothing held (session {})", s(&response["session"]));
    }
    for h in held {
        let how = if h["interrupt"] == true { "interrupt" } else { "after turn" };
        println!("{} ({how}): {}", s(&h["message"]), s(&h["text"]));
    }
    for c in context {
        println!("context: {}", c.as_str().unwrap_or("?"));
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_rfc4648() {
        for (raw, encoded) in
            [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foobar", "Zm9vYmFy")]
        {
            assert_eq!(base64(raw.as_bytes()), encoded);
        }
    }

    #[test]
    fn file_uris_are_encoded() {
        assert_eq!(file_uri(Path::new("/a b/c%d.rs")), "file:///a%20b/c%25d.rs");
    }
}
