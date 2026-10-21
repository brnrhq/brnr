//! Starting sessions and talking to them: `session new` and `resume`,
//! `prompt send`, `event wait`, `prompt cancel`, and `queue list`, `show`,
//! `drop` and `clear`.
//!
//! `session new --wait` and `prompt send --wait` print the agent's reply and
//! exit with the turn's result; `event wait` waits for a session to be idle
//! (or for the next turn, an approval, or its process's exit). Exit status:
//! 0 when the turn ended normally (`end_turn`), 1 if it failed or stopped
//! for another reason or its message was dropped (or, waiting for a turn or
//! an approval, the session closed first), 124 on `--timeout` (see ADR 8.3 in
//! docs/adr).
//!
//! `prompt send` (ADR 4.2 in docs/adr):
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

use std::env;
use std::fs;
use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Child, ExitCode, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::error::{self, Code};
use crate::request::{self, Request, Role};
use crate::{config, paths, render, signals, spawn, sys, transcript};

use super::args::Error::{self, Failed, Invalid};
use super::client::{Conn, Host, discover, session_in};
use super::ownership::{self, Resume};
use super::{
    Args, START_GRACE, START_TIMEOUT, print_json, process, read_stdin, running_session, settings,
    text,
};

/// Images bigger than this aren't sent: the whole prompt is one JSON line.
const MAX_IMAGE: u64 = 10 << 20;

/// Exit status for `--timeout`, as timeout(1) has it.
const TIMED_OUT: u8 = 124;

// ---- waiting for a turn --------------------------------------------------

/// Events `wait_for_message` needs.
const TURN_EVENTS: &[&str] = &[
    "user_message",
    "agent_message",
    "permission_request",
    "elicitation_request",
    "turn_ended",
    "message_dropped",
    "session_closed",
    "process_exited",
];

/// Follows the turn that answers `message` until it ends, printing the
/// agent's messages as they come (or, with `json`, the turn as one object at
/// the end: `{session, message, reply, stop_reason, error, tokens, dropped}`
/// and `json`'s fields), and returns the turn's exit status: 1, too, for a
/// message dropped before it was sent (`dropped` has it, and why). Approvals and
/// elicitations on the way are announced on stderr, with how to answer them.
pub(super) fn wait_for_message(
    conn: &mut Conn,
    arg: &str,
    session: &str,
    message: &str,
    deadline: Option<Instant>,
    mut json: Option<Value>,
) -> Result<ExitCode, error::Error> {
    let mut started = false;
    let mut reply: Vec<String> = Vec::new();
    loop {
        let e = match conn.next_event(deadline) {
            Ok(Some(e)) => e,
            Ok(None) => {
                eprintln!("brnr: timed out waiting for the reply");
                return Ok(ExitCode::from(TIMED_OUT));
            }
            Err(why) => return conn.ended(why),
        };
        let ended = 'event: {
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
                        "brnr: waiting for approval {request}: {} (brnr permission show {arg} {request}; brnr permission allow {arg} {request})",
                        e["title"].as_str().unwrap_or("?")
                    );
                }
                "elicitation_request" if ours => {
                    let request = e["request"].as_str().unwrap_or("?");
                    errln!(
                        "brnr: waiting for input {request}: {} (brnr elicitation show {arg} {request}; brnr elicitation accept {arg} {request})",
                        e["text"].as_str().unwrap_or("?")
                    );
                }
                "turn_ended" if carries(&e, message) => {
                    if let Some(out) = json.take() {
                        print_turn(out, session, message, &reply, &e)?;
                    }
                    break 'event Some(turn_status(&e));
                }
                "message_dropped" if e["message"] == message => {
                    if let Some(out) = json.take() {
                        print_turn(out, session, message, &reply, &e)?;
                    }
                    errln!("brnr: {message} was dropped ({})", e["reason"].as_str().unwrap_or("?"));
                    break 'event Some(ExitCode::FAILURE);
                }
                "session_closed" if ours => {
                    let error = "the session closed before the turn ended";
                    return Err(error::Error::new(Code::session_closed, error));
                }
                "process_exited" => {
                    let error = "the agent exited before the turn ended";
                    return Err(error::Error::new(Code::agent_exited, error));
                }
                _ => {}
            }
            None
        };
        if let Some(status) = ended {
            return Ok(status);
        }
    }
}

/// The turn as `--json` prints it, from the event that ended it
/// (`turn_ended`, or `message_dropped` for a message never sent): `out` with
/// `{session, message, reply, stop_reason, error, tokens, dropped}`.
/// `dropped` is the messages dropped, as `prompt cancel` and `queue` have
/// them (ADR 8.2): none, or the one whose drop ended the wait.
fn print_turn(
    mut out: Value,
    session: &str,
    message: &str,
    reply: &[String],
    end: &Value,
) -> Result<(), error::Error> {
    let dropped = match end["event"] == "message_dropped" {
        true => {
            json!([{ "message": end["message"], "text": end["text"], "reason": end["reason"] }])
        }
        false => json!([]),
    };
    for (k, v) in [
        ("session", json!(session)),
        ("message", json!(message)),
        ("reply", json!(reply.join("\n\n"))),
        ("stop_reason", end["stop_reason"].clone()),
        ("error", end["error"].clone()),
        ("tokens", end["tokens"].clone()),
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
    if e["error"].is_object() {
        errln!("brnr: turn failed: {}", render::error(&e["error"]));
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

/// A start, or an opening in a running process, that didn't commit in its
/// time, nor did the process say so (ADR 3.4).
fn timed_out() -> error::Error {
    error::Error::new(Code::start_timed_out, "timed out waiting for the session")
}

/// When `--timeout` runs out; never, for one too far off to say.
fn deadline(timeout: Option<u64>) -> Option<Instant> {
    timeout.and_then(|s| Instant::now().checked_add(Duration::from_secs(s)))
}

// ---- attachments ---------------------------------------------------------

/// `--file` as a resource link and `--image` as an image block.
fn attachments(files: &[String], images: &[String]) -> Result<Vec<Value>, error::Error> {
    let bad = |error: String| error::Error::new(Code::input_invalid, error);
    let mut blocks = Vec::new();
    for file in files {
        let path =
            fs::canonicalize(paths::expand(file)).map_err(|e| bad(format!("{file}: {e}")))?;
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
            _ => return Err(bad(format!("{image}: not a png, jpeg, gif or webp image"))),
        };
        let size = fs::metadata(&path).map_err(|e| bad(format!("{image}: {e}")))?.len();
        if size > MAX_IMAGE {
            return Err(bad(format!("{image}: {size} bytes; images go up to {MAX_IMAGE}")));
        }
        let data = fs::read(&path).map_err(|e| bad(format!("{image}: {e}")))?;
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

// A start, `session new` or `session resume`, is atomic (ADR 3.4 in
// docs/adr). Everything it needs is read and resolved before the process is
// launched, and handed to it in one request on its stdin (ADR 3.3). The
// process reports on the start channel, a socketpair at its fd 3: the start
// commits at its ready report, after which the process sends the prompt
// itself. Until then, the command going away (or giving up) stops it, and
// the prompt is never sent. With `--pid` the process is running already, and
// its answer on the control socket is the commit (see `start_in`).

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
    /// `--pid`: the running process to open it in.
    pid: Option<String>,
    take_over: bool,
    stop_when_idle: Option<u64>,
    response_timeout: Option<u64>,
    auth: Option<String>,
    strict: bool,
    wait: bool,
    timeout: Option<u64>,
    foreground: bool,
    quiet: bool,
    json: bool,
    agent: Vec<String>,
    /// `--` was given, with or without an agent after it.
    agent_given: bool,
}

/// `session new`'s flags, or (`resume`) `session resume`'s, which also
/// takes the session and `--take-over`; checked before the prompt is read
/// from stdin, so a usage error reads nothing.
fn parse_start(args: &Args, resume: bool) -> Result<StartArgs, Error> {
    if args.has("--take-over") && !resume {
        return Err(Invalid("--take-over goes with session resume".into()));
    }
    let value = |flag| args.value(flag).map(str::to_owned);
    let mut a = StartArgs {
        profile: value("--profile"),
        cwd: value("--cwd"),
        prompt: value("--prompt"),
        files: args.values("--file"),
        images: args.values("--image"),
        resume: args.positional.first().cloned(),
        pid: value("--pid"),
        take_over: args.has("--take-over"),
        stop_when_idle: args.number("--stop-when-idle"),
        response_timeout: args.number("--response-timeout"),
        auth: value("--auth"),
        strict: args.has("--strict"),
        wait: args.has("--wait"),
        timeout: args.number("--timeout"),
        foreground: args.has("--foreground"),
        quiet: args.has("--quiet"),
        json: args.json,
        agent: args.rest.clone().unwrap_or_default(),
        agent_given: args.rest.is_some(),
        ..StartArgs::default()
    };
    // The process is running: what starts one has nothing to do (P7).
    if let Some(pid) = &a.pid {
        let process = [
            ("--profile", a.profile.is_some()),
            ("--auth", a.auth.is_some()),
            ("--strict", a.strict),
            ("--stop-when-idle", a.stop_when_idle.is_some()),
            ("--response-timeout", a.response_timeout.is_some()),
            ("--foreground", a.foreground),
            ("--quiet", a.quiet),
            ("-- <agent>", a.agent_given),
        ];
        if let Some((flag, _)) = process.iter().find(|(_, given)| *given) {
            return Err(Invalid(format!(
                "{flag} is for starting a process, and doesn't go with --pid: process {pid} is \
                 running already"
            )));
        }
    }
    let (mode, model, thought_level) =
        (value("--mode"), value("--model"), value("--thought-level"));
    let options = settings::options("--option", &args.values("--option"))?;
    a.settings = request::Settings { mode, model, thought_level, options };
    let empty = |p: &Option<String>| p.as_deref().is_some_and(|p| p.trim().is_empty());
    if empty(&a.prompt) {
        return Err(Invalid("--prompt is empty".into()));
    }
    let has_prompt = a.prompt.is_some() || !a.files.is_empty() || !a.images.is_empty();
    if a.wait && !has_prompt {
        return Err(Invalid("--wait needs a --prompt (or --file, --image)".into()));
    }
    if a.wait && a.foreground {
        return Err(Invalid("--wait and --foreground don't go together".into()));
    }
    if a.quiet && !a.foreground {
        return Err(Invalid("--quiet goes with --foreground".into()));
    }
    if a.timeout.is_some() && !a.wait {
        return Err(Invalid("--timeout goes with --wait".into()));
    }
    // What stdin holds isn't the command line's: nothing there fails.
    if a.prompt.as_deref() == Some("-") {
        a.prompt = Some(read_stdin()?);
        if empty(&a.prompt) {
            let error = error::Error::new(Code::input_invalid, "the prompt on stdin is empty");
            return Err(Failed(error));
        }
    }
    Ok(a)
}

/// `brnr session new`: a process, and the new session it opens.
pub(super) fn new(args: &Args) -> Result<ExitCode, Error> {
    Ok(start(parse_start(args, false)?)?)
}

/// `brnr session resume`: a process, and the session it resumes (ADR 3.11).
pub(super) fn resume(args: &Args) -> Result<ExitCode, Error> {
    Ok(start(parse_start(args, true)?)?)
}

/// How long a start may take until it commits (`BRNR_START_TIMEOUT`).
fn start_timeout() -> Result<u64, error::Error> {
    match env::var("BRNR_START_TIMEOUT") {
        Ok(secs) => {
            secs.parse().map_err(|_| format!("BRNR_START_TIMEOUT: not seconds: {secs}").into())
        }
        Err(_) => Ok(START_TIMEOUT),
    }
}

fn start(mut a: StartArgs) -> Result<ExitCode, error::Error> {
    if let Some(pid) = a.pid.take() {
        return start_in(a, &pid);
    }
    let command = if a.resume.is_some() { "session resume" } else { "session new" };
    let blocks = attachments(&a.files, &a.images)?;
    let timeout = start_timeout()?;
    let mut resume_cwd = None;
    let hosts = if a.resume.is_some() { discover()? } else { Vec::new() };
    // --take-over: the process holding the session, and the session. It
    // closes the session once the process to resume it has started, so it
    // can say which (ADR 3.9, ADR 7.1).
    let mut owner = None;
    let mut transcript = false;
    if let Some(wanted) = a.resume.clone() {
        // A session another process holds is refused, naming the process,
        // unless --take-over. The process started here takes the lock itself,
        // and is refused the same way if it has been taken meanwhile. A
        // session brnr has no transcript of (one only the agent knows) goes to
        // the agent as given, in --cwd or here, with -- <agent> or the
        // profile's.
        let found = match ownership::locate(&hosts, &wanted).resume(None, a.take_over)? {
            Resume::TakeOver(host, running) => {
                owner = Some((host, wanted.clone()));
                transcript = true;
                Some(running)
            }
            // In any folder, whatever the cwd it is resumed in; one holding
            // only openings, which a load that failed leaves, isn't a
            // transcript of it (ADR 5.6). Its cwd, agent and profile are
            // the newest record's, of all its files (ADR 3.11).
            Resume::Inactive(past) => {
                transcript = transcript::kept(&wanted);
                Some(past)
            }
            Resume::Unknown => None,
        };
        if let Some(past) = found {
            a.resume = past["session"].as_str().map(str::to_owned);
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
        let error = format!("{}: not a directory", cwd.display());
        return Err(error::Error::new(Code::input_invalid, error));
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
        response_timeout: a.response_timeout.or(h.response_timeout),
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
            // Before the report: not ours.
            Ok(Some(msg)) if msg.get("event").is_some() => {}
            Ok(Some(msg)) => break msg,
            Ok(None) => return Err(timed_out()),
            Err(_) => break Value::Null,
        }
    };
    if ready["ok"].as_bool() != Some(true) {
        if let Some(child) = child {
            return Ok(exit_status(child)); // It has said why, on our stderr.
        }
        let error = match &ready["error"] {
            Value::Null => error::Error::from("the process exited without a session"),
            error => error::Error::of(error),
        };
        if crate::bug::is_panic(&error.message) {
            // A detached process has no terminal to say where to report it.
            errln!("brnr: {}", render::coded(&error.message, error.code.name()));
            eprintln!("{}", crate::bug::link(&error.message, command));
            return Ok(ExitCode::FAILURE);
        }
        return Err(error);
    }
    if let Some(child) = child {
        return Ok(exit_status(child));
    }
    // The turn's events follow the report on the same channel.
    if a.wait {
        conn.subscribed_at(&ready, TURN_EVENTS, &text(&ready["session"]));
    }
    started(&mut conn, &ready, a.wait, a.json, a.timeout)
}

/// What a start says once it has committed with `ready`, `{session, pid,
/// message}`: the session and its process, or with `wait` the turn,
/// followed on `conn` until `timeout`.
fn started(
    conn: &mut Conn,
    ready: &Value,
    wait: bool,
    json: bool,
    timeout: Option<u64>,
) -> Result<ExitCode, error::Error> {
    let session = text(&ready["session"]);
    // The prompt's id, as `send` gives it (ADR 4.1); null without one.
    let about = json!({ "session": session, "pid": ready["pid"], "message": ready["message"] });
    if !wait {
        if json {
            print_json(&about)?;
        } else {
            outln!("{}", describe_started(&about));
        }
        return Ok(ExitCode::SUCCESS);
    }
    if !json {
        errln!("{}", describe_started(&about));
    }
    let (message, json_out) = (text(&ready["message"]), json.then_some(about));
    wait_for_message(conn, &session, &session, &message, deadline(timeout), json_out)
}

/// `session new --pid` and `session resume --pid` (ADR 8.1): a session opened
/// in process `pid`, which is running, with the socket's `new` or `resume`.
/// The process answers once the session is open and its settings are set,
/// and only then sends the prompt: the answer is the commit, as a start's
/// ready report is (ADR 3.4). The start timeout goes with the request, and is
/// the process's: past it, the process closes the session again and says
/// so. Gone before it (Ctrl-C), the command closes its connection, and the
/// process does the same.
fn start_in(a: StartArgs, pid: &str) -> Result<ExitCode, error::Error> {
    let blocks = attachments(&a.files, &a.images)?;
    let timeout = start_timeout()?;
    let hosts = discover()?;
    let host = process(&hosts, pid)?;
    let mut req = json!({ "cmd": "new" });
    let mut cwd = a.cwd.clone();
    // --take-over: the process holding the session, to close it there.
    let mut owner = None;
    if let Some(wanted) = &a.resume {
        req = json!({ "cmd": "resume", "session": wanted });
        // A session brnr has no transcript of goes to the agent as given, in
        // --cwd or here.
        let found = match ownership::locate(&hosts, wanted).resume(Some(host), a.take_over)? {
            Resume::TakeOver(owner_host, running) => {
                owner = Some(owner_host);
                Some(running)
            }
            Resume::Inactive(past) => Some(past),
            Resume::Unknown => None,
        };
        if cwd.is_none() {
            cwd = found.and_then(|past| past["cwd"].as_str().map(str::to_owned));
        }
    }
    let cwd = match cwd {
        Some(dir) => paths::expand(&dir),
        None => env::current_dir().map_err(|e| format!("cwd: {e}"))?,
    };
    let cwd = std::path::absolute(&cwd).map_err(|e| format!("{}: {e}", cwd.display()))?;
    if !cwd.is_dir() {
        let error = format!("{}: not a directory", cwd.display());
        return Err(error::Error::new(Code::input_invalid, error));
    }
    let request::Settings { mode, model, thought_level, options } = &a.settings;
    for (key, value) in [
        ("cwd", json!(cwd.to_string_lossy())),
        ("mode", json!(mode)),
        ("model", json!(model)),
        ("thought_level", json!(thought_level)),
        ("options", json!(options)),
        ("text", json!(a.prompt.as_deref().unwrap_or_default())),
        ("blocks", json!(blocks)),
        ("timeout", json!(timeout)),
    ] {
        req[key] = value;
    }
    if let (Some(owner), Some(session)) = (owner, &a.resume) {
        // What the process would refuse is refused before the session is
        // closed where it runs, or it would be open nowhere.
        refused_in(host)?;
        let to = pid.parse().map_err(|_| format!("no brnr process {pid}"))?;
        settings::take_over(owner, session, to)?;
    }
    let mut conn = Conn::open(host)?;
    if a.wait {
        // Before the prompt can go, so no event of its turn is missed.
        conn.subscribe(TURN_EVENTS, None)?;
    }
    // The process abandons the opening when its timeout passes, and says
    // so; this is for a process stuck too badly to (ADR 3.4).
    let until =
        Instant::now().checked_add(Duration::from_secs(timeout).saturating_add(START_GRACE));
    let ready = match conn.call_until(req, until)? {
        Some(ready) => ready,
        None => return Err(timed_out()),
    };
    conn.only(&text(&ready["session"]));
    started(&mut conn, &ready, a.wait, a.json, a.timeout)
}

/// Why `host` would refuse to resume a session, as its status says: a
/// process starting or stopping, an editor's (ADR 7.1), `stop_when_idle` with
/// an agent that can't close sessions (ADR 3.8), or an agent with neither
/// `session/resume` nor `session/load` (ADR 3.11). The process checks the same
/// itself.
fn refused_in(host: &Host) -> Result<(), error::Error> {
    let refused = |code, error: String| Err(error::Error::new(code, error));
    let Some(status) = &host.status else {
        return refused(
            Code::process_unreachable,
            format!("process {} is not answering", host.id()),
        );
    };
    if status["starting"] != false {
        return refused(Code::process_starting, format!("process {} is still starting", host.id()));
    }
    if status["stopping"] == true {
        return refused(Code::process_stopping, format!("process {} is stopping", host.id()));
    }
    if status["owner"] == "editor" {
        let error = "the editor owns this process; open sessions there";
        return refused(Code::process_owned_by_editor, error.into());
    }
    let caps = &status["capabilities"];
    if caps["resume"] != true && caps["load"] != true {
        let error = "the agent can't resume sessions (no session/resume or session/load)";
        return refused(Code::capability_missing, error.into());
    }
    if !status["stop_when_idle"].is_null() && status["capabilities"]["close"] != true {
        let error = "the agent can't close sessions: with stop_when_idle, a second session would \
                     never close";
        return refused(Code::capability_missing, error.into());
    }
    Ok(())
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

pub(super) fn send(a: &Args) -> Result<ExitCode, Error> {
    let arg = a.positional[0].clone();
    let mut mode = None;
    let modes = a.given().filter_map(|f| f.strip_prefix("--"));
    for m in modes.filter(|m| ["steer", "interrupt", "context"].contains(m)) {
        match mode.replace(m) {
            Some(prev) if prev != m => {
                return Err(Invalid(format!("--{prev} and --{m} don't go together")));
            }
            _ => {}
        }
    }
    let (replace, wait, json_out) = (a.has("--replace"), a.has("--wait"), a.json);
    let timeout = a.number("--timeout");
    let (files, images) = (a.values("--file"), a.values("--image"));
    let prompt = a.value("--prompt");
    if timeout.is_some() && !wait {
        return Err(Invalid("--timeout goes with --wait".into()));
    }
    if replace && mode != Some("context") {
        return Err(Invalid("--replace only applies to --context".into()));
    }
    if wait && mode == Some("context") {
        return Err(Invalid("--wait needs a turn; --context doesn't start one".into()));
    }
    if prompt.is_some_and(|p| p.trim().is_empty()) {
        return Err(Invalid("--prompt is empty".into()));
    }
    if prompt.is_none() && files.is_empty() && images.is_empty() {
        return Err(Invalid("nothing to send: give --prompt, --file or --image".into()));
    }
    // What stdin holds isn't the command line's: nothing there fails.
    let text_in = match prompt {
        Some("-") => read_stdin()?,
        p => p.unwrap_or_default().to_owned(),
    };
    let blocks = attachments(&files, &images)?;
    if text_in.trim().is_empty() && blocks.is_empty() {
        return Err(Failed(error::Error::new(Code::input_invalid, "nothing to send")));
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
        let response = host.call(&req)?;
        if json_out {
            print_json(&response)?;
        } else {
            outln!("{}", describe_sent(&response));
        }
        return Ok(ExitCode::SUCCESS);
    }
    let mut conn = Conn::open(host)?;
    conn.subscribe(TURN_EVENTS, Some(&session))?;
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
    .map_err(Failed)
}

/// `delivered (message m1)`, `held`, …
fn describe_sent(response: &Value) -> String {
    let message =
        response["message"].as_str().map(|m| format!(" (message {m})")).unwrap_or_default();
    format!("{}{message}", text(&response["status"]))
}

// ---- wait ----------------------------------------------------------------

pub(super) fn wait(a: &Args) -> Result<ExitCode, Error> {
    let (arg, json_out) = (a.positional[0].clone(), a.json);
    let what = a.value("--for").unwrap_or("idle").to_owned();
    let timeout = a.number("--timeout");
    if !matches!(what.as_str(), "idle" | "turn" | "permission" | "elicitation" | "exit") {
        return Err(Invalid(format!("--for idle|turn|permission|elicitation|exit, not {what}")));
    }
    let hosts = discover()?;
    let (host, session) = running_session(&hosts, &arg)?;
    let mut conn = Conn::open(host)?;
    Ok(wait_on(&mut conn, &session, &what, deadline(timeout), json_out, host.info())?)
}

/// `event wait` on `conn`, the process's (`process`, its status), for `what`
/// of `session`. What ended it is printed as text, or with `--json` in one
/// shape for each `what`, whatever ended the wait or whether it held already
/// (ADR 8.3): for `idle` the session's state, `{session, state, last_turn}`;
/// for `turn` its `turn_ended`; for `permission` and `elicitation` the
/// request, as `permission show` and `elicitation show` print it; for
/// `exit` the `process_exited` event.
fn wait_on(
    conn: &mut Conn,
    session: &str,
    what: &str,
    deadline: Option<Instant>,
    json_out: bool,
    process: &Value,
) -> Result<ExitCode, error::Error> {
    // Subscribed before looking, so nothing happens unseen in between.
    let events = [
        "turn_ended",
        "permission_request",
        "elicitation_request",
        "session_closed",
        "process_exited",
    ];
    conn.subscribe(&events, Some(session))?;
    let mine = |e: &Value| e["session"] == session;
    // What ended the wait: printed as text, or with --json as itself.
    let done = |what: &Value, text: String| {
        if json_out {
            return print_json(what);
        }
        outln!("{text}");
        Ok(())
    };
    // The session's state, for `idle`: `idle`, or `inactive` once closed,
    // as `session list` has it, and how its last turn ended, as `session
    // status` has it.
    let state = |state: &str, turn: &Value| {
        let shown = json!({ "session": session, "state": state, "last_turn": turn });
        done(&shown, idle_text(state, turn))
    };
    // How the last turn ended: a wait for `idle` that ends on an idle or a
    // closed session exits as if it had waited for that turn (see ADR 8.3
    // in docs/adr).
    let mut last_turn = Value::Null;
    match what {
        "permission" | "elicitation" => match waiting(conn, session, what, &done) {
            Ok(Some(())) => return Ok(ExitCode::SUCCESS),
            Ok(None) => {}
            Err(why) => return conn.ended(why),
        },
        "idle" => {
            let (now, turn) = match idle(conn, session) {
                Ok(idle) => idle,
                Err(why) => return conn.ended(why),
            };
            if let Some(now) = now {
                state(now, &turn)?;
                return Ok(ended_as(&turn));
            }
            last_turn = turn;
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
            Err(why) if conn.missed().is_some() => return conn.ended(why),
            Err(_) if what == "exit" => {
                // Gone without saying how (killed): the event it would have
                // sent, with what brnr knows of it.
                let gone = json!({
                    "event": "process_exited",
                    "exit_status": null,
                    "error": error::Error::from("the process went without saying how it exited").json(),
                    "ts": crate::log::rfc3339(std::time::SystemTime::now()),
                    "host_id": process["host_id"],
                    "pid": process["host_pid"],
                    "seq": null,
                });
                done(&gone, render::exited(&gone["exit_status"]))?;
                return Ok(ExitCode::SUCCESS);
            }
            Err(e) => return Err(e),
        };
        let ended = 'event: {
            match (e["event"].as_str().unwrap_or_default(), what) {
                ("process_exited", "exit") => {
                    done(&e, render::exited(&e["exit_status"]))?;
                    break 'event Some(ExitCode::SUCCESS);
                }
                ("process_exited", _) => {
                    return Err(error::Error::new(Code::agent_exited, "the agent exited"));
                }
                // A closed session counts as idle; there is no next turn or
                // approval in it to wait for.
                ("session_closed", "idle") if mine(&e) => {
                    let shown =
                        json!({ "session": session, "state": "inactive", "last_turn": last_turn });
                    done(&shown, format!("inactive: session closed ({})", render::who(&e)))?;
                    break 'event Some(ended_as(&last_turn));
                }
                ("session_closed", "turn" | "permission" | "elicitation") if mine(&e) => {
                    return Err(error::Error::new(Code::session_closed, "the session closed"));
                }
                // One is waiting, unless answered since: what is waiting now
                // is what `permission show` or `elicitation show` prints.
                ("permission_request", "permission") | ("elicitation_request", "elicitation")
                    if mine(&e) =>
                {
                    match waiting(conn, session, what, &done) {
                        Ok(found) => break 'event found.map(|()| ExitCode::SUCCESS),
                        Err(why) => return conn.ended(why),
                    }
                }
                ("turn_ended", "turn") if mine(&e) => {
                    let how = e["stop_reason"].as_str().unwrap_or("error");
                    done(&e, format!("turn ended: {how}"))?;
                    break 'event Some(turn_status(&e));
                }
                ("turn_ended", "idle") if mine(&e) => {
                    last_turn = turn_of(&e);
                    let now = match idle(conn, session) {
                        Ok((now, _)) => now,
                        Err(why) => return conn.ended(why),
                    };
                    if let Some(now) = now {
                        // Behind the session, the turns that ended after
                        // this one are queued, before the status that says
                        // it is idle: it went idle after the last of them.
                        let ended = |q: &&Value| q["event"] == "turn_ended" && mine(q);
                        let e = conn.queued().rev().find(ended).cloned().unwrap_or(e);
                        state(now, &turn_of(&e))?;
                        break 'event Some(turn_status(&e));
                    }
                }
                _ => {}
            }
            None
        };
        if let Some(status) = ended {
            return Ok(status);
        }
    }
}

/// The first permission request (`what` `permission`) or elicitation
/// waiting in `session`, printed by `done` as `permission show` or
/// `elicitation show` prints it; `Ok(None)` if none is. `Err`: no answer.
fn waiting(
    conn: &mut Conn,
    session: &str,
    what: &str,
    done: &dyn Fn(&Value, String) -> Result<(), error::Error>,
) -> Result<Option<()>, error::Error> {
    let pending = conn.pending()?;
    let found = match what {
        "permission" => {
            pending.approvals(Some(session)).next().map(|p| (p, describe_permission(p)))
        }
        _ => pending.elicitations(Some(session)).next().map(|e| (e, describe_elicitation(e))),
    };
    found.map(|(e, text)| done(e, text)).transpose()
}

/// What `event wait` says of a session whose state is `state` and whose
/// last turn ended as `turn` says: `idle`, `idle: end_turn`.
fn idle_text(state: &str, turn: &Value) -> String {
    match turn.is_object() {
        true => format!("{state}: {}", turn["stop_reason"].as_str().unwrap_or("error")),
        false => state.to_owned(),
    }
}

/// Whether the session is idle, with no turn running and nothing held:
/// its state then, `idle`, or `inactive` for one that has closed; and how
/// its last turn ended, as `session status` has it (null for one that has
/// closed).
fn idle(conn: &mut Conn, session: &str) -> Result<(Option<&'static str>, Value), error::Error> {
    let status = conn.call(json!({ "cmd": "status" }))?;
    Ok(match session_in(&status, session) {
        Some(s) => {
            let is_idle = s["busy"] != true && s["held_count"].as_u64().unwrap_or(0) == 0;
            (is_idle.then_some("idle"), s["last_turn"].clone())
        }
        None => (Some("inactive"), Value::Null),
    })
}

/// A turn (`turn_ended`) as `last_turn` has it: `{stop_reason, error,
/// tokens}`.
fn turn_of(turn: &Value) -> Value {
    json!({ "stop_reason": turn["stop_reason"], "error": turn["error"], "tokens": turn["tokens"] })
}

/// The exit status of a turn that ended as `turn` says (`turn_ended`, or
/// status's `last_turn`); 0 when there was none.
fn ended_as(turn: &Value) -> ExitCode {
    if turn.is_object() { turn_status(turn) } else { ExitCode::SUCCESS }
}

fn describe_permission(p: &Value) -> String {
    format!("approval {}: {}", text(&p["request"]), text(&p["title"]))
}

fn describe_elicitation(e: &Value) -> String {
    format!("elicitation {}: {}", text(&e["request"]), text(&e["text"]))
}

// ---- cancel and queue ----------------------------------------------------

pub(super) fn cancel(a: &Args) -> Result<ExitCode, error::Error> {
    let (arg, keep_held, json_out) = (a.positional[0].clone(), a.has("--keep-held"), a.json);
    let hosts = discover()?;
    let (host, session) = running_session(&hosts, &arg)?;
    let req = json!({ "cmd": "cancel", "session": session, "keep_held": keep_held });
    let response = host.call(&req)?;
    if json_out {
        print_json(&response)?;
        return Ok(ExitCode::SUCCESS);
    }
    outln!("{}", text(&response["status"]));
    for held in response["dropped"].as_array().into_iter().flatten() {
        outln!("dropped {}: {}", text(&held["message"]), text(&held["text"]));
    }
    Ok(ExitCode::SUCCESS)
}

/// `queue list`: the held messages and context.
pub(super) fn queue_list(a: &Args) -> Result<ExitCode, error::Error> {
    queue(&a.positional[0], json!({ "cmd": "queue" }), a.json)
}

/// `queue show <session> <message>`: one held message in full, its text,
/// whether it interrupts, and its attachments.
pub(super) fn queue_show(a: &Args) -> Result<ExitCode, error::Error> {
    let (arg, message, json_out) = (&a.positional[0], &a.positional[1], a.json);
    let hosts = discover()?;
    let (host, session) = running_session(&hosts, arg)?;
    let response = host.call(&json!({ "cmd": "queue", "session": session, "show": message }))?;
    if json_out {
        print_json(&response)?;
        return Ok(ExitCode::SUCCESS);
    }
    let how = if response["interrupt"] == true { "interrupt" } else { "after turn" };
    outln!("{} ({how}), session {session}", text(&response["message"]));
    if let Some(t) = response["text"].as_str().filter(|t| !t.is_empty()) {
        outln!("{t}");
    }
    for block in response["blocks"].as_array().into_iter().flatten() {
        match block["type"].as_str() {
            Some("resource_link") => outln!("file: {}", text(&block["uri"])),
            Some("image") => {
                let data = block["data"].as_str().unwrap_or_default();
                let pad = data.bytes().rev().take_while(|&b| b == b'=').count();
                let bytes = (data.len() / 4 * 3).saturating_sub(pad);
                outln!("image: {}, {bytes} bytes", text(&block["mimeType"]));
            }
            Some("text") => outln!("text: {}", text(&block["text"])),
            _ => outln!("{block}"),
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// `queue drop <session> <message>`: one held message dropped, and what is
/// still held.
pub(super) fn queue_drop(a: &Args) -> Result<ExitCode, error::Error> {
    let (arg, message) = (&a.positional[0], &a.positional[1]);
    queue(arg, json!({ "cmd": "queue", "drop": message }), a.json)
}

/// `queue clear <session> [--messages] [--context]`: the held messages, the
/// held context, or with neither flag both, dropped; and what is left.
pub(super) fn queue_clear(a: &Args) -> Result<ExitCode, error::Error> {
    let (messages, context) = (a.has("--messages"), a.has("--context"));
    let neither = !messages && !context;
    let req = json!({ "cmd": "queue", "clear": messages || neither, "clear_context": context || neither });
    queue(&a.positional[0], req, a.json)
}

/// Sends `req`, the socket's `queue`, for session `arg`, and prints what it
/// dropped and what is held.
fn queue(arg: &str, mut req: Value, json_out: bool) -> Result<ExitCode, error::Error> {
    let hosts = discover()?;
    let (host, session) = running_session(&hosts, arg)?;
    req["session"] = json!(session);
    let response = host.call(&req)?;
    if json_out {
        print_json(&response)?;
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
    use crate::ctl::catch_up::MISSED;
    use crate::ctl::client::cut_off_at;
    use std::io::{BufRead, BufReader};
    use std::os::unix::net::UnixListener;

    /// `event wait`, cut off before the process answered its first look
    /// (`pending`, or `status` for `idle`), and unable to catch up, says what
    /// it missed and exits 3, as cut off while it waits does.
    #[test]
    fn adr_0010_event_wait_cut_off_looking_says_what_it_missed() {
        for (what, cmd) in
            [("permission", "pending"), ("elicitation", "pending"), ("idle", "status")]
        {
            let (mut conn, process) = cut_off_at(cmd);
            let ended = wait_on(&mut conn, "s", what, None, true, &Value::Null);
            assert_eq!(ended, Ok(ExitCode::from(MISSED)), "--for {what}");
            let note = conn.missed().unwrap();
            assert_eq!((&note["event"], &note["after_seq"]), (&json!("events_missed"), &json!(11)));
            process.join().unwrap();
        }
    }

    /// `session new --wait` (and `resume`) follows its turn on the start
    /// channel, subscribed as the ready report says (`socket#start`, at 10).
    /// Cut off there after one event, it catches up on the process's socket:
    /// what it missed, 12 to 14, comes from the transcript, in order and once
    /// each (11, which it got, and 15, which comes live, are in the
    /// transcript too), then what comes live, 15 and 16. Cut off where it
    /// can't catch up (records not written), it says what it missed and
    /// exits 3.
    #[test]
    fn adr_0010_a_start_channel_cut_off_catches_up_or_says_what_it_missed() {
        let dir = env::temp_dir().join(format!("brnr-start-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let (socket, host_log, file) =
            (dir.join("h.sock"), dir.join("host.jsonl"), dir.join("s.jsonl"));
        let record =
            |event: Value| json!({ "format": 1, "ts": "t", "host_id": "h", "event": event });
        let opened = json!({ "event": "session_opened", "session": "s", "events_log": file });
        fs::write(&host_log, format!("{}\n", record(opened))).unwrap();
        let event = |seq: u64, name: &str, text: &str| {
            let mut e = json!({ "event": name, "session": "s", "host_id": "h", "seq": seq });
            match name {
                "user_message" => e["message"] = json!("m1"),
                "turn_ended" => {
                    (e["messages"], e["stop_reason"]) = (json!(["m1"]), json!("end_turn"))
                }
                _ => e["text"] = json!(text),
            }
            e
        };
        let recorded = [
            event(11, "user_message", ""),
            event(12, "agent_message", "one"),
            event(13, "agent_message", "two"),
            event(14, "agent_message", "three"),
            event(15, "agent_message", "live"),
        ];
        let lines: String = recorded.iter().map(|e| format!("{}\n", record(e.clone()))).collect();
        fs::write(&file, lines).unwrap();
        // The process's socket: the start channel was cut off at 11; it
        // subscribes again at 14, and 15 and 16 come live.
        let listener = UnixListener::bind(&socket).unwrap();
        let live = [event(15, "agent_message", "live"), event(16, "turn_ended", "")];
        let process = thread::spawn(move || {
            let mut stream = listener.incoming().next().unwrap().unwrap();
            let mut asked = Vec::new();
            let lines = BufReader::new(stream.try_clone().unwrap()).lines();
            for line in lines.map_while(Result::ok) {
                let req: Value = serde_json::from_str(&line).unwrap();
                asked.push(req.clone());
                let mut answer = match req["cmd"].as_str().unwrap() {
                    "was_cut_off" => {
                        json!({ "ok": true, "cut_off": req["peer"] == "socket#start" })
                    }
                    "subscribe" => {
                        json!({ "ok": true, "peer": "socket#1", "host_id": "h", "seq": 14 })
                    }
                    _ => json!({ "ok": true, "lost": false }),
                };
                answer["req_id"] = req["req_id"].clone();
                writeln!(stream, "{answer}").unwrap();
                match req["cmd"].as_str().unwrap() {
                    "subscribe" => writeln!(stream, "{}", live[0]).unwrap(),
                    "logged" => {
                        writeln!(stream, "{}", live[1]).unwrap();
                        break;
                    }
                    _ => {}
                }
            }
            asked
        });
        let ready = json!({
            "ok": true, "session": "s", "pid": 4242, "message": "m1",
            "peer": "socket#start", "host_id": "h", "seq": 10,
        });
        // The start channel, as `start` reads it: the report, then the turn.
        let start_channel = |then: &[Value]| {
            let (ours, mut theirs) = UnixStream::pair().unwrap();
            writeln!(theirs, "{ready}").unwrap();
            for e in then {
                writeln!(theirs, "{e}").unwrap();
            }
            let mut conn = Conn::new(ours).unwrap();
            let report = conn.read(None).unwrap().unwrap();
            assert_eq!(report, ready);
            conn.subscribed_at(&report, TURN_EVENTS, "s");
            // As the process names it; here, at the fake's.
            let f = conn.follow().expect("the start channel isn't followed");
            assert_eq!(f.socket, paths::runtime_dir().join("4242.sock"));
            assert_eq!(
                (f.peer.as_str(), f.last, f.session.as_deref()),
                ("socket#start", 10, Some("s"))
            );
            (f.socket, f.host_log) = (socket.clone(), host_log.clone());
            conn
        };
        let mut conn = start_channel(&[event(11, "user_message", "")]);
        let mut got = Vec::new();
        while got.len() < 5 {
            got.push(conn.next_event(None).unwrap().unwrap());
        }
        let seqs: Vec<_> = got.iter().map(|e| e["seq"].as_u64().unwrap()).collect();
        assert_eq!(seqs, [11, 12, 13, 14, 15]);
        assert_eq!(got, recorded, "what it got, what it missed, then what came live");
        // The turn's end, live, ends the wait.
        assert_eq!(wait_for_message(&mut conn, "s", "s", "m1", None, None), Ok(ExitCode::SUCCESS));
        assert_eq!(conn.socket().1, 1, "connected again");
        let asked: Vec<_> = process.join().unwrap().into_iter().map(|r| r["cmd"].clone()).collect();
        assert_eq!(asked, ["was_cut_off", "subscribe", "logged"]);
        let _ = fs::remove_dir_all(&dir);

        // A process with records it couldn't write (`logged` says `lost`):
        // `cut_off_at`'s, asked nothing on its first connection, so that the
        // second is the one catching up makes, subscribed again at 20.
        let (first, process) = cut_off_at("none");
        let socket = first.socket_path().unwrap();
        drop(first);
        let mut conn = start_channel(&[]);
        conn.follow().unwrap().socket = socket;
        let ended = started(&mut conn, &ready, true, true, None);
        assert_eq!(ended, Ok(ExitCode::from(MISSED)));
        let note = conn.missed().unwrap();
        assert_eq!(
            (&note["event"], &note["peer"]),
            (&json!("events_missed"), &json!("socket#start"))
        );
        assert_eq!(
            (note["after_seq"].as_u64(), note["until_seq"].as_u64()),
            (Some(10), Some(20)),
            "{note}"
        );
        process.join().unwrap();
    }

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
