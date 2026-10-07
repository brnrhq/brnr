//! `brnr host`: owns the agent process and its pipes for the agent's whole
//! life. brnr calls it a process (`brnr ps`, `--pid`); it isn't a command
//! of its own in the usage, and isn't run by hand.
//!
//! Started detached by `brnr acp` for an editor, or by `brnr start` for a
//! headless session (detached, or with `--foreground` as its child), with
//! the one request that says everything on its stdin (see request.rs) and
//! fd 3: the editor link, or the start channel (see start.rs). It is the hub
//! between three kinds of peer:
//!
//! - the agent, over its stdio;
//! - the ACP owner: the editor, through the proxy on the link, or the host
//!   itself when no editor is attached (see acp.rs);
//! - any number of bridges, speaking JSON lines rather than ACP: children
//!   started from the profile, processes on the control socket such as
//!   brnr (see control.rs), and `brnr start` on its start channel.
//!
//! When the editor goes away, the agent gets what a directly spawned agent
//! would have: its stdin is closed and it is killed, with its process group
//! (a signal the proxy catches reaches the agent as that signal). An editor
//! that stops reading for [`LINK_WRITE_TIMEOUT`] counts as gone.

mod acp;
mod control;
mod requests;
mod start;
mod state;
mod strict;

use std::collections::{BTreeSet, HashMap, HashSet};
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, ErrorKind, PipeReader, PipeWriter, Read, Write};
use std::mem::{take, zeroed};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{ChildStderr, ChildStdin, ChildStdout, Command, ExitCode, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use libc::{c_int, pid_t};
use serde_json::{Value, json};

use crate::config::{Experimental, Feature, Log};
use crate::frame;
use crate::json;
use crate::log::{self, Dir, Ids, Logger, Sink};
use crate::paths;
use crate::request::{Prompt, Request, Role};
use crate::signals;

use acp::{AgentRequest, Hold, Pending, Session};
pub use control::{EVENTS, QUIET, check_bridge};
use control::{Closer, Peer};
use requests::{HostRequest, SetupStep};
use start::StartChannel;

/// Where the editor link or the start channel is: a socket either way.
const CHANNEL_FD: RawFd = 3;

/// How long to keep forwarding output after the agent exits, in case
/// something it started still holds its stdout open.
const DRAIN: Duration = Duration::from_millis(500);

/// How long the host, exiting, waits for peers to be sent what's queued for
/// them; one that stopped reading doesn't hold it up for longer.
const PEER_FLUSH: Duration = Duration::from_secs(2);

/// How long the host, exiting, then gives started bridges to exit on their
/// own, their stdin closed, so they can act on the last events.
const BRIDGE_EXIT: Duration = Duration::from_secs(2);

/// How long one write to the proxy may block before the host treats the
/// link as gone. Only the writer thread waits; the host carries on.
const LINK_WRITE_TIMEOUT: Duration = Duration::from_secs(30);

/// `brnr stop`: stdin is closed once what is queued for it is written, then
/// the agent's process group gets SIGTERM, then SIGKILL.
const STOP_TERM_AFTER: Duration = Duration::from_secs(5);
const STOP_KILL_AFTER: Duration = Duration::from_secs(5);

pub fn main(mut args: impl Iterator<Item = OsString>) -> ExitCode {
    // It takes no flags and reads no config: everything is in the request.
    if args.next().is_some() || unsafe { libc::isatty(0) } == 1 || !is_socket(CHANNEL_FD) {
        eprintln!(
            "brnr host is started by brnr start and brnr acp, not by hand \
             (brnr start --foreground runs a session in a terminal)"
        );
        return ExitCode::from(2);
    }
    close_inherited_fds(&[CHANNEL_FD]);
    set_cloexec(CHANNEL_FD);
    let channel = unsafe { UnixStream::from_raw_fd(CHANNEL_FD) };
    let request = match read_request() {
        Ok(request) => request,
        Err((msg, editor)) => {
            let channel = editor.map(|editor| (channel, editor));
            Failure { channel }.report(&msg, 2);
            return ExitCode::from(2);
        }
    };
    if request.headless().is_some_and(|h| h.foreground.is_some()) {
        // In a process group of its own, writing to the terminal.
        signals::write_from_background();
    }
    let editor = matches!(request.role, Role::Editor(_));
    let mut failure = Failure { channel: channel.try_clone().ok().map(|c| (c, editor)) };
    match Host::start(request, channel) {
        Ok(host) => host.run(),
        Err((msg, code)) => {
            failure.report(&msg, code);
            ExitCode::from(code)
        }
    }
}

/// The request on stdin, read to EOF. One cut short (whoever started us
/// died writing it) or that doesn't parse is refused; the error goes on fd 3
/// if the request says what is there (`Some(editor)`).
fn read_request() -> Result<Request, (String, Option<bool>)> {
    let mut bytes = Vec::new();
    io::stdin().lock().read_to_end(&mut bytes).map_err(|e| (format!("stdin: {e}"), None))?;
    serde_json::from_slice(&bytes).map_err(|e| {
        let raw: Value = serde_json::from_slice(&bytes).unwrap_or_default();
        let role = raw["role"].as_object();
        let editor = role.filter(|r| r.len() == 1).map(|r| r.contains_key("editor"));
        (format!("invalid start request: {e}"), editor)
    })
}

fn is_socket(fd: RawFd) -> bool {
    let mut st: libc::stat = unsafe { zeroed() };
    unsafe { libc::fstat(fd, &mut st) == 0 && st.st_mode & libc::S_IFMT == libc::S_IFSOCK }
}

/// Where a startup failure is reported, besides stderr: fd 3, as a frame
/// for the proxy on an editor's link (`true`), or as a line for brnr start
/// on the start channel.
struct Failure {
    channel: Option<(UnixStream, bool)>,
}

impl Failure {
    fn report(&mut self, msg: &str, code: u8) {
        eprintln!("brnr: {}", crate::render::clean(msg));
        match &mut self.channel {
            Some((link, true)) => {
                let report = json!({ "error": msg, "code": code }).to_string();
                let _ = frame::write(link, frame::FAILED, report.as_bytes());
            }
            Some((start, false)) => {
                let _ = writeln!(start, "{}", json!({ "ok": false, "error": msg }));
            }
            None => {}
        }
    }
}

enum Ev {
    /// A frame from the proxy; `None` once the link is gone.
    Link(Option<(u8, Vec<u8>)>),
    /// One line of agent stdout, with its `\n` unless it was the last bytes.
    AgentLine(Vec<u8>),
    AgentStdoutEof,
    AgentStderr(Vec<u8>),
    AgentStderrEof,
    /// The agent has terminated; it has not been reaped yet.
    AgentExited,
    PeerOpened {
        peer: u64,
        tx: control::Queue,
        label: String,
        closer: Closer,
    },
    PeerRequest {
        peer: u64,
        req: Value,
    },
    PeerClosed {
        peer: u64,
    },
    BridgeStderr {
        label: String,
        line: String,
    },
    /// A started bridge has terminated; it has not been reaped yet. It is a
    /// peer until then, whether its stdout is open or not.
    BridgeExited {
        label: String,
        pid: pid_t,
        peer: u64,
    },
    /// A signal sent to the host itself.
    Signal(c_int),
    /// brnr start closed its end of the start channel.
    StartGone {
        peer: u64,
    },
}

enum StopStage {
    Term,
    Kill,
}

struct Host {
    /// For `brnr start --foreground`, in a terminal: it says on stderr how
    /// things go, and exits with the agent's status.
    foreground: bool,
    /// In the foreground, a failed start has been reported on stderr.
    startup_reported: bool,
    /// The start is over: it has committed (the session is open and set up,
    /// and brnr start has been told, see `finish_start`), or an editor
    /// attached. Until then, the process stopping is the start failing.
    start_done: bool,
    info: Value,
    host_id: String,
    /// How long an unanswered permission request waits before it is denied.
    permission_timeout: Option<Duration>,
    agent_pid: pid_t,
    /// Bytes for the agent's stdin. Written on their own thread, so an agent
    /// that stops reading can't stall the host; dropping this closes the
    /// agent's stdin once what's queued is written.
    agent_in: Option<Sender<Vec<u8>>>,
    /// Dropping this makes the stdout reader close the agent's stdout.
    stop_stdout: Option<PipeWriter>,
    /// An editor is attached: from the start, until the link is gone (see
    /// `link_gone`). Not `link.is_some()`: the writer can give up first.
    editor: bool,
    /// Frames for the link writer while an editor is attached. Writes happen
    /// on their own thread so an editor that stops reading can't stall the
    /// host: signals, the control socket and bridges keep working.
    link: Option<Sender<(u8, Vec<u8>)>>,
    link_writer: Option<thread::JoinHandle<()>>,
    /// brnr start's start channel, until the start commits or fails.
    start_channel: Option<StartChannel>,
    /// When the start fails if it hasn't committed: the request's start
    /// timeout.
    start_deadline: Option<Instant>,
    log: Logger,
    sink: Sink,
    rx: Receiver<Ev>,
    sock_path: PathBuf,
    meta_path: PathBuf,
    cwd: PathBuf,

    // ACP state; see acp.rs.
    sessions: Vec<Session>,
    /// Requests whose response creates or ends a session.
    pending: HashMap<String, Pending>,
    /// Sessions this process has taken (or failed to take) the lock of
    /// before the agent opened them: a resume or a load not yet answered.
    claimed: HashMap<String, Hold>,
    /// Every unanswered request to the agent → the session it is about.
    client_requests: HashMap<String, Option<String>>,
    /// Requests the host itself sent the agent as its client.
    host_requests: HashMap<String, HostRequest>,
    /// Requests from the agent to its client that are unanswered.
    agent_requests: Vec<AgentRequest>,
    /// Agent requests the host answered itself while the editor may answer
    /// them too (a cancel): its late answers are dropped (see acp.rs).
    answered: HashSet<String>,
    /// Request id of every unanswered prompt → its session.
    prompt_session: HashMap<String, String>,
    next_id: u64,
    next_permission: u64,
    /// Bytes from the editor after the last complete line.
    editor_buf: Vec<u8>,
    /// How deeply what the host holds may nest, and how deeply the stack it
    /// is running on takes (see `deep`).
    deepest: usize,
    stack: usize,
    /// Headless start: the prompt, sent once the start has committed.
    prompt: Option<Prompt>,
    /// Headless start: the login method to run before the session opens.
    auth: Option<String>,
    /// Headless start: resume this session instead of opening a new one.
    resume: Option<String>,
    /// Headless start: mode and config options to set before the prompt.
    setup: std::collections::VecDeque<SetupStep>,
    /// Headless start: the session being opened.
    starting: Option<String>,
    /// MCP servers for the sessions the host opens, as ACP has them.
    mcp_servers: Vec<Value>,
    /// Close a session idle this long (see `fire_idle_timers`).
    stop_when_idle: Option<Duration>,
    /// In the foreground: show the session's events on stdout, as text or
    /// (`--json`) JSON lines.
    show_events: bool,
    json_events: bool,
    /// What the agent said it can do in `initialize`, and the `_meta` of
    /// its answer, where conventions ahead of the spec are advertised.
    agent_caps: Value,
    agent_meta: Value,
    auth_methods: Value,
    next_message: u64,
    started: Instant,

    // What the request carries for what enforces it.
    /// Stable ACP to the letter (see strict.rs).
    strict: bool,
    /// What to record (ADR 22): `false` turns the logger off; `events`
    /// leaves out the sessions' raw ACP.
    logging: Log,
    /// An editor's process: the actions on its session it allows (ADR 4),
    /// and the process-management behaviours it turns on (ADR 42).
    #[expect(dead_code, reason = "carried for experimental actions (ADR 4)")]
    experimental: BTreeSet<Experimental>,
    features: BTreeSet<Feature>,

    // Bridges; see control.rs.
    peers: HashMap<u64, Peer>,
    bridge_pids: Vec<pid_t>,

    status: Option<c_int>,
    stdout_open: bool,
    stderr_open: bool,
    drain_until: Option<Instant>,
    /// A stop was asked for (brnr stop, a signal, a failed start).
    stop_requested: bool,
    /// The next escalation of a stop.
    stopping: Option<(Instant, StopStage)>,
}

impl Host {
    fn start(req: Request, channel: UnixStream) -> Result<Host, (String, u8)> {
        let recorded = req.recorded();
        let Request { profile, agent, cwd, strict, log: logging, bridges, role } = req;
        let (editor, headless) = match role {
            Role::Editor(editor) => (Some(editor), None),
            Role::Headless(headless) => (None, Some(headless)),
        };
        let program: Vec<OsString> = agent.iter().map(OsString::from).collect();
        if program.is_empty() {
            return Err(("no agent".into(), 2));
        }

        let dir = paths::runtime_dir();
        paths::ensure_private(&dir).map_err(|e| (format!("{}: {e}", dir.display()), 1))?;
        let id = std::process::id().to_string();
        let started = SystemTime::now();
        let host_id = format!("{}-{id}", log::compact_utc(started));
        let sock_path = dir.join(format!("{id}.sock"));
        let meta_path = dir.join(format!("{id}.json"));
        let _ = fs::remove_file(&sock_path);
        let listener = UnixListener::bind(&sock_path)
            .map_err(|e| (format!("{}: {e}", sock_path.display()), 1))?;
        let _ = fs::set_permissions(&sock_path, fs::Permissions::from_mode(0o600));
        let cleanup = || {
            let _ = fs::remove_file(&sock_path);
        };

        let mut cmd = Command::new(&program[0]);
        cmd.args(&program[1..])
            .current_dir(&cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // Its own process group: a terminal's Ctrl-C (in the foreground) is
        // for the host, which stops the agent its own way.
        cmd.process_group(0);
        let mask = editor.as_ref().map(|e| e.sigmask.clone()).unwrap_or_default();
        // std resets the mask in the child; give the agent the editor's.
        unsafe {
            cmd.pre_exec(move || {
                signals::set_mask(&mask);
                signals::restore_for_child();
                Ok(())
            })
        };
        let mut child = cmd.spawn().map_err(|err| {
            cleanup();
            // Same codes a shell uses for "not found" / "not executable".
            let code = if err.kind() == ErrorKind::PermissionDenied { 126 } else { 127 };
            (format!("{}: {err}", program[0].to_string_lossy()), code)
        })?;
        let agent_pid = child.id() as pid_t;

        let ids = Ids {
            host_id: host_id.clone(),
            host_pid: std::process::id(),
            agent_pid: agent_pid as u32,
        };
        let proxy_pid = editor.as_ref().map(|e| e.proxy_pid);
        let log = if logging == Log::Off {
            Logger::disabled()
        } else {
            Logger::start(ids, proxy_pid, logging == Log::All).map_err(|e| {
                cleanup();
                unsafe { libc::kill(agent_pid, libc::SIGKILL) };
                (format!("log: {e}"), 1)
            })?
        };
        let sink = log.sink();

        let info = json!({
            "id": id,
            "host_id": host_id,
            "profile": profile,
            "host_pid": std::process::id(),
            "proxy_pid": proxy_pid,
            "agent_pid": agent_pid,
            "agent": agent,
            "cwd": cwd.to_string_lossy(),
            "host_log": log.host_log().map(|p| p.to_string_lossy().into_owned()),
            "socket": sock_path.to_string_lossy(),
            "started": log::rfc3339(started),
        });
        write_atomic(&meta_path, format!("{info:#}\n").as_bytes());
        // Every process records what it was asked to do (ADR 8).
        sink.note(None, json!({ "event": "started", "info": info, "request": recorded }));

        let (tx, rx) = mpsc::channel();
        let (agent_in, agent_queue) = mpsc::channel();
        let stdin = child.stdin.take().unwrap();
        thread::spawn(move || write_agent_stdin(stdin, agent_queue));
        let (stop_rx, stop_tx) = io::pipe().expect("pipe");
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        let t = tx.clone();
        thread::spawn(move || read_agent_stdout(stdout, stop_rx, t));
        let t = tx.clone();
        thread::spawn(move || read_agent_stderr(stderr, t));
        let t = tx.clone();
        thread::spawn(move || {
            if wait_exited(agent_pid).is_ok() {
                let _ = t.send(Ev::AgentExited);
            }
        });
        let mut link_writer = None;
        let (link, start_channel) = match editor {
            Some(_) => (Some(channel), None),
            None => (None, Some(channel)),
        };
        let link = link.map(|link| {
            let reader = link.try_clone().expect("clone link");
            let t = tx.clone();
            thread::spawn(move || read_link(reader, t));
            let (frames, rx) = mpsc::channel();
            link_writer = Some(thread::spawn(move || write_link(link, rx)));
            frames
        });
        let t = tx.clone();
        thread::spawn(move || control::serve(listener, t));
        let signals = signals::install();
        let t = tx.clone();
        thread::spawn(move || read_signals(signals, t));

        // An editor's process has no start deadline, and none of the rest.
        let start_deadline = (headless.as_ref())
            .and_then(|h| Instant::now().checked_add(Duration::from_secs(h.start_timeout)));
        let h = headless.unwrap_or_default();
        let shown = h.foreground.as_ref();
        let (foreground, show_events) = (shown.is_some(), shown.is_some_and(|f| !f.quiet));
        let json_events = shown.is_some_and(|f| f.json);
        let (experimental, features) = match &editor {
            Some(e) => (e.experimental.clone(), e.features.clone()),
            None => (BTreeSet::new(), BTreeSet::new()),
        };
        let mut host = Host {
            foreground,
            startup_reported: false,
            start_done: false,
            info,
            host_id,
            permission_timeout: h.permission_timeout.map(Duration::from_secs),
            agent_pid,
            agent_in: Some(agent_in),
            stop_stdout: Some(stop_tx),
            editor: link.is_some(),
            link,
            link_writer,
            start_channel: None,
            start_deadline,
            log,
            sink,
            rx,
            sock_path,
            meta_path,
            cwd: cwd.clone(),
            sessions: Vec::new(),
            pending: HashMap::new(),
            claimed: HashMap::new(),
            client_requests: HashMap::new(),
            host_requests: HashMap::new(),
            agent_requests: Vec::new(),
            answered: HashSet::new(),
            prompt_session: HashMap::new(),
            next_id: 0,
            next_permission: 0,
            editor_buf: Vec::new(),
            deepest: 0,
            stack: json::SHALLOW,
            prompt: h.prompt,
            auth: h.auth,
            resume: h.resume,
            setup: requests::setup_steps(h.mode, h.model, h.config.into_iter().collect()),
            starting: None,
            mcp_servers: h.mcp_servers,
            stop_when_idle: h.stop_when_idle.map(Duration::from_secs),
            show_events,
            json_events,
            agent_caps: Value::Null,
            agent_meta: Value::Null,
            auth_methods: Value::Null,
            next_message: 0,
            started: Instant::now(),
            strict,
            logging,
            experimental,
            features,
            peers: HashMap::new(),
            bridge_pids: Vec::new(),
            status: None,
            stdout_open: true,
            stderr_open: true,
            drain_until: None,
            stop_requested: false,
            stopping: None,
        };
        // The first peer: brnr start hears of whatever happens from here.
        if let Some(channel) = start_channel
            && let Err(err) = host.open_start_channel(channel, h.events, &tx)
        {
            host.kill_all();
            let _ = fs::remove_file(&host.sock_path);
            let _ = fs::remove_file(&host.meta_path);
            return Err((format!("start channel: {err}"), 1));
        }
        for (n, bridge) in bridges.iter().enumerate() {
            if let Err(err) = host.start_bridge(n, bridge, &tx) {
                host.kill_all();
                let _ = fs::remove_file(&host.sock_path);
                let _ = fs::remove_file(&host.meta_path);
                return Err((err, 2));
            }
        }
        if host.editor {
            host.start_done = true;
            let ready = json!({ "id": id, "host_pid": std::process::id(), "agent_pid": agent_pid });
            host.send_link(frame::READY, ready.to_string().as_bytes());
        } else {
            host.begin_headless_start();
        }
        if host.foreground {
            eprintln!(
                "brnr: process {id}: started {} in {}; Ctrl-C to stop",
                agent.join(" "),
                cwd.display()
            );
        }
        Ok(host)
    }

    fn run(mut self) -> ExitCode {
        loop {
            if self.status.is_some() && !self.stdout_open && !self.stderr_open {
                break;
            }
            let now = Instant::now();
            if self.drain_until.is_some_and(|t| now >= t) {
                break;
            }
            self.fire_start_timer(now);
            self.fire_stop_timer(now);
            self.fire_permission_timers(now);
            self.fire_idle_timers(now);
            let stop_at = self.stopping.as_ref().map(|(t, _)| *t);
            let permission_at = self.next_permission_deadline();
            let idle_at = self.next_idle_deadline();
            let wake = [self.drain_until, stop_at, self.start_deadline, permission_at, idle_at]
                .into_iter()
                .flatten()
                .min();
            let ev = match wake {
                None => match self.rx.recv() {
                    Ok(ev) => ev,
                    Err(_) => break,
                },
                Some(at) => match self.rx.recv_timeout(at.saturating_duration_since(now)) {
                    Ok(ev) => ev,
                    Err(RecvTimeoutError::Timeout) => continue,
                    Err(RecvTimeoutError::Disconnected) => break,
                },
            };
            let mut ev = Some(ev);
            if self.deep(0, |host| host.handle(ev.take().unwrap())).is_none() {
                // No stack to be had for what the host holds: this one, then.
                self.handle(ev.take().unwrap());
            }
        }
        // Finishing drops what the host holds: on a stack that takes it too.
        let deepest = self.deepest;
        let mut host = Some(self);
        json::on_stack(deepest, || host.take().unwrap().finish())
            .unwrap_or_else(|| host.take().unwrap().finish())
    }

    /// Runs `f` on a stack that takes values `depth` deep, and what the host
    /// holds: this one, or a thread's of its own (see json.rs). `None` if
    /// there is no such stack to be had.
    fn deep<R: Send>(&mut self, depth: usize, f: impl FnOnce(&mut Host) -> R + Send) -> Option<R> {
        let need = depth.max(self.deepest);
        if need <= self.stack {
            return Some(f(self));
        }
        let outer = self.stack;
        let result = json::on_stack(need, || {
            self.stack = need;
            f(self)
        });
        self.stack = outer;
        result
    }

    /// An ACP line as json.rs reads it, on a stack that takes `depth` (see
    /// `deep`). What the host keeps of it may now nest that deep.
    fn read(&mut self, line: &[u8], depth: usize) -> Option<Value> {
        let msg = json::parse(line)?;
        self.deepest = self.deepest.max(depth);
        Some(msg)
    }

    fn finish(mut self) -> ExitCode {
        for i in 0..self.sessions.len() {
            self.flush_agent_message(i);
        }
        if let Some(status) = self.status {
            self.send_link(frame::EXIT, &status.to_be_bytes());
        }
        if !self.start_done {
            self.startup_failed("the agent exited before the session started");
        }
        let status = describe_status(self.status);
        // Held messages that never became a prompt.
        for i in 0..self.sessions.len() {
            self.drop_held(i, "exit");
        }
        self.emit_to_sessions(json!({ "event": "exited", "status": status }));
        // Let go of the sessions as brnr stops listing the process (ADR 3).
        self.claimed.clear();
        for s in &mut self.sessions {
            if let Hold::Owner(lock) = &mut s.hold {
                lock.take();
            }
        }
        let _ = fs::remove_file(&self.sock_path);
        let _ = fs::remove_file(&self.meta_path);
        // Bridges also see EOF on their stdin once we exit.
        // The last events (`exited`) reach the peers before we exit. Started
        // bridges, their stdin closed, should exit then; only those that
        // haven't by BRIDGE_EXIT get SIGTERM.
        self.flush_peers(PEER_FLUSH);
        let until = Instant::now() + BRIDGE_EXIT;
        while !self.bridge_pids.is_empty() {
            match self.rx.recv_timeout(until.saturating_duration_since(Instant::now())) {
                Ok(ev @ Ev::BridgeExited { .. }) => self.handle(ev),
                Ok(_) => {}
                Err(_) => break,
            }
        }
        for &pid in &self.bridge_pids {
            unsafe { libc::kill(pid, libc::SIGTERM) };
        }
        if self.foreground {
            eprintln!("brnr: agent exited: {status}");
        }
        // Let the writer deliver what's queued (it gives up on a proxy that
        // stopped reading; see LINK_WRITE_TIMEOUT).
        self.link = None;
        if let Some(writer) = self.link_writer.take() {
            let _ = writer.join();
        }
        self.log.finish();
        let code = match self.status {
            Some(s) if libc::WIFEXITED(s) => libc::WEXITSTATUS(s) as u8,
            Some(s) => 128 + libc::WTERMSIG(s) as u8,
            None => 1,
        };
        match code {
            _ if !self.foreground => ExitCode::SUCCESS,
            // A start that didn't finish failed, however the agent exited.
            0 if !self.start_done => ExitCode::FAILURE,
            // Otherwise as the agent did, like a shell reports it.
            code => ExitCode::from(code),
        }
    }

    fn handle(&mut self, ev: Ev) {
        match ev {
            Ev::Link(Some((kind, payload))) => match kind {
                frame::DATA => self.editor_bytes(&payload),
                frame::EOF => self.editor_eof(),
                frame::SIGNAL if payload.len() == 4 => {
                    self.signal(i32::from_be_bytes(payload.try_into().unwrap()));
                }
                frame::STDOUT_CLOSED => self.editor_stopped_reading(),
                _ => {}
            },
            Ev::Link(None) => self.link_gone(),
            Ev::AgentLine(line) => self.agent_line(&line),
            Ev::AgentStdoutEof => self.stdout_open = false,
            Ev::AgentStderr(bytes) => {
                self.sink.msg(None, Dir::AgentStderr, &bytes);
                self.send_link(frame::STDERR, &bytes);
            }
            Ev::AgentStderrEof => self.stderr_open = false,
            Ev::AgentExited => {
                // Stopping: whatever the agent left running in its process
                // group goes too. Done before reaping, while the pid (and so
                // the group id) can't be reused.
                if self.stop_requested {
                    self.kill_group(libc::SIGKILL);
                }
                // Reaped here, on the thread that sends signals, so a signal
                // can never reach a recycled pid.
                self.status = reap(self.agent_pid).ok();
                self.agent_in = None;
                self.stopping = None;
                self.drain_until = Some(Instant::now() + DRAIN);
            }
            Ev::PeerOpened { peer, tx, label, closer } => {
                self.peers.insert(peer, Peer::new(tx, label, closer));
            }
            Ev::PeerRequest { peer, req } => self.peer_request(peer, req),
            Ev::PeerClosed { peer } => {
                self.peers.remove(&peer);
            }
            Ev::BridgeStderr { label, line } => {
                self.sink
                    .note(None, json!({ "event": "bridge-stderr", "bridge": label, "text": line }));
            }
            Ev::Signal(sig) => self.host_signal(sig),
            Ev::StartGone { peer } => self.start_gone(peer),
            Ev::BridgeExited { label, pid, peer } => {
                // Reaped here, once it is out of `bridge_pids`, so a signal to
                // a bridge can never reach a recycled pid.
                self.bridge_pids.retain(|&p| p != pid);
                self.peers.remove(&peer);
                let status = reap(pid).ok().filter(|&s| libc::WIFEXITED(s));
                let status = status.map(|s| libc::WEXITSTATUS(s));
                self.sink.note(
                    None,
                    json!({ "event": "bridge-exited", "bridge": label, "status": status }),
                );
            }
        }
    }

    // ---- the editor's side of the link --------------------------------

    fn editor_attached(&self) -> bool {
        self.editor
    }

    /// A signal the proxy caught reaches the agent as that signal.
    fn signal(&mut self, sig: c_int) {
        if self.status.is_none() {
            self.sink.note(None, json!({ "event": "signal", "signal": sig }));
            unsafe { libc::kill(self.agent_pid, sig) };
        }
    }

    fn editor_eof(&mut self) {
        let rest = take(&mut self.editor_buf);
        if !rest.is_empty() {
            self.record_editor_rest(&rest);
            self.write_agent(&rest);
        }
        self.sink.note(None, json!({ "event": "editor-closed-stdin" }));
        self.agent_in = None;
    }

    fn editor_stopped_reading(&mut self) {
        self.sink.note(None, json!({ "event": "editor-stopped-reading" }));
        // The agent gets EPIPE, just as it would directly.
        self.stop_stdout = None;
    }

    /// The editor went away, or stopped reading for [`LINK_WRITE_TIMEOUT`]:
    /// the agent goes too, as if the editor had run it, with whatever it
    /// started in its process group.
    fn link_gone(&mut self) {
        self.link = None;
        if !take(&mut self.editor) || self.status.is_some() {
            return; // The proxy left after the agent.
        }
        self.sink.note(None, json!({ "event": "editor-disconnected" }));
        self.agent_in = None;
        self.kill_group(libc::SIGKILL);
    }

    fn send_link(&mut self, kind: u8, payload: &[u8]) {
        if let Some(link) = &self.link
            && link.send((kind, payload.to_vec())).is_err()
        {
            // The writer gave up and shut the link down, so the reader
            // thread reports it gone (`link_gone`).
            self.link = None;
        }
    }

    fn write_agent(&mut self, bytes: &[u8]) {
        if let Some(agent) = &self.agent_in
            && agent.send(bytes.to_vec()).is_err()
        {
            self.agent_in = None; // The agent closed its stdin.
        }
    }

    // ---- stopping -----------------------------------------------------

    /// A signal to the host itself. HUP, INT, QUIT and TERM stop the agent
    /// gracefully, and kill its process group if a stop is already under
    /// way; USR1 and USR2 are passed on.
    fn host_signal(&mut self, sig: c_int) {
        if self.status.is_some() {
            return;
        }
        self.sink.note(None, json!({ "event": "host-signal", "signal": sig }));
        match sig {
            libc::SIGUSR1 | libc::SIGUSR2 => unsafe {
                libc::kill(self.agent_pid, sig);
            },
            _ if self.stop_requested => {
                if self.foreground {
                    eprintln!("brnr: killing the agent");
                }
                self.kill_group(libc::SIGKILL);
            }
            _ => {
                if self.foreground {
                    eprintln!("brnr: stopping (again to kill)");
                }
                self.begin_stop();
            }
        }
    }

    /// Closes the agent's stdin, then escalates to SIGTERM and SIGKILL (of
    /// its whole process group) if it doesn't exit.
    pub(super) fn begin_stop(&mut self) {
        if self.status.is_some() || self.stop_requested {
            return;
        }
        self.stop_requested = true;
        self.sink.note(None, json!({ "event": "stopping" }));
        self.agent_in = None;
        self.stopping = Some((Instant::now() + STOP_TERM_AFTER, StopStage::Term));
    }

    fn fire_stop_timer(&mut self, now: Instant) {
        let Some((at, stage)) = &self.stopping else { return };
        if now < *at || self.status.is_some() {
            return;
        }
        match stage {
            StopStage::Term => {
                self.kill_group(libc::SIGTERM);
                self.stopping = Some((now + STOP_KILL_AFTER, StopStage::Kill));
            }
            StopStage::Kill => {
                self.kill_group(libc::SIGKILL);
                self.stopping = None;
            }
        }
    }

    /// The start fails when its timeout passes before the commit, rather
    /// than carry on where nobody knows about it (brnr start gives up
    /// waiting a little later).
    fn fire_start_timer(&mut self, now: Instant) {
        if self.start_deadline.is_some_and(|t| now >= t) {
            self.start_deadline = None;
            if !self.start_done && !self.stop_requested {
                self.fail_start("timed out waiting for the session");
            }
        }
    }

    /// Signals the agent's process group (it leads its own; see `start`),
    /// and the agent itself if it has left the group. Only while the agent
    /// hasn't been reaped, so the ids are ours.
    fn kill_group(&self, sig: c_int) {
        if self.status.is_some() {
            return;
        }
        unsafe {
            let group = libc::kill(-self.agent_pid, sig) == 0;
            if !group || libc::getpgid(self.agent_pid) != self.agent_pid {
                libc::kill(self.agent_pid, sig);
            }
        }
    }

    /// Startup failed after the agent was spawned.
    fn kill_all(&mut self) {
        self.kill_group(libc::SIGKILL);
        for &pid in &self.bridge_pids {
            unsafe { libc::kill(pid, libc::SIGTERM) };
        }
    }
}

fn describe_status(status: Option<c_int>) -> Value {
    match status {
        None => Value::Null,
        Some(s) if libc::WIFEXITED(s) => json!({ "code": libc::WEXITSTATUS(s) }),
        Some(s) => json!({ "signal": libc::WTERMSIG(s) }),
    }
}

// ---- threads -----------------------------------------------------------

/// Writes queued frames to the proxy until the queue closes or a write
/// fails or times out. Then the link is shut down, so the reader thread
/// sees it gone too: an editor that stopped reading is an editor gone.
fn write_link(mut link: UnixStream, frames: Receiver<(u8, Vec<u8>)>) {
    let _ = link.set_write_timeout(Some(LINK_WRITE_TIMEOUT));
    for (kind, payload) in frames {
        if frame::write(&mut link, kind, &payload).is_err() {
            let _ = link.shutdown(std::net::Shutdown::Both);
            return;
        }
    }
}

/// Writes queued bytes to the agent's stdin until the queue closes or the
/// agent closes its end. Dropping `stdin` on the way out closes it.
fn write_agent_stdin(mut stdin: ChildStdin, queue: Receiver<Vec<u8>>) {
    for bytes in queue {
        if stdin.write_all(&bytes).is_err() {
            return;
        }
    }
}

fn read_signals(mut signals: PipeReader, tx: Sender<Ev>) {
    let mut sig = [0];
    loop {
        match signals.read(&mut sig) {
            Ok(1) => {}
            Err(err) if err.kind() == ErrorKind::Interrupted => continue,
            _ => return,
        }
        if tx.send(Ev::Signal(sig[0] as c_int)).is_err() {
            return;
        }
    }
}

fn read_link(link: UnixStream, tx: Sender<Ev>) {
    let mut reader = BufReader::new(link);
    loop {
        match frame::read(&mut reader) {
            Ok(Some(frame)) => {
                if tx.send(Ev::Link(Some(frame))).is_err() {
                    return;
                }
            }
            Ok(None) | Err(_) => {
                let _ = tx.send(Ev::Link(None));
                return;
            }
        }
    }
}

/// Splits the agent's stdout into lines until EOF, or until `stop` closes,
/// which drops our end so the agent gets EPIPE like it would directly.
fn read_agent_stdout(mut out: ChildStdout, stop: PipeReader, tx: Sender<Ev>) {
    let mut fds = [
        libc::pollfd { fd: out.as_raw_fd(), events: libc::POLLIN, revents: 0 },
        libc::pollfd { fd: stop.as_raw_fd(), events: libc::POLLIN, revents: 0 },
    ];
    let mut buf = vec![0; 64 * 1024];
    let mut line = Vec::new();
    loop {
        if unsafe { libc::poll(fds.as_mut_ptr(), 2, -1) } < 0 {
            if io::Error::last_os_error().kind() == ErrorKind::Interrupted {
                continue;
            }
            break;
        }
        if fds[1].revents != 0 {
            break;
        }
        if fds[0].revents == 0 {
            continue;
        }
        let n = match out.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(err) if err.kind() == ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        for &b in &buf[..n] {
            line.push(b);
            if b == b'\n' && tx.send(Ev::AgentLine(take(&mut line))).is_err() {
                return;
            }
        }
    }
    if !line.is_empty() {
        let _ = tx.send(Ev::AgentLine(line));
    }
    let _ = tx.send(Ev::AgentStdoutEof);
}

fn read_agent_stderr(err: ChildStderr, tx: Sender<Ev>) {
    let mut reader = BufReader::new(err);
    let mut line = Vec::new();
    while let Ok(n) = reader.read_until(b'\n', &mut line) {
        if n == 0 || tx.send(Ev::AgentStderr(take(&mut line))).is_err() {
            break;
        }
    }
    let _ = tx.send(Ev::AgentStderrEof);
}

// ---- process plumbing --------------------------------------------------

/// Closes every fd we inherited except stdio and `keep`, so the host holds
/// nothing of the editor's.
fn close_inherited_fds(keep: &[RawFd]) {
    let fds: Vec<RawFd> = match fs::read_dir("/dev/fd") {
        Ok(dir) => dir.filter_map(|e| e.ok()?.file_name().to_str()?.parse().ok()).collect(),
        Err(_) => return,
    };
    for fd in fds {
        if fd > 2 && !keep.contains(&fd) {
            unsafe { libc::close(fd) };
        }
    }
}

fn set_cloexec(fd: RawFd) {
    unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
}

fn write_atomic(path: &std::path::Path, bytes: &[u8]) {
    let tmp = path.with_extension("json.tmp");
    let written = File::create(&tmp).and_then(|mut f| f.write_all(bytes));
    if written.is_ok() {
        let _ = fs::rename(&tmp, path);
    }
}

/// Blocks until `pid` has terminated, without reaping it.
fn wait_exited(pid: pid_t) -> io::Result<()> {
    unsafe {
        let mut info: libc::siginfo_t = zeroed();
        let flags = libc::WEXITED | libc::WNOWAIT;
        while libc::waitid(libc::P_PID, pid as libc::id_t, &mut info, flags) < 0 {
            let err = io::Error::last_os_error();
            if err.kind() != ErrorKind::Interrupted {
                return Err(err);
            }
        }
    }
    Ok(())
}

fn reap(pid: pid_t) -> io::Result<c_int> {
    let mut status = 0;
    while unsafe { libc::waitpid(pid, &mut status, 0) } < 0 {
        let err = io::Error::last_os_error();
        if err.kind() != ErrorKind::Interrupted {
            return Err(err);
        }
    }
    Ok(status)
}

/// Whether process `pid` exists (it may belong to someone else).
pub fn alive(pid: i64) -> bool {
    pid > 0
        && (unsafe { libc::kill(pid as pid_t, 0) } == 0
            || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM))
}

/// A JSON-RPC id as a map key: `1` and `"1"` stay distinct.
fn id_key(id: &Value) -> String {
    id.to_string()
}

fn text_block(text: &str) -> Value {
    json!({ "type": "text", "text": text })
}
