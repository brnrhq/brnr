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

use std::collections::VecDeque;
use std::env;
use std::fs;
use std::io::{self, BufRead, BufReader, ErrorKind, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, ExitCode, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::request::{self, Request, Role};
use crate::{config, json, lock, log, paths, render, signals, spawn, sys};

use super::catch_up::{Follow, MISSED, TRIES};

use super::{
    Found, Host, START_GRACE, START_TIMEOUT, USAGE, call, closing, closing_in, connect, discover,
    find_session, print_json, process, read_stdin, response_json, running_in, running_session,
    settings, text,
};

/// Images bigger than this aren't sent: the whole prompt is one JSON line.
const MAX_IMAGE: u64 = 10 << 20;

/// Exit status for `--timeout`, as timeout(1) has it.
const TIMED_OUT: u8 = 124;

// ---- a connection that subscribes and calls ------------------------------

/// One control connection that is subscribed to events and also makes
/// requests: responses are matched by `req_id`, and events that arrive in
/// between are kept for `next_event`. Cut off by the process for falling
/// behind, it catches up from the transcript and goes on live on a new
/// connection (ADR 10.2, see catch_up.rs). What it reads may nest as deep as
/// the agent made it: [`Conn::deep`] uses it and lets go of it on a stack
/// that takes it (ADR 2.5).
pub(super) struct Conn {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
    events: VecDeque<Value>,
    /// How deep what it read, and may still hold in `events`, nests.
    deepest: usize,
    /// How deep the stack it runs on takes (see `deep`).
    stack: usize,
    next_req: u64,
    /// The process's socket, to subscribe on again.
    socket: Option<PathBuf>,
    /// Subscribed: where it is, to catch up from.
    follow: Option<Follow>,
    /// What it missed, once it has given up catching up.
    missed: Option<Value>,
    /// How many times it has connected again.
    reconnects: u32,
}

impl Conn {
    pub(super) fn open(host: &Host) -> Result<Conn, String> {
        let stream = connect(host).map_err(|e| format!("process {}: {e}", host.id()))?;
        let mut conn = Conn::new(stream)?;
        conn.socket = host.meta["socket"].as_str().map(PathBuf::from);
        Ok(conn)
    }

    /// One over `stream`: a control connection, or the start channel.
    fn new(stream: UnixStream) -> Result<Conn, String> {
        let writer = stream.try_clone().map_err(|e| e.to_string())?;
        Ok(Conn {
            reader: BufReader::new(stream),
            writer,
            events: VecDeque::new(),
            deepest: 0,
            stack: json::SHALLOW,
            next_req: 0,
            socket: None,
            follow: None,
            missed: None,
            reconnects: 0,
        })
    }

    /// Its connection, to look at without reading (`brnr event notify`),
    /// and how many times it has connected again, to tell a new one.
    pub(super) fn socket(&self) -> (io::Result<UnixStream>, u32) {
        (self.writer.try_clone(), self.reconnects)
    }

    /// Subscribes to `events`, of `session` (the command's to filter; the
    /// process sends every session's) or of every session.
    pub(super) fn subscribe(
        &mut self,
        events: &[&str],
        session: Option<&str>,
    ) -> Result<(), String> {
        let answer = self.call(json!({ "cmd": "subscribe", "events": events }))?;
        if let Some(socket) = self.socket.clone() {
            self.follow = Follow::new(socket, &answer, events, session);
        }
        Ok(())
    }

    /// Follows only `session`, which the subscription came before.
    fn only(&mut self, session: &str) {
        if let Some(f) = &mut self.follow {
            f.session = Some(session.to_owned());
        }
    }

    /// The start channel, subscribed to `events` by the process `ready` names
    /// once the start commits: as a subscription, from where it says.
    fn subscribed_at(&mut self, ready: &Value, events: &[&str], session: &str) {
        let socket = paths::runtime_dir().join(format!("{}.sock", ready["pid"]));
        self.follow = Follow::new(socket, ready, events, Some(session));
    }

    /// What it missed and couldn't catch up on, once it has given up (ADR 10.2).
    pub(super) fn missed(&self) -> Option<&Value> {
        self.missed.as_ref()
    }

    /// How a command ends that has got `Err(why)` from the connection: with
    /// [`MISSED`] once it has said what it missed, if it gave up catching
    /// up; else as `why`.
    pub(super) fn ended(&self, why: String) -> Result<ExitCode, String> {
        let Some(note) = &self.missed else { return Err(why) };
        let text = render::event(note, &render::Options { session: false, time: false });
        errln!("brnr: {}", text.unwrap_or_default());
        Ok(ExitCode::from(MISSED))
    }

    /// Runs `f` with `value` on a stack that takes it and what the connection
    /// holds: this one, or a thread's of its own (see json.rs). `f` lets go of
    /// `value` there. `Err` says there is no such stack to be had (P3); then
    /// `value` is kept until the process exits, rather than let go of here.
    pub(super) fn deep<R: Send>(
        &mut self,
        value: Value,
        f: impl FnOnce(&mut Conn, Value) -> R + Send,
    ) -> Result<R, String> {
        let need = json::nesting([&value]).max(self.deepest);
        let mut value = Some(value);
        let result = if need <= self.stack {
            Some(f(self, value.take().unwrap()))
        } else {
            let outer = self.stack;
            let result = json::on_stack(need, || {
                self.stack = need;
                f(self, value.take().unwrap())
            });
            self.stack = outer;
            result
        };
        // Not taken: no stack to be had, to let go of it on either.
        std::mem::forget(value);
        // What was deep may be gone, as in the host's `deep`.
        if self.deepest > json::SHALLOW {
            self.deepest = json::nesting(&self.events);
        }
        result.ok_or_else(|| format!("no stack to be had for a message {need} deep"))
    }

    /// A request whose failure is an error.
    pub(super) fn call(&mut self, req: Value) -> Result<Value, String> {
        self.call_until(req, None)?.ok_or_else(|| "the process closed the connection".into())
    }

    /// A request whose failure is an error, answered by `deadline`;
    /// `Ok(None)` when it passes first. One that only asks (`status`,
    /// `pending`) is asked again once caught up, if the process cut the
    /// connection off before it answered.
    fn call_until(
        &mut self,
        mut req: Value,
        deadline: Option<Instant>,
    ) -> Result<Option<Value>, String> {
        self.next_req += 1;
        let id = format!("r{}", self.next_req);
        req["req_id"] = json!(id);
        writeln!(self.writer, "{req}").map_err(|e| e.to_string())?;
        let again = matches!(req["cmd"].as_str(), Some("status" | "pending"));
        loop {
            let msg = match self.read(deadline) {
                Ok(Some(msg)) => msg,
                Ok(None) => return Ok(None),
                Err(why) if again && self.follow.is_some() => {
                    self.catch_up(why)?;
                    writeln!(self.writer, "{req}").map_err(|e| e.to_string())?;
                    continue;
                }
                Err(why) => return Err(why),
            };
            if msg["req_id"] == id.as_str() {
                if msg["ok"].as_bool() != Some(true) {
                    return Err(msg["error"].as_str().unwrap_or("request failed").to_owned());
                }
                return Ok(Some(msg));
            }
            if msg.get("event").is_some() {
                self.events.push_back(msg);
            } else {
                json::free(msg); // An answer to another request.
            }
        }
    }

    /// The next event; `Ok(None)` when `deadline` passes, `Err` when the
    /// connection closes, but for a cut-off it has caught up on.
    pub(super) fn next_event(
        &mut self,
        deadline: Option<Instant>,
    ) -> Result<Option<Value>, String> {
        loop {
            if let Some(event) = self.events.pop_front() {
                return Ok(Some(event));
            }
            match self.read(deadline) {
                Ok(None) => return Ok(None),
                Ok(Some(msg)) if msg.get("event").is_some() => return Ok(Some(msg)),
                Ok(Some(msg)) => json::free(msg), // A response nobody waits for.
                Err(why) => self.catch_up(why)?,
            }
        }
    }

    /// What is left to read before the connection ends, without catching
    /// up, why it ended, and whether the process cut it off: for `event
    /// notify`, which looks while its command runs (ADR 10.2).
    pub(super) fn rest(&mut self) -> (Vec<Value>, String, bool) {
        let mut rest: Vec<Value> = self.events.drain(..).collect();
        let why = loop {
            match self.read(None) {
                Ok(Some(msg)) if msg.get("event").is_some() => rest.push(msg),
                Ok(Some(msg)) => json::free(msg),
                Ok(None) => {}
                Err(why) => break why,
            }
        };
        let cut = match self.follow.as_ref() {
            Some(f) => {
                match UnixStream::connect(&f.socket).map_err(|e| e.to_string()).and_then(Conn::new)
                {
                    Ok(mut conn) => conn.was_cut_off(&f.peer).is_some(),
                    Err(_) => f.cut_off_before_it_went(),
                }
            }
            // Given up catching up, in a request: what it read meanwhile
            // is left, then what it missed.
            None => self.missed.is_some(),
        };
        (rest, why, cut)
    }

    /// Asks the process whether it cut `peer` off: if it did, the last
    /// position then (`last_queued`), if it says; none if it didn't, or
    /// doesn't answer.
    fn was_cut_off(&mut self, peer: &str) -> Option<Option<u64>> {
        let asked = self.call(json!({ "cmd": "was_cut_off", "peer": peer })).ok()?;
        (asked["cut_off"] == true).then(|| asked["last_queued"].as_u64())
    }

    /// The connection has ended (`why`) before the end the command waits
    /// for: if the process cut it off, what it missed, read from the
    /// transcript, is queued, followed by what comes live on a new
    /// subscription (ADR 10.2). `Err(why)` if it wasn't cut off; or, with
    /// `missed` saying what it missed, if it can't catch up. A new
    /// subscription that fails, or doesn't say where it starts, is one more
    /// cut-off: there is no position to catch up to, so it is tried again.
    fn catch_up(&mut self, why: String) -> Result<(), String> {
        let Some(mut f) = self.follow.take() else { return Err(why) };
        // Once the process has said it cut `f.peer` off: where it was then.
        let mut cut: Option<Option<u64>> = None;
        let (live, until) = loop {
            f.tries += 1;
            let fresh =
                UnixStream::connect(&f.socket).map_err(|e| e.to_string()).and_then(Conn::new);
            let mut conn = match fresh {
                Ok(conn) => conn,
                // Gone: its transcript is whole (ADR 5.5), and is read to its end.
                Err(_) if cut.is_some() || f.cut_off_before_it_went() => break (None, None),
                Err(_) => return Err(why),
            };
            if cut.is_none() {
                cut = conn.was_cut_off(&f.peer);
                if cut.is_none() {
                    return Err(why);
                }
            }
            match conn.resubscribe(&f) {
                Ok((peer, until)) => {
                    f.peer = peer;
                    break (Some(conn), Some(until));
                }
                Err((Some(at), e)) => return Err(self.give_up(&f, Some(at), &e)),
                Err((None, e)) if f.tries > TRIES => {
                    // Live, as far as it knows: up to where the process
                    // said it cut the peer off.
                    let known = cut.flatten();
                    let n = f.tries;
                    let mut e = format!("cut off {n} times in a row while catching up ({e})");
                    if known.is_none() {
                        e.push_str("; the process says no position, so up to where is unknown");
                    }
                    return Err(self.give_up(&f, known, &e));
                }
                Err((None, _)) => {}
            }
        };
        if f.tries > TRIES {
            let e = format!("cut off {} times in a row while catching up", f.tries);
            return Err(self.give_up(&f, until, &e));
        }
        let caught = match f.read(until) {
            Ok(caught) => caught,
            Err(e) => return Err(self.give_up(&f, until, &e)),
        };
        f.last = until.or(caught.last().and_then(|e| e["seq"].as_u64())).unwrap_or(f.last);
        self.deepest = self.deepest.max(json::nesting(&caught));
        self.events.extend(caught);
        let Some(mut conn) = live else { return Ok(()) };
        if !conn.events.is_empty() {
            f.tries = 0;
        }
        // Live while it subscribed: read, so caught up on no more.
        if let Some(seq) = conn.events.iter().filter_map(|e| e["seq"].as_u64()).max() {
            f.last = f.last.max(seq);
        }
        self.events.append(&mut conn.events);
        self.deepest = self.deepest.max(conn.deepest);
        std::mem::swap(&mut self.reader, &mut conn.reader);
        std::mem::swap(&mut self.writer, &mut conn.writer);
        self.reconnects += 1;
        self.follow = Some(f);
        Ok(())
    }

    /// Subscribes as `f` did, and waits for the process to have written what
    /// it recorded until then: the new subscription's peer, and where it
    /// starts, which the transcript has everything up to. `Err` says why it
    /// can't, and where the new subscription starts once it has one.
    fn resubscribe(&mut self, f: &Follow) -> Result<(String, u64), (Option<u64>, String)> {
        let events: Vec<&str> = f.events.iter().map(String::as_str).collect();
        let answer = self.call(json!({ "cmd": "subscribe", "events": events }));
        let answer = answer.map_err(|e| (None, e))?;
        let next = Follow::new(f.socket.clone(), &answer, &events, f.session.as_deref());
        let next = next.ok_or((None, "the process doesn't say where its events are".into()))?;
        let at = Some(next.last);
        if self.call(json!({ "cmd": "logged" })).map_err(|e| (at, e))?["lost"] == true {
            let why = "the process has records it couldn't write yet (a full disk?)";
            return Err((at, why.into()));
        }
        Ok((next.peer, next.last))
    }

    /// Gives up catching up: `missed` says what it missed, and why.
    fn give_up(&mut self, f: &Follow, until: Option<u64>, why: &str) -> String {
        let note = f.missed(until, why);
        let text = render::event(&note, &render::Options { session: false, time: false });
        self.missed = Some(note);
        text.unwrap_or_else(|| why.to_owned())
    }

    /// The next line, an event or a response; an event moves on where a
    /// subscription is.
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
            // Cut short: the process cut the connection off mid-line.
            Ok(_) if !line.ends_with('\n') => Err("the process closed the connection".into()),
            Ok(_) => {
                // As the host reads it (see json.rs): an event carries what
                // the agent sent as deep as it nested it (ADR 2.5).
                let line = line.as_bytes();
                let depth = json::depth(line);
                let msg = json::on_stack(depth, || json::parse::<Value>(line));
                let msg = msg.flatten().unwrap_or(Value::Null);
                self.deepest = self.deepest.max(depth);
                if let (Some(f), Some(seq)) = (&mut self.follow, msg["seq"].as_u64())
                    && msg.get("event").is_some()
                {
                    (f.last, f.tries) = (seq, 0);
                }
                Ok(Some(msg))
            }
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }
}

impl Drop for Conn {
    /// What it still holds may be deep: let go of on a stack that takes it.
    fn drop(&mut self) {
        json::free(Value::Array(std::mem::take(&mut self.events).into()));
    }
}

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
    "exited",
];

/// Follows the turn that answers `message` until it ends, printing the
/// agent's messages as they come (or, with `json`, the turn as one object at
/// the end: `{session, message, reply, stop_reason, error, tokens, dropped}`
/// and `json`'s fields), and returns the turn's exit status: 1, too, for a
/// message dropped before it was sent (`dropped` says why). Approvals and
/// elicitations on the way are announced on stderr, with how to answer them.
pub(super) fn wait_for_message(
    conn: &mut Conn,
    arg: &str,
    session: &str,
    message: &str,
    deadline: Option<Instant>,
    mut json: Option<Value>,
) -> Result<ExitCode, String> {
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
        // Used and let go of on a stack that takes it (ADR 2.5).
        let ended = conn.deep(e, |_, e| -> Result<Option<ExitCode>, String> {
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
                        e["message"].as_str().unwrap_or("?")
                    );
                }
                "turn_ended" if carries(&e, message) => {
                    if let Some(out) = json.take() {
                        print_turn(out, session, message, &reply, &e)?;
                    }
                    return Ok(Some(turn_status(&e)));
                }
                "message_dropped" if e["message"] == message => {
                    if let Some(out) = json.take() {
                        print_turn(out, session, message, &reply, &e)?;
                    }
                    errln!("brnr: {message} was dropped ({})", e["by"].as_str().unwrap_or("?"));
                    return Ok(Some(ExitCode::FAILURE));
                }
                "session_closed" if ours => {
                    return Err("the session closed before the turn ended".into());
                }
                "exited" => return Err("the agent exited before the turn ended".into()),
                _ => {}
            }
            Ok(None)
        })??;
        if let Some(status) = ended {
            return Ok(status);
        }
    }
}

/// The turn as `--json` prints it, from the event that ended it
/// (`turn_ended`, or `message_dropped` for a message never sent): `out` with
/// `{session, message, reply, stop_reason, error, tokens, dropped}`.
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
            "--pid" => a.pid = Some(value("--pid")?),
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
            "--response-timeout" => {
                let secs = value("--response-timeout")?;
                a.response_timeout = Some(seconds("--response-timeout", &secs)?);
            }
            "--strict" => a.strict = true,
            "--wait" => a.wait = true,
            "--foreground" => a.foreground = true,
            "--quiet" => a.quiet = true,
            "--json" => a.json = true,
            "--" => {
                a.agent = it.by_ref().cloned().collect();
                a.agent_given = true;
                break;
            }
            flag if flag.starts_with("--") => return Err(format!("unknown option: {flag}")),
            _ if resume && a.resume.is_none() => a.resume = Some(arg.clone()),
            _ => return Err(USAGE.to_owned()),
        }
    }
    if resume && a.resume.is_none() {
        return Err(USAGE.to_owned());
    }
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
            return Err(format!(
                "{flag} is for starting a process, and doesn't go with --pid: process {pid} is \
                 running already"
            ));
        }
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

/// `brnr session resume`: a process, and the session it resumes (ADR 3.11).
pub(super) fn resume(args: &[String]) -> Result<ExitCode, String> {
    start(parse_start(args, true)?)
}

/// How long a start may take until it commits (`BRNR_START_TIMEOUT`).
fn start_timeout() -> Result<u64, String> {
    match env::var("BRNR_START_TIMEOUT") {
        Ok(secs) => secs.parse().map_err(|_| format!("BRNR_START_TIMEOUT: not seconds: {secs}")),
        Err(_) => Ok(START_TIMEOUT),
    }
}

fn start(mut a: StartArgs) -> Result<ExitCode, String> {
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
        // Closing in its process: free once its records are written (ADR 5.5).
        if let Some(host) = closing_in(&hosts, &wanted) {
            return Err(closing(&wanted, host.id()));
        }
        // A session another process holds is refused, naming the process,
        // unless --take-over. The process started here takes the lock itself,
        // and is refused the same way if it has been taken meanwhile.
        let found = match lock::holder(&wanted) {
            Some(pid) if !a.take_over => {
                let hint = " (--take-over closes it there and resumes it here)";
                return Err(running_in(&wanted, pid, hint));
            }
            Some(pid) => {
                let (host, running) = settings::held(&hosts, &wanted, pid)?;
                owner = Some((host, wanted.clone()));
                transcript = true;
                Some(running)
            }
            // A session brnr has no transcript of (one only the agent knows)
            // goes to the agent as given, in --cwd or here, with -- <agent>
            // or the profile's.
            None => match find_session(&hosts, &wanted) {
                // Served without its lock, shared by an editor's process.
                Ok(Found::Running(host, _)) => {
                    return Err(format!("{wanted} is running in process {}", host.id()));
                }
                // One holding only openings, which a load that failed
                // leaves, isn't a transcript of it (ADR 5.6).
                Ok(Found::Inactive(past)) => {
                    let file = past["log"].as_str().map(Path::new);
                    transcript = file.is_some_and(log::has_records);
                    Some(past)
                }
                Err(_) => None,
            },
        };
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
            Ok(Some(msg)) if msg.get("event").is_some() => json::free(msg),
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
        if crate::bug::is_panic(error) {
            // A detached process has no terminal to say where to report it.
            errln!("brnr: {error}");
            eprintln!("{}", crate::bug::link(error, command));
            return Ok(ExitCode::FAILURE);
        }
        return Err(error.to_owned());
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
) -> Result<ExitCode, String> {
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
fn start_in(a: StartArgs, pid: &str) -> Result<ExitCode, String> {
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
        if let Some(host) = closing_in(&hosts, wanted) {
            return Err(closing(wanted, host.id()));
        }
        let here = format!("{wanted} is already open in process {pid}");
        let found = match lock::holder(wanted) {
            Some(holder) if holder.to_string() == pid => return Err(here),
            Some(holder) if !a.take_over => {
                let hint =
                    format!(" (--take-over closes it there and resumes it in process {pid})");
                return Err(running_in(wanted, holder, &hint));
            }
            Some(holder) => {
                let (owner_host, running) = settings::held(&hosts, wanted, holder)?;
                owner = Some(owner_host);
                Some(running)
            }
            // A session brnr has no transcript of goes to the agent as
            // given, in --cwd or here.
            None => match find_session(&hosts, wanted) {
                Ok(Found::Running(h, _)) if h.id() == pid => return Err(here),
                Ok(Found::Running(h, _)) => {
                    return Err(format!("{wanted} is running in process {}", h.id()));
                }
                Ok(Found::Inactive(past)) => Some(past),
                Err(_) => None,
            },
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
        return Err(format!("{}: not a directory", cwd.display()));
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
        None => return Err("timed out waiting for the session".into()),
    };
    conn.only(&text(&ready["session"]));
    started(&mut conn, &ready, a.wait, a.json, a.timeout)
}

/// Why `host` would refuse to resume a session, as its status says: a
/// process starting or stopping, an editor's (ADR 7.1), `stop_when_idle` with
/// an agent that can't close sessions (ADR 3.8), or an agent with neither
/// `session/resume` nor `session/load` (ADR 3.11). The process checks the same
/// itself.
fn refused_in(host: &Host) -> Result<(), String> {
    let Some(status) = &host.status else {
        return Err(format!("process {} is not answering", host.id()));
    };
    if status["starting"] != false {
        return Err(format!("process {} is still starting", host.id()));
    }
    if status["stopping"] == true {
        return Err(format!("process {} is stopping", host.id()));
    }
    if status["owner"] == "editor" {
        return Err("the editor owns this process; open sessions there".into());
    }
    let caps = &status["capabilities"];
    if caps["resume"] != true && caps["load"] != true {
        return Err("the agent can't resume sessions (no session/resume or session/load)".into());
    }
    if !status["stop_when_idle"].is_null() && status["capabilities"]["close"] != true {
        return Err("the agent can't close sessions: with stop_when_idle, a second session would \
                    never close"
            .into());
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

/// `prompt send`'s options that take a value.
const SEND_VALUES: [&str; 3] = ["--timeout", "--file", "--image"];

/// Where `prompt send`'s prompt starts in `args`, at its first word after
/// the session or at `--`, and whether its options before it, `-h` and
/// `--help` among them, ask for its usage. From the prompt on, every word is
/// the prompt's; an option's value is the option's.
pub(super) fn prompt_start(args: &[String]) -> (usize, bool) {
    let (mut session, mut help) = (false, false);
    let mut i = 0;
    while let Some(arg) = args.get(i) {
        match arg.as_str() {
            "--" => break,
            "-h" | "--help" => help = true,
            flag if SEND_VALUES.contains(&flag) => i += 1,
            flag if flag.starts_with("--") => {}
            _ if !session => session = true,
            _ => break,
        }
        i += 1;
    }
    (i.min(args.len()), help)
}

pub(super) fn send(args: &[String]) -> Result<ExitCode, String> {
    let mut arg = None;
    let mut mode = None;
    let (mut replace, mut wait, mut json_out) = (false, false, false);
    let mut timeout = None;
    let mut files = Vec::new();
    let mut images = Vec::new();
    let (options, prompt) = args.split_at(prompt_start(args).0);
    let words = prompt.strip_prefix(&["--".to_owned()][..]).unwrap_or(prompt);
    let mut it = options.iter();
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
            flag if flag.starts_with("--") => return Err(format!("unknown option: {flag}")),
            _ => arg = Some(a.clone()),
        }
    }
    let arg = arg.ok_or(USAGE)?;
    if timeout.is_some() && !wait {
        return Err("--timeout goes with --wait".into());
    }
    if replace && mode != Some("context") {
        return Err("--replace only applies to --context".into());
    }
    if wait && mode == Some("context") {
        return Err("--wait needs a turn; --context doesn't start one".into());
    }
    let text_in = match words {
        [one] if one == "-" => read_stdin()?,
        // What follows would be sent as text, an option there too.
        [first, ..] if first == "-" => {
            return Err(
                "- reads the prompt from stdin, and goes alone: options go before it".into()
            );
        }
        _ => words.join(" "),
    };
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
    if !matches!(what.as_str(), "idle" | "turn" | "permission" | "elicitation" | "exit") {
        return Err(format!("--for idle|turn|permission|elicitation|exit, not {what}"));
    }
    let arg = arg.ok_or(USAGE)?;
    let hosts = discover()?;
    let (host, session) = running_session(&hosts, &arg)?;
    let mut conn = Conn::open(host)?;
    wait_on(&mut conn, &session, &what, deadline(timeout), json_out)
}

/// `event wait` on `conn`, the process's, for `what` of `session`.
fn wait_on(
    conn: &mut Conn,
    session: &str,
    what: &str,
    deadline: Option<Instant>,
    json_out: bool,
) -> Result<ExitCode, String> {
    // Subscribed before looking, so nothing happens unseen in between.
    let events =
        ["turn_ended", "permission_request", "elicitation_request", "session_closed", "exited"];
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
    let idle_now = json!({ "event": "idle", "session": session });
    // How the last turn ended, as `shallow_turn` has it: a wait for `idle`
    // that ends on an idle or a closed session exits as if it had waited
    // for that turn (see ADR 8.3 in docs/adr).
    let mut last_turn = Value::Null;
    // What the process answers, and each event, is used and let go of on a
    // stack that takes it (ADR 2.5).
    match what {
        "permission" => {
            let pending = match conn.call(json!({ "cmd": "pending" })) {
                Ok(pending) => pending,
                Err(why) => return conn.ended(why),
            };
            let found = conn.deep(pending, |_, pending| {
                let p = pending["pending"].as_array().into_iter().flatten().find(|p| mine(p));
                p.map(|p| done(p, describe_permission(p))).transpose()
            })??;
            if found.is_some() {
                return Ok(ExitCode::SUCCESS);
            }
        }
        "elicitation" => {
            let pending = match conn.call(json!({ "cmd": "pending" })) {
                Ok(pending) => pending,
                Err(why) => return conn.ended(why),
            };
            let found = conn.deep(pending, |_, pending| {
                let mut waiting = pending["elicitations"].as_array().into_iter().flatten();
                waiting.find(|e| mine(e)).map(|e| done(e, describe_elicitation(e))).transpose()
            })??;
            if found.is_some() {
                return Ok(ExitCode::SUCCESS);
            }
        }
        "idle" => {
            let (is_idle, turn) = match idle(conn, session) {
                Ok(idle) => idle,
                Err(why) => return conn.ended(why),
            };
            if is_idle {
                done(&idle_now, "idle".into())?;
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
                let gone = json!({ "event": "exited", "status": null });
                done(&gone, render::exited(&gone["status"]))?;
                return Ok(ExitCode::SUCCESS);
            }
            Err(e) => return Err(e),
        };
        let ended = conn.deep(e, |conn, e| -> Result<Option<ExitCode>, String> {
            match (e["event"].as_str().unwrap_or_default(), what) {
                ("exited", "exit") => {
                    done(&e, render::exited(&e["status"]))?;
                    return Ok(Some(ExitCode::SUCCESS));
                }
                ("exited", _) => return Err("the agent exited".into()),
                // A closed session counts as idle; there is no next turn or
                // approval in it to wait for.
                ("session_closed", "idle") if mine(&e) => {
                    done(&e, format!("idle: session closed ({})", text(&e["by"])))?;
                    return Ok(Some(ended_as(&last_turn)));
                }
                ("session_closed", "turn" | "permission" | "elicitation") if mine(&e) => {
                    return Err("the session closed".into());
                }
                ("permission_request", "permission") if mine(&e) => {
                    done(&e, describe_permission(&e))?;
                    return Ok(Some(ExitCode::SUCCESS));
                }
                ("elicitation_request", "elicitation") if mine(&e) => {
                    done(&e, describe_elicitation(&e))?;
                    return Ok(Some(ExitCode::SUCCESS));
                }
                ("turn_ended", "turn") if mine(&e) => {
                    let how = e["stop_reason"].as_str().unwrap_or("error");
                    done(&e, format!("turn ended: {how}"))?;
                    return Ok(Some(turn_status(&e)));
                }
                ("turn_ended", "idle") if mine(&e) => {
                    last_turn = shallow_turn(&e);
                    let is_idle = match idle(conn, session) {
                        Ok((is_idle, _)) => is_idle,
                        Err(why) => return conn.ended(why).map(Some),
                    };
                    if is_idle {
                        // Behind the session, the turns that ended after
                        // this one are queued, before the status that says
                        // it is idle: it went idle after the last of them.
                        // Queued after this stack was had, they may need a
                        // deeper one.
                        return conn.deep(e, |conn, e| {
                            let ended = |q: &&Value| q["event"] == "turn_ended" && mine(q);
                            let e = conn.events.iter().rev().find(ended).cloned().unwrap_or(e);
                            let how = e["stop_reason"].as_str().unwrap_or("error");
                            done(&e, format!("idle: {how}"))?;
                            Ok(Some(turn_status(&e)))
                        })?;
                    }
                }
                _ => {}
            }
            Ok(None)
        })??;
        if let Some(status) = ended {
            return Ok(status);
        }
    }
}

/// Whether the session is idle, with no turn running and nothing held (a
/// closed one counts), and how its last turn ended, as `shallow_turn` has
/// it (null for one that has closed).
fn idle(conn: &mut Conn, session: &str) -> Result<(bool, Value), String> {
    let status = conn.call(json!({ "cmd": "status" }))?;
    conn.deep(status, |_, status| {
        let sessions = status["sessions"].as_array().map_or(&[][..], Vec::as_slice);
        match sessions.iter().find(|s| s["session_id"] == session) {
            Some(s) => {
                let is_idle = s["busy"] != true && s["held"].as_u64().unwrap_or(0) == 0;
                (is_idle, shallow_turn(&s["last_turn"]))
            }
            None => (true, Value::Null),
        }
    })
}

/// Of a turn (`turn_ended`, or status's `last_turn`), what `turn_status`
/// reads: nothing the agent nested, so it is held on any stack (ADR 2.5).
fn shallow_turn(turn: &Value) -> Value {
    if !turn.is_object() {
        return Value::Null;
    }
    let error = turn["error"]
        .as_object()
        .map(|e| json!({ "message": e.get("message").and_then(Value::as_str) }));
    json!({ "stop_reason": turn["stop_reason"].as_str(), "error": error })
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
    format!("elicitation {}: {}", text(&e["request"]), text(&e["message"]))
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

/// `queue list`: the held messages and context.
pub(super) fn queue_list(args: &[String]) -> Result<ExitCode, String> {
    let (positional, json_out) = queue_args(args, &[])?;
    let [arg] = positional[..] else { return Err(USAGE.to_owned()) };
    queue(arg, json!({ "cmd": "queue" }), json_out)
}

/// `queue show <session> <message>`: one held message in full, its text,
/// whether it interrupts, and its attachments.
pub(super) fn queue_show(args: &[String]) -> Result<ExitCode, String> {
    let (positional, json_out) = queue_args(args, &[])?;
    let [arg, message] = positional[..] else { return Err(USAGE.to_owned()) };
    let hosts = discover()?;
    let (host, session) = running_session(&hosts, arg)?;
    let response = call(host, &json!({ "cmd": "queue", "session": session, "show": message }))?;
    if json_out {
        print_json(&response_json(response))?;
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
pub(super) fn queue_drop(args: &[String]) -> Result<ExitCode, String> {
    let (positional, json_out) = queue_args(args, &[])?;
    let [arg, message] = positional[..] else { return Err(USAGE.to_owned()) };
    queue(arg, json!({ "cmd": "queue", "drop": message }), json_out)
}

/// `queue clear <session> [--messages] [--context]`: the held messages, the
/// held context, or with neither flag both, dropped; and what is left.
pub(super) fn queue_clear(args: &[String]) -> Result<ExitCode, String> {
    let (positional, json_out) = queue_args(args, &["--messages", "--context"])?;
    let [arg] = positional[..] else { return Err(USAGE.to_owned()) };
    let has = |flag: &str| args.iter().any(|a| a == flag);
    let (messages, context) = (has("--messages"), has("--context"));
    let neither = !messages && !context;
    let req = json!({ "cmd": "queue", "clear": messages || neither, "clear_context": context || neither });
    queue(arg, req, json_out)
}

/// A queue command's positional arguments and `--json`, refusing any other
/// option than `flags`.
fn queue_args<'a>(args: &'a [String], flags: &[&str]) -> Result<(Vec<&'a str>, bool), String> {
    let mut positional = Vec::new();
    let mut json_out = false;
    for a in args {
        match a.as_str() {
            "--json" => json_out = true,
            flag if flags.contains(&flag) => {}
            flag if flag.starts_with("--") => return Err(format!("unknown option: {flag}")),
            _ => positional.push(a.as_str()),
        }
    }
    Ok((positional, json_out))
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

/// A connection to a process, for a test, that the process cuts off,
/// unanswered, at its first `cmd` request, after one event more (at 11),
/// and that can't be caught up with: the process says it cut it off, and
/// that it has records it couldn't write (ADR 10.2).
#[cfg(test)]
pub(super) fn cut_off_at(cmd: &'static str) -> (Conn, thread::JoinHandle<()>) {
    use std::os::unix::net::UnixListener;
    use std::sync::atomic::{AtomicU32, Ordering};
    static MADE: AtomicU32 = AtomicU32::new(0);
    let n = MADE.fetch_add(1, Ordering::Relaxed);
    let dir = env::temp_dir().join(format!("brnr-cut-{}-{n}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let socket = dir.join("h.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let process = thread::spawn(move || {
        for (k, stream) in (1..=2).zip(listener.incoming()) {
            let mut stream = stream.unwrap();
            let lines = BufReader::new(stream.try_clone().unwrap()).lines();
            for line in lines.map_while(Result::ok) {
                let req: Value = serde_json::from_str(&line).unwrap();
                let mut answer = match req["cmd"].as_str().unwrap() {
                    c if c == cmd => {
                        writeln!(stream, "{}", json!({ "event": "turn_ended", "seq": 11 }))
                            .unwrap();
                        break;
                    }
                    "was_cut_off" => json!({ "ok": true, "cut_off": true }),
                    "subscribe" => {
                        json!({ "ok": true, "peer": format!("socket#{k}"), "host_id": "h", "seq": 10 * k })
                    }
                    _ => json!({ "ok": true, "lost": true }),
                };
                answer["req_id"] = req["req_id"].clone();
                writeln!(stream, "{answer}").unwrap();
            }
        }
        let _ = fs::remove_dir_all(&dir);
    });
    let mut conn = Conn::new(UnixStream::connect(&socket).unwrap()).unwrap();
    conn.socket = Some(socket);
    (conn, process)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    /// A process that cuts every new connection off again once it has
    /// answered `was_cut_off`, `subscribe` (the k-th at `10 k`) and `logged`,
    /// the first after one event more (at `10 k + 1`): a command gives up
    /// once it is cut off more than `TRIES` times in a row with nothing read
    /// live in between, and says what it missed.
    #[test]
    fn adr_0010_a_cut_off_while_catching_up_is_tried_again_then_given_up() {
        let dir = env::temp_dir().join(format!("brnr-tries-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let (socket, host_log) = (dir.join("h.sock"), dir.join("host.jsonl"));
        fs::write(&host_log, "").unwrap();
        let listener = UnixListener::bind(&socket).unwrap();
        let process = thread::spawn(move || {
            let mut served = 0;
            for (k, stream) in (1..).zip(listener.incoming()) {
                let mut stream = stream.unwrap();
                served = k;
                let mut lines = BufReader::new(stream.try_clone().unwrap()).lines();
                while let Some(Ok(line)) = lines.next() {
                    let req: Value = serde_json::from_str(&line).unwrap();
                    let answer = match req["cmd"].as_str().unwrap() {
                        "was_cut_off" => json!({ "ok": true, "cut_off": true }),
                        "subscribe" => {
                            json!({ "ok": true, "peer": format!("socket#{k}"), "host_id": "h", "seq": 10 * k })
                        }
                        _ => json!({ "ok": true, "lost": false }),
                    };
                    let mut answer = answer;
                    answer["req_id"] = req["req_id"].clone();
                    writeln!(stream, "{answer}").unwrap();
                    if req["cmd"] == "logged" {
                        if k == 1 {
                            let event = json!({ "event": "turn_ended", "seq": 10 * k + 1 });
                            writeln!(stream, "{event}").unwrap();
                        }
                        break;
                    }
                }
                if k == TRIES as u64 + 2 {
                    break;
                }
            }
            served
        });
        // Cut off before it read anything.
        let (ours, theirs) = UnixStream::pair().unwrap();
        drop(theirs);
        let mut conn = Conn::new(ours).unwrap();
        conn.follow = Some(Follow {
            socket,
            host_id: "h".into(),
            host_log,
            peer: "socket#0".into(),
            events: vec!["turn_ended".into()],
            session: None,
            last: 0,
            tries: 0,
        });
        let live = conn.next_event(None).unwrap().unwrap();
        assert_eq!(live["seq"], 11);
        assert!(conn.next_event(None).is_err());
        let note = conn.missed().unwrap();
        assert_eq!(note["error"], "cut off 4 times in a row while catching up", "{note}");
        assert_eq!((note["after"].as_u64(), note["until"].as_u64()), (Some(40), Some(50)));
        assert_eq!(process.join().unwrap(), 5, "connections after the first");
        let _ = fs::remove_dir_all(&dir);
    }

    /// An event that comes live while a command subscribes again (12, after
    /// `subscribe` at 11) is one it has read: cut off once more before the
    /// next line, it catches up after 12, so it gets each event once, in
    /// order (11 to 13, all in the transcript).
    #[test]
    fn adr_0010_an_event_live_while_subscribing_again_comes_once() {
        let dir = env::temp_dir().join(format!("brnr-once-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let (socket, host_log) = (dir.join("h.sock"), dir.join("host.jsonl"));
        let event = |seq: u64| json!({ "event": "turn_ended", "seq": seq });
        let lines: String = (11..=13)
            .map(|n| format!("{}\n", json!({ "ts": "t", "host_id": "h", "event": event(n) })))
            .collect();
        fs::write(&host_log, lines).unwrap();
        let listener = UnixListener::bind(&socket).unwrap();
        // The k-th connection subscribes at `10 + k`; 12 comes live on the
        // first between `subscribe` and `logged`, which cuts it off, 13 on
        // the second after `logged`.
        let process = thread::spawn(move || {
            for (k, stream) in (1..=2).zip(listener.incoming()) {
                let mut stream = stream.unwrap();
                let lines = BufReader::new(stream.try_clone().unwrap()).lines();
                for line in lines.map_while(Result::ok) {
                    let req: Value = serde_json::from_str(&line).unwrap();
                    let mut answer = match req["cmd"].as_str().unwrap() {
                        "was_cut_off" => json!({ "ok": true, "cut_off": true }),
                        "subscribe" => {
                            json!({ "ok": true, "peer": format!("socket#{k}"), "host_id": "h", "seq": 10 + k })
                        }
                        _ => json!({ "ok": true, "lost": false }),
                    };
                    answer["req_id"] = req["req_id"].clone();
                    writeln!(stream, "{answer}").unwrap();
                    match (k, req["cmd"].as_str().unwrap()) {
                        (1, "subscribe") => writeln!(stream, "{}", event(12)).unwrap(),
                        (1, "logged") => break,
                        (_, "logged") => {
                            writeln!(stream, "{}", event(13)).unwrap();
                            break;
                        }
                        _ => {}
                    }
                }
            }
        });
        // Cut off before it read anything.
        let (ours, theirs) = UnixStream::pair().unwrap();
        drop(theirs);
        let mut conn = Conn::new(ours).unwrap();
        conn.follow = Some(Follow {
            socket,
            host_id: "h".into(),
            host_log,
            peer: "socket#0".into(),
            events: vec!["turn_ended".into()],
            session: None,
            last: 10,
            tries: 0,
        });
        let mut seqs = Vec::new();
        while let Ok(Some(e)) = conn.next_event(None) {
            seqs.push(e["seq"].as_u64().unwrap());
        }
        assert_eq!(seqs, [11, 12, 13]);
        assert_eq!(conn.reconnects, 2);
        process.join().unwrap();
        let _ = fs::remove_dir_all(&dir);
    }

    /// A new subscription that fails, or doesn't say where it starts, is one
    /// more cut-off, tried again under the same limit. A process that goes
    /// meanwhile (its socket refuses) is caught up with to its end, so that
    /// `until` is null only for a process that has gone. One that stays
    /// gives up at the 4th, `until` where it said it cut the peer off
    /// (`last_queued`, 30), or null with the error saying why if it said no
    /// position.
    #[test]
    fn adr_0010_a_new_subscription_that_fails_is_one_more_cut_off() {
        // A process that says it cut the peer off (`last_queued`) and fails
        // each new subscription (no `seq`, or an error), then, after
        // `serves` connections, goes or (`stays`) only stops answering.
        let process = |name: &str, last_queued: Option<u64>, serves: usize, stays: bool| {
            let dir = env::temp_dir().join(format!("brnr-resub-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            let (socket, host_log) = (dir.join("h.sock"), dir.join("host.jsonl"));
            let exited = json!({ "event": "exited", "status": 0, "seq": 12 });
            let record = json!({ "ts": "t", "host_id": "h", "event": exited });
            fs::write(&host_log, format!("{record}\n")).unwrap();
            let listener = UnixListener::bind(&socket).unwrap();
            let at = socket.clone();
            let thread = thread::spawn(move || {
                let mut asked = Vec::new();
                for (k, stream) in (1..=serves).zip(listener.incoming()) {
                    let mut stream = stream.unwrap();
                    let lines = BufReader::new(stream.try_clone().unwrap()).lines();
                    for line in lines.map_while(Result::ok) {
                        let req: Value = serde_json::from_str(&line).unwrap();
                        asked.push(format!("{k}:{}", req["cmd"].as_str().unwrap()));
                        let mut answer = match req["cmd"].as_str().unwrap() {
                            "was_cut_off" => {
                                json!({ "ok": true, "cut_off": true, "last_queued": last_queued })
                            }
                            "subscribe" if k % 2 == 1 => {
                                json!({ "ok": true, "peer": format!("socket#{k}"), "host_id": "h" })
                            }
                            _ => json!({ "ok": false, "error": "connection gone" }),
                        };
                        answer["req_id"] = req["req_id"].clone();
                        // Gone before the command hears, so that it can't
                        // connect again.
                        if req["cmd"] == "subscribe" && k == serves && !stays {
                            fs::remove_file(&at).unwrap();
                        }
                        writeln!(stream, "{answer}").unwrap();
                        if req["cmd"] == "subscribe" {
                            break;
                        }
                    }
                }
                asked
            });
            // Cut off before it read anything.
            let (ours, theirs) = UnixStream::pair().unwrap();
            drop(theirs);
            let mut conn = Conn::new(ours).unwrap();
            conn.follow = Some(Follow {
                socket,
                host_id: "h".into(),
                host_log,
                peer: "socket#0".into(),
                events: vec!["exited".into()],
                session: None,
                last: 10,
                tries: 0,
            });
            (conn, thread, dir)
        };
        let tries = TRIES as usize + 1;
        let all: Vec<String> = (1..=tries).map(|k| format!("{k}:subscribe")).collect();
        let once = |asked: Vec<String>, served: usize| {
            let mut want = vec!["1:was_cut_off".to_owned()];
            want.extend(all.iter().take(served).cloned());
            assert_eq!(asked, want, "was_cut_off asked once, subscribe on each connection");
        };

        // Gone after two: caught up to its end from the transcript.
        let (mut conn, thread, dir) = process("gone", Some(30), 2, false);
        let caught = conn.next_event(None).unwrap().unwrap();
        assert_eq!((&caught["event"], &caught["seq"]), (&json!("exited"), &json!(12)));
        assert!(conn.missed().is_none());
        once(thread.join().unwrap(), 2);
        let _ = fs::remove_dir_all(&dir);

        // Still running: gives up at the 4th, up to where it was cut off.
        for (name, last_queued) in [("stays", Some(30)), ("nowhere", None)] {
            let (mut conn, thread, dir) = process(name, last_queued, tries, true);
            assert!(conn.next_event(None).is_err());
            let note = conn.missed().expect("no events-missed note").clone();
            assert_eq!(
                (note["after"].as_u64(), note["until"].as_u64()),
                (Some(10), last_queued),
                "{note}"
            );
            let error = note["error"].as_str().unwrap();
            assert!(error.starts_with("cut off 4 times in a row while catching up"), "{note}");
            assert_eq!(error.contains("no position"), last_queued.is_none(), "{note}");
            once(thread.join().unwrap(), tries);
            let _ = fs::remove_dir_all(&dir);
        }
    }

    /// `event wait`, cut off before the process answered its first look
    /// (`pending`, or `status` for `idle`), and unable to catch up, says what
    /// it missed and exits 3, as cut off while it waits does.
    #[test]
    fn adr_0010_event_wait_cut_off_looking_says_what_it_missed() {
        for (what, cmd) in
            [("permission", "pending"), ("elicitation", "pending"), ("idle", "status")]
        {
            let (mut conn, process) = cut_off_at(cmd);
            let ended = wait_on(&mut conn, "s", what, None, true);
            assert_eq!(ended, Ok(ExitCode::from(MISSED)), "--for {what}");
            let note = conn.missed().unwrap();
            assert_eq!((&note["event"], &note["after"]), (&json!("events-missed"), &json!(11)));
            process.join().unwrap();
        }
    }

    /// A live process that can't be caught up with (here, it has records it
    /// couldn't write) has the note say up to where: where the command
    /// subscribed again (at 20); null is for a process that has gone.
    #[test]
    fn adr_0010_a_live_process_that_cant_be_caught_up_with_says_until_where() {
        let (mut conn, process) = cut_off_at("status");
        conn.subscribe(&["turn_ended"], None).unwrap();
        assert!(conn.call(json!({ "cmd": "status" })).is_err());
        let note = conn.missed().unwrap();
        assert_eq!(
            (note["after"].as_u64(), note["until"].as_u64()),
            (Some(11), Some(20)),
            "{note}"
        );
        process.join().unwrap();
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
        let record = |event: Value| json!({ "ts": "t", "host_id": "h", "event": event });
        let opened = json!({ "event": "session-opened", "session_id": "s", "file": file });
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
            let f = conn.follow.as_mut().expect("the start channel isn't followed");
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
        assert_eq!(conn.reconnects, 1);
        let asked: Vec<_> = process.join().unwrap().into_iter().map(|r| r["cmd"].clone()).collect();
        assert_eq!(asked, ["was_cut_off", "subscribe", "logged"]);
        let _ = fs::remove_dir_all(&dir);

        // A process with records it couldn't write (`logged` says `lost`):
        // `cut_off_at`'s, asked nothing on its first connection, so that the
        // second is the one catching up makes, subscribed again at 20.
        let (first, process) = cut_off_at("none");
        let socket = first.socket.clone().unwrap();
        drop(first);
        let mut conn = start_channel(&[]);
        conn.follow.as_mut().unwrap().socket = socket;
        let ended = started(&mut conn, &ready, true, true, None);
        assert_eq!(ended, Ok(ExitCode::from(MISSED)));
        let note = conn.missed().unwrap();
        assert_eq!(
            (&note["event"], &note["peer"]),
            (&json!("events-missed"), &json!("socket#start"))
        );
        assert_eq!(
            (note["after"].as_u64(), note["until"].as_u64()),
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
