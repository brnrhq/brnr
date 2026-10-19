//! `brnr event notify (<session> | --pid <pid> | --stdin) [--events <a,b,...>]
//! -- <command> [args...]`: runs a command once per event of a session, or of
//! every session in a process, for notifications.
//!
//! The event is in the command's environment and, as JSON, on its stdin;
//! nothing is substituted into the command line, so what an agent writes
//! can't become arguments:
//!
//! - `BRNR_EVENT`: the event name; `BRNR_TEXT`: it as `brnr event watch` would
//!   show it; `BRNR_TITLE`: the session's title, else its id;
//! - `BRNR_SESSION_ID`, `BRNR_REQUEST` (an approval's or an elicitation's
//!   handle),
//!   `BRNR_MESSAGE` (the agent's last message in the session);
//! - `BRNR_PID`, the process.
//!
//! The text, title and message have control characters escaped, as `event
//! watch` shows them, and are cut at 32 KiB; the event on stdin has them whole.
//!
//! `--events` is read as for `event watch`, but its default (and `default`)
//! is `permission_request`, `elicitation_request` (ADR 68), `turn_ended`,
//! `exited`. It exits when the process
//! does (or the session closes).
//!
//! Commands run one at a time, in order, each in a process group of its
//! own, and no event is read while one runs: one that takes too long makes
//! notify fall behind, and the process cuts it off as it does any peer that
//! does (ADR 6 in docs/adr). On the socket, cut off, notify lets the command
//! finish, then catches up from the transcript and goes on (ADR 69); unable
//! to, it says what it missed and exits 3. Its connection ended otherwise
//! (the process killed), or stopped by a signal (HUP, INT, QUIT or TERM),
//! notify stops the command it is running (SIGTERM to its group, SIGKILL
//! [`STOP_WAIT`] later), says on stderr that it was cut off and that no more
//! notifications come, and exits non-zero.
//!
//! With `--stdin` it reads the events from its stdin, one per line, instead of
//! connecting: a started bridge's transport (ADR 35 and 36 in docs/adr). As a
//! bridge in a profile it is
//! `command = ["brnr", "event", "notify", "--stdin", "--", …]`, and `BRNR_PID`
//! is the one the process gives it. The bridge's `events`, if the profile
//! limits them, must include `agent_message`, `session_changed` and `exited`
//! for the environment and the end. Its stdin ending before `exited` is being
//! cut off; the process cuts off a bridge that falls behind with SIGTERM, and a
//! started bridge's stderr is in the host log (`bridge-stderr`): a started
//! bridge doesn't catch up (ADR 69). On the socket, the connection closing
//! before `exited` is being cut off, which notify also looks for while a
//! command runs, and asks the process about.

use std::collections::{HashMap, VecDeque};
use std::env;
use std::io::{self, BufRead, PipeReader, Read, Stdin, Write};
use std::os::fd::{AsFd, AsRawFd};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, ExitCode, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use libc::c_int;
use serde_json::Value;

use crate::{host, json, render, signals, sys};

use super::catch_up::MISSED;
use super::talk::Conn;
use super::{USAGE, discover, events_arg, session_or_pid};

const DEFAULT_EVENTS: &[&str] =
    &["permission_request", "elicitation_request", "turn_ended", "exited"];

/// The most of the agent's text one variable gets. Linux refuses to start a
/// program with a variable over 128 KiB, and macOS one whose environment and
/// arguments come to over 1 MiB.
const ENV_TEXT_MAX: usize = 32 << 10;

/// The signals that end notify, once it has stopped the command it runs.
const STOPPING: &[c_int] = &[libc::SIGHUP, libc::SIGINT, libc::SIGQUIT, libc::SIGTERM];

/// How long a command notify stops has after SIGTERM, before SIGKILL.
const STOP_WAIT: Duration = Duration::from_secs(2);

/// How often, while a command runs, notify looks whether the process has
/// hung up the connection.
const HANGUP_CHECK: Duration = Duration::from_millis(100);

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
    // From here a signal doesn't end notify before it has stopped what it
    // runs, which is in a process group of its own.
    let caught = signals::catch(STOPPING);
    let Events { source, process, only, mut titles } =
        Events::open(stdin, session.as_deref(), pid.as_deref(), &wanted)?;
    // Looked at while a command runs, until the process hangs up.
    let mut hangup = source.socket().0;
    let (tx, mut rx) = mpsc::channel();
    let (ask, asks) = mpsc::channel();
    let t = tx.clone();
    thread::spawn(move || read_events(source, asks, t));
    let t = tx.clone();
    thread::spawn(move || forward_signals(caught, t));
    let mut last_message: HashMap<String, String> = HashMap::new();
    let options = render::Options { session: false, time: false };
    // What was left once the process hung up, read while a command ran, and
    // why there was no more.
    let mut ahead: VecDeque<Value> = VecDeque::new();
    let mut end: Option<String> = None;
    let ended = loop {
        let e = if let Some(e) = ahead.pop_front() {
            e
        } else if let Some(why) = end.take() {
            break Err(cut_off(&why, None));
        } else {
            let _ = ask.send(Ask::Next);
            match next(&rx) {
                Ok((e, socket)) => {
                    // Caught up, on a new connection (ADR 69).
                    if socket.is_some() {
                        hangup = socket;
                    }
                    e
                }
                Err(Stop::Ended(why)) => break Err(cut_off(&why, None)),
                Err(Stop::Missed(note)) => {
                    let text = render::event(&note, &options).unwrap_or_default();
                    errln!("brnr: {text}; no more notifications");
                    break Ok(ExitCode::from(MISSED));
                }
            }
        };
        // Run for, and let go of, on a stack that takes it (ADR 26). How
        // notify ends, if it does here.
        let ended = json::holding(e, |e| -> Option<Result<ExitCode, String>> {
            // Mutably: this may run on a thread of its own, and a `Receiver`
            // isn't `Sync`.
            let rx = &mut rx;
            let name = e["event"].as_str().unwrap_or_default().to_owned();
            let session = e["session"].as_str().unwrap_or_default().to_owned();
            if only.as_deref().is_some_and(|s| !session.is_empty() && s != session) {
                return None;
            }
            match name.as_str() {
                "agent_message" => {
                    last_message
                        .insert(session.clone(), e["text"].as_str().unwrap_or_default().to_owned());
                }
                "session_changed" if e["what"] == "title" => {
                    let title = e["value"].as_str().unwrap_or_default().to_owned();
                    titles.insert(session.clone(), title);
                }
                _ => {}
            }
            if events.contains(&name) {
                let text = render::event(&e, &options);
                let title = titles.get(&session).cloned().unwrap_or_else(|| session.clone());
                let message = last_message.get(&session).cloned().unwrap_or_default();
                let vars = [
                    ("BRNR_EVENT", name.clone()),
                    ("BRNR_TEXT", env_text(&text.unwrap_or_default())),
                    ("BRNR_TITLE", env_text(&title)),
                    ("BRNR_SESSION_ID", session.clone()),
                    ("BRNR_REQUEST", e["request"].as_str().unwrap_or_default().to_owned()),
                    ("BRNR_MESSAGE", env_text(&message)),
                    ("BRNR_PID", process.clone()),
                ];
                let mut running = Running::start(&command, &e, &vars, &tx);
                // Asked for the rest, once the process hung up; not got it yet.
                let mut draining = false;
                while running.is_some() || draining {
                    match rx.recv_timeout(HANGUP_CHECK) {
                        Ok(Msg::Done(pid)) if running.as_ref().is_some_and(|r| r.pid() == pid) => {
                            running.take().unwrap().reap();
                        }
                        Ok(Msg::Signal(sig)) => {
                            let stopped = running.take().map(|r| r.stop(rx));
                            return Some(Err(cut_off(
                                &render::signal(sig.into()),
                                stopped.as_deref(),
                            )));
                        }
                        // What the process sent before it hung up says whether
                        // that was the end; else the process says whether it
                        // cut notify off, which catches up once the command is
                        // done (ADR 69).
                        Ok(Msg::Rest(rest, why, cut)) => {
                            draining = false;
                            let end_too = rest.iter().any(|e| ends(e, only.as_deref()));
                            if !end_too && !cut {
                                json::free(Value::Array(rest));
                                let stopped = running.take().map(|r| r.stop(rx));
                                return Some(Err(cut_off(&why, stopped.as_deref())));
                            }
                            ahead.extend(rest);
                            if end_too {
                                end = Some(why);
                            }
                        }
                        Err(RecvTimeoutError::Timeout)
                            if running.is_some() && hangup.as_ref().is_some_and(hung_up) =>
                        {
                            hangup = None;
                            draining = true;
                            let _ = ask.send(Ask::Rest);
                        }
                        _ => {}
                    }
                }
            }
            ends(&e, only.as_deref()).then_some(Ok(ExitCode::SUCCESS))
        });
        match ended {
            Ok(None) => {}
            Ok(Some(ended)) => break ended,
            Err(why) => break Err(why),
        }
    };
    // What it read ahead may be deep too.
    json::free(Value::Array(ahead.into()));
    ended
}

/// Whether `e` is the last event notify waits for: `exited`, or with `only`
/// that session's `session_closed`.
fn ends(e: &Value, only: Option<&str>) -> bool {
    e["event"] == "exited"
        || (e["event"] == "session_closed" && only.is_some_and(|s| e["session"] == s))
}

/// Why notify stops before the end: `why`, and the command it stopped.
fn cut_off(why: &str, stopped: Option<&str>) -> String {
    let stopped = stopped.map(|s| format!("; stopped {s}")).unwrap_or_default();
    format!("cut off ({why}){stopped}; no more notifications")
}

/// What reaches notify's loop.
enum Msg {
    /// The next event, as asked for, and the connection, if it is a new one
    /// (caught up, ADR 69).
    Event(Value, Option<UnixStream>),
    /// No more events, as asked for the next: why.
    End(Stop),
    /// What there was still to read once the process had hung up, why there
    /// was no more, and whether the process had cut notify off.
    Rest(Vec<Value>, String, bool),
    Signal(c_int),
    /// The command with this pid has exited; it isn't reaped yet.
    Done(i32),
}

/// Why there are no more events.
enum Stop {
    /// The connection or stdin ended.
    Ended(String),
    /// Cut off, it couldn't catch up: what it missed (ADR 69).
    Missed(Value),
}

/// What the reader is asked for.
enum Ask {
    /// The next event.
    Next,
    /// Everything left, once the process has hung up: no more than it had
    /// sent by then, without catching up.
    Rest,
}

/// The next event: one at a time, as notify asks, so it is read no faster
/// than notify gets through them, and the process sees it fall behind.
fn read_events(mut source: Source, asks: Receiver<Ask>, tx: Sender<Msg>) {
    // How many times the connection the loop has was made again.
    let mut told = 0;
    for ask in asks {
        if matches!(ask, Ask::Rest) {
            let (rest, why, cut) = source.rest();
            // Not sent, notify has ended: let go of here.
            if let Err(mpsc::SendError(Msg::Rest(rest, ..))) = tx.send(Msg::Rest(rest, why, cut)) {
                json::free(Value::Array(rest));
            }
            if cut {
                continue;
            }
            return;
        }
        loop {
            match source.next() {
                Ok(Some(e)) => {
                    let (socket, made) = source.socket();
                    let socket = socket.filter(|_| made != told);
                    told = made;
                    // Not sent, notify has ended: let go of here.
                    if let Err(mpsc::SendError(Msg::Event(e, _))) = tx.send(Msg::Event(e, socket)) {
                        json::free(e);
                    }
                    break;
                }
                Ok(None) => {}
                Err(why) => {
                    let _ = tx.send(Msg::End(match source.missed() {
                        Some(note) => Stop::Missed(note),
                        None => Stop::Ended(why),
                    }));
                    return;
                }
            }
        }
    }
}

/// The next event, waited for with no command running, and its connection
/// if it is a new one; `Err` for why there is none.
fn next(rx: &Receiver<Msg>) -> Result<(Value, Option<UnixStream>), Stop> {
    loop {
        match rx.recv() {
            Ok(Msg::Event(e, socket)) => return Ok((e, socket)),
            Ok(Msg::End(stop)) => return Err(stop),
            Ok(Msg::Signal(sig)) => return Err(Stop::Ended(render::signal(sig.into()))),
            Ok(_) => {}
            Err(_) => return Err(Stop::Ended("no more events".to_owned())),
        }
    }
}

fn forward_signals(mut caught: PipeReader, tx: Sender<Msg>) {
    let mut sig = [0];
    loop {
        match caught.read(&mut sig) {
            Ok(1) => {}
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            _ => return,
        }
        if tx.send(Msg::Signal(c_int::from(sig[0]))).is_err() {
            return;
        }
    }
}

/// Whether the process has hung up `socket`: it closed the connection, as
/// when it cuts a peer off, or exited. What it sent before may still be
/// unread.
fn hung_up(socket: &UnixStream) -> bool {
    let mut p = libc::pollfd { fd: socket.as_raw_fd(), events: libc::POLLIN, revents: 0 };
    // SAFETY: p is a valid pollfd, and the count says there is one.
    unsafe { libc::poll(&mut p, 1, 0) > 0 && p.revents & libc::POLLHUP != 0 }
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
    Socket(Box<Conn>),
    /// A started bridge's (ADR 35 in docs/adr).
    Stdin(Stdin),
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
            let source = Source::Stdin(io::stdin());
            return Ok(Events { source, process, only: None, titles: HashMap::new() });
        }
        let hosts = discover()?;
        let (host, only) = session_or_pid(&hosts, session, pid)?;
        Events::on(Conn::open(host)?, host.id(), only, wanted)
    }

    /// Subscribed to `wanted` on `conn`, process `process`'s.
    fn on(
        mut conn: Conn,
        process: &str,
        only: Option<String>,
        wanted: &[&str],
    ) -> Result<Events, String> {
        conn.subscribe(wanted, only.as_deref())?;
        // Titles the agent gave before we subscribed. Cut off before it
        // answered, and unable to catch up, notify goes on with none: what
        // it read meanwhile comes first, then what it missed (ADR 69).
        let status = match conn.call(serde_json::json!({ "cmd": "status" })) {
            Ok(status) => status,
            Err(_) if conn.missed().is_some() => Value::Null,
            Err(why) => return Err(why),
        };
        // Read and let go of on a stack that takes it (ADR 26).
        let titles = conn.deep(status, |_, status| {
            let sessions = status["sessions"].as_array().into_iter().flatten();
            sessions
                .filter_map(|s| {
                    Some((s["session_id"].as_str()?.to_owned(), s["title"].as_str()?.to_owned()))
                })
                .collect::<HashMap<String, String>>()
        })?;
        Ok(Events {
            source: Source::Socket(Box::new(conn)),
            process: process.to_owned(),
            only,
            titles,
        })
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
                match stdin.lock().read_until(b'\n', &mut line) {
                    Ok(0) => Err("stdin closed".into()),
                    // As the host reads it (see json.rs), as the socket's.
                    // A line that isn't an event is let go of there too.
                    Ok(_) => {
                        let depth = json::depth(&line);
                        let event =
                            || json::parse::<Value>(&line).filter(|e| e.get("event").is_some());
                        json::on_stack(depth, event)
                            .ok_or_else(|| format!("no stack to be had for a message {depth} deep"))
                    }
                    Err(e) => Err(format!("stdin: {e}")),
                }
            }
        }
    }

    /// The connection, to see the process hang up while a command runs,
    /// and how many times it was made again.
    fn socket(&self) -> (Option<UnixStream>, u32) {
        match self {
            Source::Socket(conn) => {
                let (socket, made) = conn.socket();
                (socket.ok(), made)
            }
            Source::Stdin(_) => (None, 0),
        }
    }

    /// What is left before the process hung up, why it did, and whether it
    /// cut notify off (ADR 69).
    fn rest(&mut self) -> (Vec<Value>, String, bool) {
        match self {
            Source::Socket(conn) => conn.rest(),
            Source::Stdin(_) => (Vec::new(), "stdin closed".into(), false),
        }
    }

    /// What it missed, cut off, if it couldn't catch up.
    fn missed(&self) -> Option<Value> {
        match self {
            Source::Socket(conn) => conn.missed().cloned(),
            Source::Stdin(_) => None,
        }
    }
}

/// The agent's text for the environment: as `event watch` would show it (see
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

/// The command running for one event.
struct Running {
    child: Child,
    program: String,
    /// `sh (turn_ended)`, for saying it was stopped.
    what: String,
}

impl Running {
    /// Starts the command for `event`, in a process group of its own, with
    /// the event on its stdin and its stdout sent to our stderr: as a
    /// bridge, our stdout is read by the process as requests. Its exit is
    /// sent on `tx` as `Done`, unreaped. `None` if it couldn't start, which
    /// is said.
    fn start(
        command: &[String],
        event: &Value,
        vars: &[(&str, String)],
        tx: &Sender<Msg>,
    ) -> Option<Running> {
        let stdout = Stdio::from(std::io::stderr().as_fd().try_clone_to_owned().expect("stderr"));
        let mut cmd = Command::new(&command[0]);
        cmd.args(&command[1..])
            .envs(vars.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::piped())
            .stdout(stdout)
            .process_group(0);
        let child = signals::as_found(&mut cmd).spawn();
        let mut child = match child {
            Ok(child) => child,
            Err(err) => {
                eprintln!("brnr event notify: {}: {err}", command[0]);
                return None;
            }
        };
        if let Some(mut stdin) = child.stdin.take() {
            // On a thread of its own: a command that doesn't read it holds
            // nothing up.
            let line = event.to_string();
            thread::spawn(move || {
                let _ = writeln!(stdin, "{line}");
            });
        }
        let (pid, t) = (child.id() as i32, tx.clone());
        thread::spawn(move || {
            if host::wait_exited(pid).is_ok() {
                let _ = t.send(Msg::Done(pid));
            }
        });
        let what = format!("{} ({})", command[0], event["event"].as_str().unwrap_or("?"));
        Some(Running { child, program: command[0].clone(), what })
    }

    fn pid(&self) -> i32 {
        self.child.id() as i32
    }

    /// It has exited: reaped, and said if it failed.
    fn reap(mut self) {
        if let Ok(status) = self.child.wait()
            && !status.success()
        {
            eprintln!("brnr event notify: {} exited with {status}", self.program);
        }
    }

    /// Stops it, with whatever it started in its process group: SIGTERM,
    /// then once it has exited, or [`STOP_WAIT`] later, SIGKILL; then
    /// reaped. Unreaped until then, its pid is still its group's. Returns
    /// what it was.
    fn stop(mut self, rx: &Receiver<Msg>) -> String {
        let pid = self.pid();
        sys::kill(-pid, libc::SIGTERM);
        let until = Instant::now() + STOP_WAIT;
        loop {
            match rx.recv_timeout(until.saturating_duration_since(Instant::now())) {
                Ok(Msg::Done(done)) if done == pid => break,
                // Stopped while draining: what is left goes with it.
                Ok(Msg::Rest(rest, ..)) => json::free(Value::Array(rest)),
                Ok(Msg::Event(e, _)) => json::free(e),
                Ok(_) => {}
                Err(_) => break,
            }
        }
        sys::kill(-pid, libc::SIGKILL);
        let _ = self.child.wait();
        self.what
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cut off before the process answered its `status`, and unable to catch
    /// up, notify goes on: the event it read meanwhile, left once the process
    /// hung up, then what it missed (ADR 69).
    #[test]
    fn adr_0069_notify_cut_off_looking_runs_for_what_it_read_then_says_what_it_missed() {
        let (conn, process) = crate::ctl::talk::cut_off_at("status");
        let mut events = Events::on(conn, "1", None, DEFAULT_EVENTS).unwrap();
        let (rest, _, cut) = events.source.rest();
        assert_eq!((rest.len(), &rest[0]["seq"], cut), (1, &Value::from(11), true));
        assert!(events.source.next().is_err());
        assert_eq!(events.source.missed().unwrap()["after"], 11);
        process.join().unwrap();
    }
}
