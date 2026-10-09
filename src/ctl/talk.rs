//! Starting sessions and talking to them: `session new` and `resume`,
//! `prompt send`, `event wait`, `prompt cancel`, and `queue list` and `drop`.
//!
//! `session new --wait` and `send --wait` print the agent's reply and exit
//! with the turn's result; `wait` waits for a session to be idle (or for the
//! next turn, an approval, or its process's exit). Exit status: 0 when the
//! turn ended normally (`end_turn`), 1 if it failed or stopped for another
//! reason or its message was dropped (or, waiting for a turn or an approval,
//! the session closed first), 124 on `--timeout` (see ADR 21 in docs/adr).
//!
//! `send` (ADR 18 in docs/adr):
//! - default: a prompt and a turn of its own, sent now if no turn is
//!   running, else held and sent when the running one ends, in the order
//!   sent. brnr never sends a prompt while one runs.
//! - `--steer`: into the running turn (`_session/steering`), which then
//!   carries it; a prompt when no turn is running. Refused while one runs if
//!   the agent doesn't steer, and in strict mode.
//! - `--interrupt`: brnr cancels the running turn, then sends it, ahead of
//!   what is held.
//! - `--context`: no turn; appended to the next prompt, whoever sends it.
//!   `--replace` replaces the last held context instead of adding to it.

use std::collections::VecDeque;
use std::env;
use std::fs;
use std::io::{self, BufRead, BufReader, ErrorKind, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Child, ExitCode, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use brnr::request::{self, Request, Role};
use brnr::{config, lock, paths, signals, spawn, sys};

use super::{
    Found, Host, START_GRACE, START_TIMEOUT, USAGE, call, connect, discover, find_session,
    print_json, read_stdin, response_json, running_session, settings, text,
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
        let stream = connect(host).map_err(|e| format!("process {}: {e}", host.id()))?;
        Conn::new(stream)
    }

    /// One over `stream`: a control connection, or the start channel.
    fn new(stream: UnixStream) -> Result<Conn, String> {
        let writer = stream.try_clone().map_err(|e| e.to_string())?;
        Ok(Conn { reader: BufReader::new(stream), writer, events: VecDeque::new(), next_req: 0 })
    }

    /// Its connection, to look at without reading (`brnr event notify`).
    pub(super) fn socket(&self) -> io::Result<UnixStream> {
        self.writer.try_clone()
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
            let msg = self.read(None)?.ok_or("the process closed the connection")?;
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
        // macOS refuses it on a connection the process has closed (EINVAL),
        // where reading doesn't wait anyway: it reads what is left, then EOF.
        if let Err(e) = self.reader.get_ref().set_read_timeout(timeout)
            && e.raw_os_error() != Some(libc::EINVAL)
        {
            return Err(e.to_string());
        }
        let mut line = String::new();
        match self.reader.read_line(&mut line) {
            Ok(0) => Err("the process closed the connection".into()),
            Ok(_) => Ok(Some(serde_json::from_str(&line).unwrap_or(Value::Null))),
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }
}

// ---- waiting for a turn --------------------------------------------------

/// Events `wait_for_message` needs.
const TURN_EVENTS: &[&str] = &[
    "user_message",
    "agent_message",
    "permission_request",
    "turn_ended",
    "message_dropped",
    "session_closed",
    "exited",
];

/// Follows the turn that answers `message` until it ends, printing the
/// agent's messages as they come (or, with `json`, the turn as one object at
/// the end: `{session, message, reply, stop_reason, error, dropped}` and
/// `json`'s fields), and returns the turn's exit status: 1, too, for a message
/// dropped before it was sent (`dropped` says why). Approvals on the way are
/// announced on stderr, with how to answer them.
pub(super) fn wait_for_message(
    conn: &mut Conn,
    arg: &str,
    session: &str,
    message: &str,
    deadline: Option<Instant>,
    json: Option<Value>,
) -> Result<ExitCode, String> {
    let mut started = false;
    let mut reply: Vec<String> = Vec::new();
    loop {
        let Some(e) = conn.next_event(deadline)? else {
            eprintln!("brnr: timed out waiting for the reply");
            return Ok(ExitCode::from(TIMED_OUT));
        };
        let ours = e["session"] == session;
        match e["event"].as_str().unwrap_or_default() {
            "user_message" if e["message"] == message => started = true,
            "agent_message" if ours && started => {
                let text = e["text"].as_str().unwrap_or_default();
                if json.is_some() {
                    reply.push(text.to_owned());
                } else {
                    outln!("{text}");
                    io::stdout().flush().ok();
                }
            }
            "permission_request" if ours => {
                let request = e["request"].as_str().unwrap_or("?");
                errln!(
                    "brnr: waiting for approval {request}: {} (brnr permission show {arg} {request}; brnr approve {arg} {request})",
                    e["title"].as_str().unwrap_or("?")
                );
            }
            "turn_ended" if carries(&e, message) => {
                if let Some(out) = json {
                    print_turn(out, session, message, &reply, &e)?;
                }
                return Ok(turn_status(&e));
            }
            "message_dropped" if e["message"] == message => {
                if let Some(out) = json {
                    print_turn(out, session, message, &reply, &e)?;
                }
                errln!("brnr: {message} was dropped ({})", e["by"].as_str().unwrap_or("?"));
                return Ok(ExitCode::FAILURE);
            }
            "session_closed" if ours => {
                return Err("the session closed before the turn ended".into());
            }
            "exited" => return Err("the agent exited before the turn ended".into()),
            _ => {}
        }
    }
}

/// The turn as `--json` prints it, from the event that ended it
/// (`turn_ended`, or `message_dropped` for a message never sent): `out` with
/// `{session, message, reply, stop_reason, error, dropped}`.
fn print_turn(
    mut out: Value,
    session: &str,
    message: &str,
    reply: &[String],
    end: &Value,
) -> Result<(), String> {
    let dropped = if end["event"] == "message_dropped" { end["by"].clone() } else { Value::Null };
    for (k, v) in [
        ("session", json!(session)),
        ("message", json!(message)),
        ("reply", json!(reply.join("\n\n"))),
        ("stop_reason", end["stop_reason"].clone()),
        ("error", end["error"].clone()),
        ("dropped", dropped),
    ] {
        out[k] = v;
    }
    print_json(&out)
}

/// Whether `turn_ended` event `e` is of the turn that carried `message`.
fn carries(e: &Value, message: &str) -> bool {
    e["messages"].as_array().is_some_and(|m| m.iter().any(|m| m == message))
}

/// 0 for a turn that ended normally, else 1 (and why, on stderr).
fn turn_status(e: &Value) -> ExitCode {
    if let Some(error) = e["error"].as_object() {
        errln!("brnr: turn failed: {}", error["message"].as_str().unwrap_or("?"));
        return ExitCode::FAILURE;
    }
    match e["stop_reason"].as_str() {
        Some("end_turn") => ExitCode::SUCCESS,
        reason => {
            errln!("brnr: turn stopped: {}", reason.unwrap_or("?"));
            ExitCode::FAILURE
        }
    }
}

/// When `--timeout` runs out; never, for one too far off to say.
fn deadline(timeout: Option<u64>) -> Option<Instant> {
    timeout.and_then(|s| Instant::now().checked_add(Duration::from_secs(s)))
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

// ---- session new and resume ---------------------------------------------

// A start, `session new` or `session resume`, is atomic (ADR 7 in
// docs/adr). Everything it needs is read and resolved before the process is
// launched, and handed to it in one request on its stdin (ADR 8). The
// process reports on the start channel, a socketpair at its fd 3: the start
// commits at its ready report, after which the process sends the prompt
// itself. Until then, the command going away (or giving up) stops it, and
// the prompt is never sent.

#[derive(Default)]
struct StartArgs {
    profile: Option<String>,
    cwd: Option<String>,
    prompt: Option<String>,
    files: Vec<String>,
    images: Vec<String>,
    /// `--mode`, `--model`, `--thought-level` and `--option`.
    settings: request::Settings,
    /// The session `resume` resumes; none for `new`.
    resume: Option<String>,
    take_over: bool,
    stop_when_idle: Option<u64>,
    permission_timeout: Option<u64>,
    auth: Option<String>,
    strict: bool,
    wait: bool,
    timeout: Option<u64>,
    foreground: bool,
    quiet: bool,
    json: bool,
    agent: Vec<String>,
}

/// `session new`'s flags, or (`resume`) `session resume`'s, which also
/// takes the session and `--take-over`.
fn parse_start(args: &[String], resume: bool) -> Result<StartArgs, String> {
    let mut a = StartArgs::default();
    let mut pairs = Vec::new();
    let mut by_category: [(&str, Option<String>); 3] =
        [("mode", None), ("model", None), ("thought-level", None)];
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let flag = arg.strip_prefix("--");
        if let Some((name, set)) = by_category.iter_mut().find(|(name, _)| Some(*name) == flag) {
            let value = it.next().ok_or(format!("--{name} needs a value"))?;
            if let Some(was) = set.replace(value.clone())
                && was != *value
            {
                return Err(format!("--{name} {was} and --{name} {value} disagree"));
            }
            continue;
        }
        let mut value = |flag: &str| it.next().cloned().ok_or(format!("{flag} needs a value"));
        match arg.as_str() {
            "--profile" => a.profile = Some(value("--profile")?),
            "--cwd" => a.cwd = Some(value("--cwd")?),
            "--prompt" => a.prompt = Some(value("--prompt")?),
            "--file" => a.files.push(value("--file")?),
            "--image" => a.images.push(value("--image")?),
            "--option" => pairs.push(value("--option")?),
            "--take-over" if resume => a.take_over = true,
            "--take-over" => return Err("--take-over goes with session resume".into()),
            "--auth" => a.auth = Some(value("--auth")?),
            "--timeout" => a.timeout = Some(seconds("--timeout", &value("--timeout")?)?),
            "--stop-when-idle" => {
                a.stop_when_idle = Some(seconds("--stop-when-idle", &value("--stop-when-idle")?)?);
            }
            "--permission-timeout" => {
                let secs = value("--permission-timeout")?;
                a.permission_timeout = Some(seconds("--permission-timeout", &secs)?);
            }
            "--strict" => a.strict = true,
            "--wait" => a.wait = true,
            "--foreground" => a.foreground = true,
            "--quiet" => a.quiet = true,
            "--json" => a.json = true,
            "--" => {
                a.agent = it.by_ref().cloned().collect();
                break;
            }
            other if other.starts_with('-') && other != "-" => {
                return Err(format!("unknown option: {other}"));
            }
            _ if resume && a.resume.is_none() => a.resume = Some(arg.clone()),
            _ => return Err(USAGE.to_owned()),
        }
    }
    if resume && a.resume.is_none() {
        return Err(USAGE.to_owned());
    }
    let [(_, mode), (_, model), (_, thought_level)] = by_category;
    let options = settings::options("--option", &pairs)?;
    a.settings = request::Settings { mode, model, thought_level, options };
    if a.prompt.as_deref() == Some("-") {
        a.prompt = Some(read_stdin()?);
    }
    if a.prompt.as_deref().is_some_and(|p| p.trim().is_empty()) {
        return Err("--prompt is empty".into());
    }
    let has_prompt = a.prompt.is_some() || !a.files.is_empty() || !a.images.is_empty();
    if a.wait && !has_prompt {
        return Err("--wait needs a --prompt (or --file, --image)".into());
    }
    if a.wait && a.foreground {
        return Err("--wait and --foreground don't go together".into());
    }
    if a.quiet && !a.foreground {
        return Err("--quiet goes with --foreground".into());
    }
    if a.timeout.is_some() && !a.wait {
        return Err("--timeout goes with --wait".into());
    }
    Ok(a)
}

/// `brnr session new`: a process, and the new session it opens.
pub(super) fn new(args: &[String]) -> Result<ExitCode, String> {
    start(parse_start(args, false)?)
}

/// `brnr session resume`: a process, and the session it resumes (ADR 14).
pub(super) fn resume(args: &[String]) -> Result<ExitCode, String> {
    start(parse_start(args, true)?)
}

fn start(mut a: StartArgs) -> Result<ExitCode, String> {
    let command = if a.resume.is_some() { "session resume" } else { "session new" };
    let blocks = attachments(&a.files, &a.images)?;
    let timeout = match env::var("BRNR_START_TIMEOUT") {
        Ok(secs) => secs.parse().map_err(|_| format!("BRNR_START_TIMEOUT: not seconds: {secs}"))?,
        Err(_) => START_TIMEOUT,
    };
    let mut resume_cwd = None;
    let hosts = if a.resume.is_some() { discover()? } else { Vec::new() };
    // --take-over: the process holding the session, and the session. It
    // closes the session once the process to resume it has started, so it
    // can say which (ADR 3, ADR 4).
    let mut owner = None;
    let mut transcript = false;
    if let Some(wanted) = a.resume.clone() {
        // A session another process holds is refused, naming the process,
        // unless --take-over. The process started here takes the lock itself,
        // and is refused the same way if it has been taken meanwhile.
        let found = match lock::holder(&wanted) {
            Some(pid) if !a.take_over => {
                return Err(format!(
                    "{wanted} is running in process {pid} (--take-over closes it there and \
                     resumes it here)"
                ));
            }
            Some(pid) => {
                let (host, running) = settings::held(&hosts, &wanted, pid)?;
                owner = Some((host, wanted.clone()));
                Some(running)
            }
            // A session brnr has no transcript of (one `brnr sessions` lists)
            // goes to the agent as given, in --cwd or here, with -- <agent>
            // or the profile's.
            None => match find_session(&hosts, &wanted) {
                // Served without its lock, shared by an editor's process.
                Ok(Found::Running(host, _)) => {
                    return Err(format!("{wanted} is running in process {}", host.id()));
                }
                Ok(Found::Inactive(past)) => Some(past),
                Err(_) => None,
            },
        };
        transcript = found.is_some();
        if let Some(past) = found {
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
            if a.profile.is_none() {
                a.profile = past["profile"].as_str().map(str::to_owned);
            }
        }
    }
    let cfg = config::load(a.profile.as_deref())?;
    let h = &cfg.headless;
    let cwd = match a.cwd.clone().or(resume_cwd).or(h.cwd.clone()) {
        Some(dir) => paths::expand(&dir),
        None => env::current_dir().map_err(|e| format!("cwd: {e}"))?,
    };
    let cwd = std::path::absolute(&cwd).map_err(|e| format!("{}: {e}", cwd.display()))?;
    if !cwd.is_dir() {
        return Err(format!("{}: not a directory", cwd.display()));
    }
    let mcp_servers =
        h.mcp_servers.iter().map(config::McpServer::to_acp).collect::<Result<Vec<_>, _>>()?;
    let defaults = request::Settings {
        mode: h.mode.clone(),
        model: h.model.clone(),
        thought_level: h.thought_level.clone(),
        options: h.options.clone(),
    };
    let has_prompt = a.prompt.is_some() || !blocks.is_empty();
    let prompt = has_prompt.then(|| request::Prompt { text: a.prompt.unwrap_or_default(), blocks });
    let events =
        if a.wait { TURN_EVENTS.iter().map(|e| e.to_string()).collect() } else { Vec::new() };
    let headless = request::Headless {
        resume: a.resume,
        transcript,
        settings: a.settings,
        defaults,
        mcp_servers,
        auth: a.auth.or(h.auth.clone()),
        prompt,
        start_timeout: timeout,
        stop_when_idle: a.stop_when_idle.or(h.stop_when_idle),
        permission_timeout: a.permission_timeout.or(h.permission_timeout),
        events,
        foreground: a.foreground.then_some(request::Foreground { quiet: a.quiet, json: a.json }),
    };
    let role = Role::Headless(Box::new(headless));
    let mut request = Request::new(a.profile, &cfg, a.agent, cwd.clone(), role)?;
    request.strict |= a.strict;

    let (channel, theirs) = UnixStream::pair().map_err(|e| format!("socketpair: {e}"))?;
    let mut cmd = spawn::host_command().map_err(|e| e.to_string())?;
    cmd.current_dir(&cwd).stdin(Stdio::piped());
    let mut child = None;
    let (stdin, pid) = if a.foreground {
        // Our child, in a group of its own, printing the session to our
        // stdout; we pass it the signals we get (Ctrl-C), once.
        let signals = signals::install();
        let mut started = spawn::child(&mut cmd, &[(theirs.as_raw_fd(), 3)])
            .map_err(|e| format!("starting the process: {e}"))?;
        let pid = started.id();
        thread::spawn(move || forward_signals(signals, pid as i32));
        let stdin = started.stdin.take();
        child = Some(started);
        (stdin, pid)
    } else {
        cmd.stdout(Stdio::null()).stderr(Stdio::null());
        spawn::detached(&mut cmd, &[(theirs.as_raw_fd(), 3)])
            .map_err(|e| format!("starting the process: {e}"))?
    };
    drop(theirs);
    // The process waits for its request meanwhile; if the session can't be
    // taken over, it goes without one, having done nothing.
    if let Some((owner, session)) = owner
        && let Err(err) = settings::take_over(owner, &session, pid)
    {
        sys::kill(pid as i32, libc::SIGKILL);
        if let Some(mut child) = child {
            let _ = child.wait();
        }
        return Err(err);
    }
    request.send(stdin.expect("piped")).map_err(|e| format!("starting the process: {e}"))?;

    // Exiting closes the channel, which tells a process that is still
    // starting that nobody is waiting: it stops instead of carrying on. It
    // fails the start itself when its timeout passes; this is for a process
    // stuck too badly to say so.
    let mut conn = Conn::new(channel)?;
    let fallback =
        Instant::now().checked_add(Duration::from_secs(timeout).saturating_add(START_GRACE));
    let ready = loop {
        match conn.read(fallback) {
            Ok(Some(msg)) if msg.get("event").is_some() => {} // Before the report: not ours.
            Ok(Some(msg)) => break msg,
            Ok(None) => return Err("timed out waiting for the session".into()),
            Err(_) => break Value::Null,
        }
    };
    if ready["ok"].as_bool() != Some(true) {
        if let Some(child) = child {
            return Ok(exit_status(child)); // It has said why, on our stderr.
        }
        let error = ready["error"].as_str().unwrap_or("the process exited without a session");
        if brnr::bug::is_panic(error) {
            // A detached process has no terminal to say where to report it.
            errln!("brnr: {error}");
            eprintln!("{}", brnr::bug::link(error, command));
            return Ok(ExitCode::FAILURE);
        }
        return Err(error.to_owned());
    }
    if let Some(child) = child {
        return Ok(exit_status(child));
    }
    let session = text(&ready["session"]);
    // The prompt's id, as `send` gives it (ADR 17); null without one.
    let about = json!({ "session": session, "pid": ready["pid"], "message": ready["message"] });
    if !a.wait {
        if a.json {
            print_json(&about)?;
        } else {
            outln!("{}", describe_started(&about));
        }
        return Ok(ExitCode::SUCCESS);
    }
    if !a.json {
        errln!("{}", describe_started(&about));
    }
    // The turn's events follow the report on the same channel.
    let (message, json_out) = (text(&ready["message"]), a.json.then_some(about));
    wait_for_message(&mut conn, &session, &session, &message, deadline(a.timeout), json_out)
}

/// `started sess-1 (process 4466)`.
fn describe_started(about: &Value) -> String {
    format!("started {} (process {})", text(&about["session"]), about["pid"])
}

/// `session new --foreground`: the signals we get go to the process we
/// started.
fn forward_signals(mut signals: io::PipeReader, pid: i32) {
    use std::io::Read;
    let mut sig = [0];
    while matches!(signals.read(&mut sig), Ok(1)) {
        sys::kill(pid, sig[0] as i32);
    }
}

/// Waits for the process `session new --foreground` started and exits as it
/// did.
fn exit_status(mut child: Child) -> ExitCode {
    use std::os::unix::process::ExitStatusExt;
    match child.wait() {
        Ok(status) => match (status.code(), status.signal()) {
            (Some(code), _) => ExitCode::from(code as u8),
            (_, Some(sig)) => ExitCode::from(128 + sig as u8),
            _ => ExitCode::FAILURE,
        },
        Err(_) => ExitCode::FAILURE,
    }
}

// ---- send ----------------------------------------------------------------

pub(super) fn send(args: &[String]) -> Result<ExitCode, String> {
    let mut arg = None;
    let mut mode = None;
    let (mut replace, mut wait, mut json_out) = (false, false, false);
    let mut timeout = None;
    let mut files = Vec::new();
    let mut images = Vec::new();
    let mut words = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let set_mode = |mode: &mut Option<&str>, m| match mode.replace(m) {
            Some(prev) if prev != m => Err(format!("--{prev} and --{m} don't go together")),
            _ => Ok(()),
        };
        let mut value = |flag: &str| it.next().cloned().ok_or(format!("{flag} needs a value"));
        match a.as_str() {
            "--steer" => set_mode(&mut mode, "steer")?,
            "--interrupt" => set_mode(&mut mode, "interrupt")?,
            "--context" => set_mode(&mut mode, "context")?,
            "--replace" => replace = true,
            "--wait" => wait = true,
            "--json" => json_out = true,
            "--timeout" => timeout = Some(seconds("--timeout", &value("--timeout")?)?),
            "--file" => files.push(value("--file")?),
            "--image" => images.push(value("--image")?),
            "--" => words.extend(it.by_ref().cloned()),
            flag if flag.starts_with("--") => return Err(format!("unknown option: {flag}")),
            _ if arg.is_none() => arg = Some(a.clone()),
            _ => words.push(a.clone()),
        }
    }
    let arg = arg.ok_or(USAGE)?;
    if replace && mode != Some("context") {
        return Err("--replace only applies to --context".into());
    }
    if wait && mode == Some("context") {
        return Err("--wait needs a turn; --context doesn't start one".into());
    }
    let text_in = if words == ["-"] { read_stdin()? } else { words.join(" ") };
    let blocks = attachments(&files, &images)?;
    if text_in.trim().is_empty() && blocks.is_empty() {
        return Err("nothing to send".into());
    }

    let hosts = discover()?;
    let (host, session) = running_session(&hosts, &arg)?;
    let req = json!({
        "cmd": "send",
        "session": session,
        "text": text_in,
        "blocks": blocks,
        "mode": mode.unwrap_or("prompt"),
        "replace": replace,
    });
    if !wait {
        let response = call(host, &req)?;
        if json_out {
            print_json(&response_json(response))?;
        } else {
            outln!("{}", describe_sent(&response));
        }
        return Ok(ExitCode::SUCCESS);
    }
    let mut conn = Conn::open(host)?;
    conn.subscribe(TURN_EVENTS)?;
    let response = conn.call(req)?;
    if !json_out {
        errln!("{}", describe_sent(&response));
    }
    let message = text(&response["message"]);
    wait_for_message(
        &mut conn,
        &arg,
        &session,
        &message,
        deadline(timeout),
        json_out.then(|| json!({})),
    )
}

/// `delivered (message m1)`, `held`, …
fn describe_sent(response: &Value) -> String {
    let message =
        response["message"].as_str().map(|m| format!(" (message {m})")).unwrap_or_default();
    format!("{}{message}", text(&response["status"]))
}

// ---- wait ----------------------------------------------------------------

pub(super) fn wait(args: &[String]) -> Result<ExitCode, String> {
    let mut arg = None;
    let mut what = "idle".to_owned();
    let mut timeout = None;
    let mut json_out = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = |flag: &str| it.next().cloned().ok_or(format!("{flag} needs a value"));
        match a.as_str() {
            "--for" => what = value("--for")?,
            "--timeout" => timeout = Some(seconds("--timeout", &value("--timeout")?)?),
            "--json" => json_out = true,
            flag if flag.starts_with("--") => return Err(format!("unknown option: {flag}")),
            _ if arg.is_none() => arg = Some(a.clone()),
            _ => return Err(USAGE.to_owned()),
        }
    }
    if !matches!(what.as_str(), "idle" | "turn" | "permission" | "exit") {
        return Err(format!("--for idle|turn|permission|exit, not {what}"));
    }
    let arg = arg.ok_or(USAGE)?;
    let hosts = discover()?;
    let (host, session) = running_session(&hosts, &arg)?;
    let deadline = deadline(timeout);
    let mut conn = Conn::open(host)?;
    // Subscribed before looking, so nothing happens unseen in between.
    conn.subscribe(&["turn_ended", "permission_request", "session_closed", "exited"])?;
    let mine = |e: &Value| e["session"] == session.as_str();
    // What ended the wait: printed as text, or with --json as itself.
    let done = |what: &Value, text: String| {
        if json_out {
            return print_json(what);
        }
        outln!("{text}");
        Ok(())
    };
    let idle_now = json!({ "event": "idle", "session": session });
    // How the last turn ended: a wait for `idle` that ends on an idle or a
    // closed session exits as if it had waited for that turn (see ADR 21 in
    // docs/adr).
    let mut last_turn = Value::Null;
    match what.as_str() {
        "permission" => {
            let pending = conn.call(json!({ "cmd": "pending" }))?;
            if let Some(p) = pending["pending"].as_array().into_iter().flatten().find(|p| mine(p)) {
                done(p, describe_permission(p))?;
                return Ok(ExitCode::SUCCESS);
            }
        }
        "idle" => {
            let (is_idle, s) = idle(&mut conn, &session)?;
            if is_idle {
                done(&idle_now, "idle".into())?;
                return Ok(ended_as(&s["last_turn"]));
            }
            last_turn = s["last_turn"].clone();
        }
        _ => {}
    }
    loop {
        let e = match conn.next_event(deadline) {
            Ok(Some(e)) => e,
            Ok(None) => {
                eprintln!("brnr: timed out");
                return Ok(ExitCode::from(TIMED_OUT));
            }
            Err(_) if what == "exit" => {
                done(&json!({ "event": "exited", "status": null }), "exited".into())?;
                return Ok(ExitCode::SUCCESS);
            }
            Err(e) => return Err(e),
        };
        match (e["event"].as_str().unwrap_or_default(), what.as_str()) {
            ("exited", "exit") => {
                done(&e, format!("exited: {}", e["status"]))?;
                return Ok(ExitCode::SUCCESS);
            }
            ("exited", _) => return Err("the agent exited".into()),
            // A closed session counts as idle; there is no next turn or
            // approval in it to wait for.
            ("session_closed", "idle") if mine(&e) => {
                done(&e, format!("idle: session closed ({})", text(&e["by"])))?;
                return Ok(ended_as(&last_turn));
            }
            ("session_closed", "turn" | "permission") if mine(&e) => {
                return Err("the session closed".into());
            }
            ("permission_request", "permission") if mine(&e) => {
                done(&e, describe_permission(&e))?;
                return Ok(ExitCode::SUCCESS);
            }
            ("turn_ended", "turn") if mine(&e) => {
                done(&e, format!("turn ended: {}", e["stop_reason"].as_str().unwrap_or("error")))?;
                return Ok(turn_status(&e));
            }
            ("turn_ended", "idle") if mine(&e) => {
                last_turn = e.clone();
                if idle(&mut conn, &session)?.0 {
                    // Behind the session, the turns that ended after this
                    // one are queued, before the status that says it is
                    // idle: it went idle after the last of them.
                    let ended = |q: &&Value| q["event"] == "turn_ended" && mine(q);
                    let e = conn.events.iter().rev().find(ended).cloned().unwrap_or(e);
                    done(&e, format!("idle: {}", e["stop_reason"].as_str().unwrap_or("error")))?;
                    return Ok(turn_status(&e));
                }
            }
            _ => {}
        }
    }
}

/// Whether the session is idle, with no turn running and nothing held (a
/// closed one counts), and its status (null for one that has closed).
fn idle(conn: &mut Conn, session: &str) -> Result<(bool, Value), String> {
    let status = conn.call(json!({ "cmd": "status" }))?;
    let sessions = status["sessions"].as_array().map_or(&[][..], Vec::as_slice);
    Ok(match sessions.iter().find(|s| s["session_id"] == session) {
        Some(s) => (s["busy"] != true && s["held"].as_u64().unwrap_or(0) == 0, s.clone()),
        None => (true, Value::Null),
    })
}

/// The exit status of a turn that ended as `turn` says (`turn_ended`, or
/// status's `last_turn`); 0 when there was none.
fn ended_as(turn: &Value) -> ExitCode {
    if turn.is_object() { turn_status(turn) } else { ExitCode::SUCCESS }
}

fn describe_permission(p: &Value) -> String {
    format!("approval {}: {}", text(&p["request"]), text(&p["title"]))
}

// ---- cancel and queue ----------------------------------------------------

pub(super) fn cancel(args: &[String]) -> Result<ExitCode, String> {
    let mut arg = None;
    let (mut keep_held, mut json_out) = (false, false);
    for a in args {
        match a.as_str() {
            "--keep-held" => keep_held = true,
            "--json" => json_out = true,
            flag if flag.starts_with("--") => return Err(format!("unknown option: {flag}")),
            _ if arg.is_none() => arg = Some(a.clone()),
            _ => return Err(USAGE.to_owned()),
        }
    }
    let arg = arg.ok_or(USAGE)?;
    let hosts = discover()?;
    let (host, session) = running_session(&hosts, &arg)?;
    let req = json!({ "cmd": "cancel", "session": session, "keep_held": keep_held });
    let response = call(host, &req)?;
    if json_out {
        print_json(&response_json(response))?;
        return Ok(ExitCode::SUCCESS);
    }
    outln!("{}", text(&response["status"]));
    for held in response["dropped"].as_array().into_iter().flatten() {
        outln!("dropped {}: {}", text(&held["message"]), text(&held["text"]));
    }
    Ok(ExitCode::SUCCESS)
}

/// `queue list`: the held messages and context, after clearing them with
/// `--clear` and `--clear-context`.
pub(super) fn queue_list(args: &[String]) -> Result<ExitCode, String> {
    let mut arg = None;
    let mut json_out = false;
    let mut req = json!({ "cmd": "queue" });
    for a in args {
        match a.as_str() {
            "--clear" => req["clear"] = json!(true),
            "--clear-context" => req["clear_context"] = json!(true),
            "--json" => json_out = true,
            flag if flag.starts_with("--") => return Err(format!("unknown option: {flag}")),
            _ if arg.is_none() => arg = Some(a.clone()),
            _ => return Err(USAGE.to_owned()),
        }
    }
    queue(&arg.ok_or(USAGE)?, req, json_out)
}

/// `queue drop <session> <message>`: one held message dropped, and what is
/// still held.
pub(super) fn queue_drop(args: &[String]) -> Result<ExitCode, String> {
    let mut positional = Vec::new();
    let mut json_out = false;
    for a in args {
        match a.as_str() {
            "--json" => json_out = true,
            flag if flag.starts_with("--") => return Err(format!("unknown option: {flag}")),
            _ => positional.push(a),
        }
    }
    let [arg, message] = positional[..] else { return Err(USAGE.to_owned()) };
    queue(arg, json!({ "cmd": "queue", "drop": message }), json_out)
}

/// Sends `req`, the socket's `queue`, for session `arg`, and prints what it
/// dropped and what is held.
fn queue(arg: &str, mut req: Value, json_out: bool) -> Result<ExitCode, String> {
    let hosts = discover()?;
    let (host, session) = running_session(&hosts, arg)?;
    req["session"] = json!(session);
    let response = call(host, &req)?;
    if json_out {
        print_json(&response_json(response))?;
        return Ok(ExitCode::SUCCESS);
    }
    for held in response["dropped"].as_array().into_iter().flatten() {
        outln!("dropped {}: {}", text(&held["message"]), text(&held["text"]));
    }
    let held = response["held"].as_array().map_or(&[][..], Vec::as_slice);
    let context = response["context"].as_array().map_or(&[][..], Vec::as_slice);
    if held.is_empty() && context.is_empty() {
        outln!("nothing held");
    }
    for h in held {
        let how = if h["interrupt"] == true { "interrupt" } else { "after turn" };
        outln!("{} ({how}): {}", text(&h["message"]), text(&h["text"]));
    }
    for c in context {
        outln!("context: {}", c.as_str().unwrap_or("?"));
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
