//! `brnr host`: owns the agent process and its pipes for the agent's whole
//! life. brnr calls it a process (`brnr process list`, `--pid`); it isn't a
//! command of its own in the usage, and isn't run by hand.
//!
//! Started detached by `brnr acp` for an editor, or by `brnr session new` for
//! a headless session (detached, or with `--foreground` as its child), with
//! the one request that says everything on its stdin (see request.rs) and
//! fd 3: the editor link, or the start channel (see start.rs). An editor's
//! process also has the proxy's signal link on fd 4 (see frame.rs). It is
//! the hub between three kinds of peer:
//!
//! - the agent, over its stdio;
//! - the ACP owner: the editor, through the proxy on the link, or the host
//!   itself when no editor is attached (see acp.rs);
//! - any number of bridges, speaking JSON lines rather than ACP: children
//!   started from the profile, processes on the control socket such as
//!   brnr (see control.rs), and `brnr session new` on its start channel.
//!
//! When the editor goes away, the agent gets what a directly spawned agent
//! would have: its stdin is closed and it is killed, with its process group
//! (a signal the proxy catches reaches the agent as that signal, at once:
//! the signal link is read whatever holds the link back). An editor that is
//! slow, or stops reading, holds the agent back as a pipe would, for as long
//! as it does, and an agent that stops reading its stdin holds the editor
//! back likewise (see flow.rs).

mod acp;
mod control;
mod death;
mod display;
mod experimental;
mod flow;
#[doc(hidden)]
pub mod fuzz;
mod requests;
mod start;
mod state;
mod strict;

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, BufReader, ErrorKind, PipeReader, PipeWriter, Read, Write};
use std::mem::{take, zeroed};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, ExitCode, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

use libc::{c_int, pid_t};
use serde_json::{Value, json};

use crate::config::{Bridge, Experimental, Feature, Log};
use crate::frame;
use crate::json;
use crate::log::{self, Dir, Ids, Logger, Sink};
use crate::paths;
use crate::request::{Prompt, Request, Role, Settings};
use crate::schema::AuthMethod;
use crate::signals;
use crate::sys;

use acp::{AgentRequest, ClientRequest, Hold, Pending, Session};
use control::{Closer, Peer};
pub use control::{EVENTS, QUIET, check_bridge};
use display::Display;
use flow::{Backlog, LINE_BYTES};
use requests::Capabilities;
use start::StartChannel;

/// Where the editor link or the start channel is: a socket either way.
const CHANNEL_FD: RawFd = 3;

/// Where an editor's process has the proxy's signal link (see frame.rs).
const SIGNAL_LINK_FD: RawFd = 4;

/// How long to keep forwarding output after the agent exits, in case
/// something it started still holds its stdout open.
const DRAIN: Duration = Duration::from_millis(500);

/// How long the host, exiting, waits for peers to be sent what's queued for
/// them; one that stopped reading doesn't hold it up for longer.
const PEER_FLUSH: Duration = Duration::from_secs(2);

/// How long the host, exiting, waits for its logger to have written what
/// it recorded before it lets go of its sessions (`wait_logged`).
const LOG_FLUSH: Duration = Duration::from_secs(2);

/// How long the host, exiting, then gives started bridges to exit on their
/// own, their stdin closed, so they can act on the last events.
const BRIDGE_EXIT: Duration = Duration::from_secs(2);

/// Events the reader threads may have queued for the event loop before
/// they wait for it (ADR 6). The agent's output and the editor's input are
/// held to a number of bytes as well (see flow.rs).
const EVENTS_QUEUED: usize = 1024;

/// The agent's last lines on stderr, for a failed start (ADR 10), and how
/// much of each is kept.
const STDERR_TAIL: usize = 20;
const STDERR_LINE: usize = 2000;

/// The most of the agent's stderr read at once, and of a line of it the host
/// log takes as one record: a longer line is recorded in pieces of this
/// (ADR 51). Nobody reads stderr as lines but people, and the host log; what
/// is read goes on to the editor or the terminal at once, newline or not
/// (ADR 62).
const PIECE: usize = 64 << 10;

/// `brnr process stop`: stdin is closed once what is queued for it is written,
/// then the agent's process group gets SIGTERM, then SIGKILL.
const STOP_TERM_AFTER: Duration = Duration::from_secs(5);
const STOP_KILL_AFTER: Duration = Duration::from_secs(5);

pub fn main(mut args: impl Iterator<Item = OsString>) -> ExitCode {
    // It takes no flags and reads no config: everything is in the request.
    // SAFETY: isatty(3) takes a descriptor number and touches no memory.
    if args.next().is_some() || unsafe { libc::isatty(0) } == 1 || !is_socket(CHANNEL_FD) {
        eprintln!(
            "brnr host is started by brnr session new and brnr acp, not by hand \
             (brnr session new --foreground runs a session in a terminal)"
        );
        return ExitCode::from(2);
    }
    death::install();
    // A socket on fd 4 is kept until the request says whose process this
    // is: the signal link, if it is an editor's.
    let signal_link = is_socket(SIGNAL_LINK_FD);
    close_inherited_fds(if signal_link { &[CHANNEL_FD, SIGNAL_LINK_FD] } else { &[CHANNEL_FD] });
    set_cloexec(CHANNEL_FD);
    // SAFETY: is_socket checked CHANNEL_FD is an open socket,
    // close_inherited_fds kept it, and nothing else in the process holds it:
    // the UnixStream takes it over.
    let channel = unsafe { UnixStream::from_raw_fd(CHANNEL_FD) };
    let signal_link = signal_link.then(|| {
        set_cloexec(SIGNAL_LINK_FD);
        // SAFETY: as for CHANNEL_FD: an open socket (is_socket), kept, and
        // held by nothing else.
        unsafe { UnixStream::from_raw_fd(SIGNAL_LINK_FD) }
    });
    let request = match read_request() {
        Ok(request) => request,
        Err((msg, editor)) => {
            let channel = editor.map(|editor| (channel, editor));
            Failure { channel }.report(&msg, 2);
            return ExitCode::from(2);
        }
    };
    if let Some(h) = request.headless().filter(|h| h.foreground.is_some()) {
        // In a process group of its own, writing to the terminal.
        signals::write_from_background();
        death::foreground(if h.resume.is_some() { "session resume" } else { "session new" });
    }
    let editor = matches!(request.role, Role::Editor(_));
    // An editor's process doesn't start without its signal link, as it
    // doesn't without the link; a headless one's fd 4 isn't brnr's, and is
    // closed.
    let signal_link = match signal_link {
        None if editor => {
            Failure { channel: Some((channel, true)) }.report("no signal link on fd 4", 2);
            return ExitCode::from(2);
        }
        link => link.filter(|_| editor),
    };
    let mut failure = Failure { channel: channel.try_clone().ok().map(|c| (c, editor)) };
    match panic::catch_unwind(AssertUnwindSafe(|| Host::start(request, channel, signal_link))) {
        Ok(Ok(host)) => host.run(),
        Ok(Err((msg, code))) => {
            failure.report(&msg, code);
            ExitCode::from(code)
        }
        // A panic before the agent was spawned (later ones are the host's
        // to record, see `died_starting`): there is no log yet, and nobody
        // knows of the process but whoever started it (ADR 11).
        Err(_) => {
            let id = std::process::id();
            for file in [format!("{id}.sock"), format!("{id}.json")] {
                let _ = fs::remove_file(paths::runtime_dir().join(file));
            }
            failure.report(death::panicked().unwrap_or("brnr panicked"), 101);
            ExitCode::from(101)
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
    // SAFETY: stat is plain data, for which all zeros is a valid value.
    let mut st: libc::stat = unsafe { zeroed() };
    // SAFETY: st is a valid stat for fstat(2) to fill; any fd is a valid
    // argument (a bad one is EBADF).
    unsafe { libc::fstat(fd, &mut st) == 0 && st.st_mode & libc::S_IFMT == libc::S_IFSOCK }
}

/// Where a startup failure is reported, besides stderr: fd 3, as a frame
/// for the proxy on an editor's link (`true`), or as a line for brnr session
/// new on the start channel.
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

/// What `Host::set_up` makes the rest of the host from: the agent spawned,
/// the control socket bound, fd 3 and fd 4, and what the request says.
struct Parts {
    child: Child,
    listener: UnixListener,
    /// The editor link, or the start channel.
    channel: UnixStream,
    signal_link: Option<UnixStream>,
    /// The event loop's channel, for the threads to send on.
    tx: SyncSender<Ev>,
    /// The request, for the `started` record.
    recorded: Value,
    profile: Option<String>,
    agent: Vec<String>,
    proxy_pid: Option<u32>,
    started: SystemTime,
    /// What the start channel is subscribed to once the start commits.
    events: Vec<String>,
    bridges: Vec<Bridge>,
}

enum Ev {
    /// A frame from the proxy on the link; `None` once the link is gone.
    Link(Option<(u8, Vec<u8>)>),
    /// A frame from the proxy on the signal link, which nothing holds back;
    /// `None` once it is gone.
    SignalLink(Option<(u8, Vec<u8>)>),
    /// One line of agent stdout, with its `\n` unless it was the last bytes.
    AgentLine(Vec<u8>),
    /// A piece of a line of agent stdout past `LINE_BYTES`, as it came;
    /// `end` if the line ends with it (ADR 51).
    AgentPiece {
        bytes: Vec<u8>,
        end: bool,
    },
    AgentStdoutEof,
    /// Agent stderr, as one read returned it: up to `PIECE` bytes, not
    /// necessarily a line (ADR 62).
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
    /// brnr session new closed its end of the start channel.
    StartGone {
        peer: u64,
    },
    /// A thread panicked (see death.rs).
    Panicked,
}

enum StopStage {
    Term,
    Kill,
}

struct Host {
    /// For `brnr session new --foreground`, in a terminal: it says on stderr
    /// how things go, and exits with the agent's status.
    foreground: bool,
    /// In the foreground, a failed start has been reported on stderr.
    startup_reported: bool,
    /// The start is over: it has committed (the session is open and set up,
    /// and brnr session new has been told, see `finish_start`), or an editor
    /// attached. Until then, the process stopping is the start failing.
    start_done: bool,
    info: Value,
    host_id: String,
    /// How long an unanswered permission request waits before it is rejected.
    permission_timeout: Option<Duration>,
    agent_pid: pid_t,
    /// Bytes for the agent's stdin. Written on their own thread, so an agent
    /// that stops reading can't stall the host; dropping this closes the
    /// agent's stdin once what's queued is written.
    agent_in: Option<Sender<Vec<u8>>>,
    /// The editor's input on its way to the agent's stdin, and the agent's
    /// output on its way to the editor: past a cap, the link or the agent's
    /// stdout and stderr aren't read (see flow.rs).
    to_agent: Backlog,
    from_agent: Backlog,
    /// The agent's last lines on stderr (ADR 10).
    stderr_tail: VecDeque<String>,
    /// Dropping this makes the stdout reader close the agent's stdout.
    stop_stdout: Option<PipeWriter>,
    /// An editor is attached: from the start, until the link is gone (see
    /// `link_gone`). Not `link.is_some()`: the writer can give up first.
    editor: bool,
    /// Frames for the link writer while an editor is attached. Writes happen
    /// on their own thread so an editor that stops reading can't stall the
    /// host: signals, the control socket and bridges keep working.
    link: Option<Sender<(u8, Vec<u8>)>>,
    link_writer: Option<JoinHandle<()>>,
    /// brnr session new's start channel, until the start commits or fails.
    start_channel: Option<StartChannel>,
    /// When the start fails if it hasn't committed: the request's start
    /// timeout.
    start_deadline: Option<Instant>,
    log: Logger,
    sink: Sink,
    /// Detached: the thread taking what the process writes on stderr to the
    /// host log (see death.rs).
    own_stderr: Option<JoinHandle<()>>,
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
    /// Every unanswered request to the agent, by its id there, which is the
    /// host's whoever asked (ADR 61): who asked, and the session it is about.
    client_requests: HashMap<String, ClientRequest>,
    /// Requests from the agent to its client that are unanswered.
    agent_requests: Vec<AgentRequest>,
    /// Agent requests the host answered itself while the editor may answer
    /// them too (a cancel, an allow): its late answers are dropped (see
    /// experimental.rs).
    answered: HashMap<String, experimental::Answered>,
    /// Sessions brnr closed under the editor (ADR 4) → the process that took
    /// each over, if one did: the editor's requests for them are answered
    /// here.
    closed: HashMap<String, Option<u32>>,
    /// Request id of every unanswered prompt → its session.
    prompt_session: HashMap<String, String>,
    /// The last of the host's ids, for requests to the agent and its echoes.
    next_id: u64,
    next_permission: u64,
    /// Bytes from the editor after the last complete line.
    editor_buf: Vec<u8>,
    /// A line past `LINE_BYTES` is going through unread, from the editor or
    /// from the agent, until its newline (ADR 51).
    editor_long: bool,
    agent_long: bool,
    /// The line of the agent's stderr still coming, for the host log and
    /// the tail (ADR 10): what came of it is sent on already (ADR 62).
    stderr_line: Vec<u8>,
    /// The last piece of the agent's stderr the log has didn't end its line.
    stderr_mid_line: bool,
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
    /// Headless start: brnr has a transcript of the session resumed, so a
    /// load's replay isn't recorded again (ADR 57).
    transcript: bool,
    /// Headless start: what its flags and its profile set, resolved once
    /// the session is open and the agent has said which option is which.
    settings: (Settings, Settings),
    /// MCP servers for the sessions the host opens, as ACP has them.
    mcp_servers: Vec<Value>,
    /// Close a session idle this long (see `fire_idle_timers`).
    stop_when_idle: Option<Duration>,
    /// In the foreground: show the session's events on stdout, as text or
    /// (`--json`) JSON lines, and brnr's messages on stderr.
    show_events: bool,
    json_events: bool,
    display: Option<Display>,
    /// What the agent said it can do in `initialize`, and the login methods
    /// it offers.
    caps: Capabilities,
    auth_methods: Vec<AuthMethod>,
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
    experimental: BTreeSet<Experimental>,
    features: BTreeSet<Feature>,

    // Bridges; see control.rs.
    peers: HashMap<u64, Peer>,
    bridge_pids: Vec<pid_t>,

    status: Option<c_int>,
    /// The agent's stdout hasn't ended: once it has, so has the editor's
    /// (`agent_stdout_ended`).
    stdout_open: bool,
    stderr_open: bool,
    drain_until: Option<Instant>,
    /// A stop was asked for (brnr process stop, a signal, a failed start).
    stop_requested: bool,
    /// The next escalation of a stop.
    stopping: Option<(Instant, StopStage)>,
}

impl Host {
    /// `channel` is fd 3; `signal_link`, fd 4, is an editor's process's.
    fn start(
        req: Request,
        channel: UnixStream,
        signal_link: Option<UnixStream>,
    ) -> Result<Host, (String, u8)> {
        let recorded = req.recorded();
        let Request { profile, agent, cwd, strict, log: logging, bridges, mut role } = req;
        let (proxy_pid, mask, events) = match &mut role {
            Role::Editor(e) => (Some(e.proxy_pid), e.sigmask.clone(), Vec::new()),
            Role::Headless(h) => (None, Vec::new(), take(&mut h.events)),
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
        // std resets the mask in the child; give the agent the editor's.
        unsafe {
            // SAFETY: the closure runs in the child between fork and exec,
            // where only async-signal-safe calls may be made: set_mask is
            // pthread_sigmask(3) on a sigset built on the stack from mask,
            // which was moved in and isn't reallocated, and restore_for_child
            // is an atomic load and signal(2).
            cmd.pre_exec(move || {
                signals::set_mask(&mask);
                signals::restore_for_child();
                Ok(())
            })
        };
        let child = cmd.spawn().map_err(|err| {
            cleanup();
            // Same codes a shell uses for "not found" / "not executable".
            let code = if err.kind() == ErrorKind::PermissionDenied { 126 } else { 127 };
            (format!("{}: {err}", program[0].to_string_lossy()), code)
        })?;
        let agent_pid = child.id() as pid_t;

        // From here there is an agent, and a host to hold it and what the
        // rest of the start makes, as it is made (see `set_up`): whatever
        // ends the start early, an error or a panic, finds them there, kills
        // the agent with them (P14) and records how it ended (ADR 11).
        let (tx, rx) = mpsc::sync_channel(EVENTS_QUEUED);
        let new = Host::new(role, cwd, strict, logging, agent_pid, rx);
        let mut host = Host { host_id, sock_path, meta_path, ..new };
        let parts = Parts {
            child,
            listener,
            channel,
            signal_link,
            tx,
            recorded,
            profile,
            agent,
            proxy_pid,
            started,
            events,
            bridges,
        };
        match panic::catch_unwind(AssertUnwindSafe(|| host.set_up(parts))) {
            Ok(Ok(())) => Ok(host),
            Ok(Err((error, code))) => Err(host.abandon(error, code)),
            Err(_) => Err(host.died_starting(death::panicked().unwrap_or("brnr panicked"))),
        }
    }

    /// The host of `role`'s agent, `agent_pid`, before anything is set up
    /// (see `set_up`): no log, no threads, no peers, no sessions. Its ids
    /// and files are `start`'s to fill in.
    fn new(
        role: Role,
        cwd: PathBuf,
        strict: bool,
        logging: Log,
        agent_pid: pid_t,
        rx: Receiver<Ev>,
    ) -> Host {
        let (editor, headless) = match role {
            Role::Editor(editor) => (Some(editor), None),
            Role::Headless(headless) => (None, Some(headless)),
        };
        let foreground = headless.as_ref().is_some_and(|h| h.foreground.is_some());
        // An editor's process has no start deadline, and none of the rest.
        let start_deadline = (headless.as_ref())
            .and_then(|h| Instant::now().checked_add(Duration::from_secs(h.start_timeout)));
        let h = headless.unwrap_or_default();
        let shown = h.foreground.as_ref();
        let show_events = shown.is_some_and(|f| !f.quiet);
        let json_events = shown.is_some_and(|f| f.json);
        let (experimental, features) = match &editor {
            Some(e) => (e.experimental.clone(), e.features.clone()),
            None => (BTreeSet::new(), BTreeSet::new()),
        };
        // No log until `set_up` starts it.
        let log = Logger::disabled();
        Host {
            foreground,
            startup_reported: false,
            start_done: false,
            info: Value::Null,
            host_id: String::new(),
            permission_timeout: h.permission_timeout.map(Duration::from_secs),
            agent_pid,
            agent_in: None,
            to_agent: Backlog::default(),
            from_agent: Backlog::default(),
            stderr_tail: VecDeque::new(),
            stop_stdout: None,
            editor: editor.is_some(),
            link: None,
            link_writer: None,
            start_channel: None,
            start_deadline,
            sink: log.sink(),
            log,
            own_stderr: None,
            rx,
            sock_path: PathBuf::new(),
            meta_path: PathBuf::new(),
            cwd,
            sessions: Vec::new(),
            pending: HashMap::new(),
            claimed: HashMap::new(),
            client_requests: HashMap::new(),
            agent_requests: Vec::new(),
            answered: HashMap::new(),
            closed: HashMap::new(),
            prompt_session: HashMap::new(),
            next_id: 0,
            next_permission: 0,
            editor_buf: Vec::new(),
            editor_long: false,
            agent_long: false,
            stderr_line: Vec::new(),
            stderr_mid_line: false,
            deepest: 0,
            stack: json::SHALLOW,
            prompt: h.prompt,
            auth: h.auth,
            resume: h.resume,
            transcript: h.transcript,
            settings: (h.settings, h.defaults),
            mcp_servers: h.mcp_servers,
            stop_when_idle: h.stop_when_idle.map(Duration::from_secs),
            show_events,
            json_events,
            display: None,
            caps: Capabilities::default(),
            auth_methods: Vec::new(),
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
        }
    }

    /// The rest of the start, once the agent is spawned: the log and the
    /// metadata, the threads on the agent's pipes, the editor's links or the
    /// start channel, the control socket, signals, the display, bridges;
    /// then the editor is told the process is ready, or the headless start
    /// begins. Each is the host's as soon as it is made, for `abandon` or
    /// `died_starting` to undo if the start ends here.
    fn set_up(&mut self, parts: Parts) -> Result<(), (String, u8)> {
        let Parts {
            mut child,
            listener,
            channel,
            signal_link,
            tx,
            recorded,
            profile,
            agent,
            proxy_pid,
            started,
            events,
            bridges,
        } = parts;
        let id = std::process::id().to_string();
        if self.logging != Log::Off {
            let ids = Ids {
                host_id: self.host_id.clone(),
                host_pid: std::process::id(),
                agent_pid: self.agent_pid as u32,
            };
            self.log = Logger::start(ids, proxy_pid, self.logging == Log::All)
                .map_err(|e| (format!("log: {e}"), 1))?;
            self.sink = self.log.sink();
        }
        self.info = json!({
            "id": id,
            "host_id": self.host_id,
            "profile": profile,
            "host_pid": std::process::id(),
            "proxy_pid": proxy_pid,
            "agent_pid": self.agent_pid,
            "agent": agent,
            "cwd": self.cwd.to_string_lossy(),
            "host_log": self.log.host_log().map(|p| p.to_string_lossy().into_owned()),
            "socket": self.sock_path.to_string_lossy(),
            "started": log::rfc3339(started),
        });
        write_atomic(&self.meta_path, format!("{:#}\n", self.info).as_bytes());
        // Every process records what it was asked to do (ADR 8).
        let info = self.info.clone();
        self.sink.note(None, json!({ "event": "started", "info": info, "request": recorded }));
        // A detached process's stderr is /dev/null until here (ADR 11).
        if !self.foreground && self.log.host_log().is_some() {
            self.own_stderr = death::capture_stderr(self.sink.clone());
        }

        death::wake(tx.clone());
        let (agent_in, agent_queue) = mpsc::channel();
        let stdin = child.stdin.take().unwrap();
        let b = self.to_agent.clone();
        thread::spawn(move || write_agent_stdin(stdin, agent_queue, b));
        self.agent_in = Some(agent_in);
        let (stop_rx, stop_tx) = io::pipe().expect("pipe");
        self.stop_stdout = Some(stop_tx);
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        let (t, b) = (tx.clone(), self.from_agent.clone());
        thread::spawn(move || read_agent_stdout(stdout, stop_rx, t, b));
        let (t, b, shown) = (tx.clone(), self.from_agent.clone(), self.foreground);
        thread::spawn(move || read_agent_stderr(stderr, t, b, shown));
        let (t, agent_pid) = (tx.clone(), self.agent_pid);
        thread::spawn(move || {
            if wait_exited(agent_pid).is_ok() {
                let _ = t.send(Ev::AgentExited);
            }
        });
        let (link, start_channel) =
            if self.editor { (Some(channel), None) } else { (None, Some(channel)) };
        if let Some(link) = link {
            let reader = link.try_clone().expect("clone link");
            let (t, b) = (tx.clone(), self.to_agent.clone());
            thread::spawn(move || read_link(reader, t, b));
            let (frames, rx) = mpsc::channel();
            let b = self.from_agent.clone();
            self.link_writer = Some(thread::spawn(move || write_link(link, rx, b)));
            self.link = Some(frames);
        }
        if let Some(signal_link) = signal_link {
            let t = tx.clone();
            thread::spawn(move || read_signal_link(signal_link, t));
        }
        let t = tx.clone();
        thread::spawn(move || control::serve(listener, t));
        let signals = signals::install();
        let t = tx.clone();
        thread::spawn(move || read_signals(signals, t));
        if self.foreground {
            self.display = Some(Display::start(self.sink.clone(), self.json_events));
        }

        // brnr session new hears how the start ends from here.
        if let Some(channel) = start_channel {
            self.open_start_channel(channel, events, &tx)
                .map_err(|err| (format!("start channel: {err}"), 1))?;
        }
        for (n, bridge) in bridges.iter().enumerate() {
            self.start_bridge(n, bridge, &tx).map_err(|err| (err, 2))?;
        }
        death::test_panic_starting();
        if self.editor {
            self.start_done = true;
            let ready =
                json!({ "id": id, "host_pid": std::process::id(), "agent_pid": self.agent_pid });
            self.send_link(frame::READY, ready.to_string().as_bytes());
        } else {
            self.begin_headless_start();
        }
        if self.foreground {
            self.say(format!(
                "brnr: process {id}: started {} in {}; Ctrl-C to stop",
                agent.join(" "),
                self.cwd.display()
            ));
        }
        Ok(())
    }

    /// A start that fails before the event loop runs: what it started goes,
    /// and its host log records the end, as `finish` does, so a log without
    /// `exited` is a process that died (ADR 11). Returns `error` and `code`.
    fn abandon(mut self, error: String, code: u8) -> (String, u8) {
        self.kill_all();
        let _ = fs::remove_file(&self.sock_path);
        let _ = fs::remove_file(&self.meta_path);
        self.sink.note(None, json!({ "event": "exited", "status": null, "reason": error }));
        self.release_stderr();
        std::mem::replace(&mut self.log, Logger::disabled()).finish();
        (error, code)
    }

    /// A panic as the start is under way, before the event loop (ADR 11):
    /// recorded as `died` records one, as far as what the start has made
    /// allows (no sessions yet; the log, the display and bridges once they
    /// are there), and the agent killed with its process group (P14).
    /// brnr session new or the editor is told by `main`, on fd 3, as of any
    /// start that fails before the event loop (see `Failure`), so nothing else
    /// is to write there: the start channel is let go of, and what is
    /// queued for the editor (`READY`) goes first. Returns the error and
    /// the exit code.
    fn died_starting(mut self, reason: &str) -> (String, u8) {
        self.record_panic(reason);
        self.release_start_channel();
        self.link = None;
        if let Some(writer) = self.link_writer.take() {
            let until = Instant::now() + PEER_FLUSH;
            while !writer.is_finished() && Instant::now() < until {
                thread::sleep(Duration::from_millis(10));
            }
        }
        self.wind_up(reason);
        (reason.to_owned(), 101)
    }

    fn run(mut self) -> ExitCode {
        // A panic on the event loop ends it here; on another thread, at the
        // next turn of the loop (see death.rs).
        let looped = panic::catch_unwind(AssertUnwindSafe(|| self.event_loop()));
        let died = death::panicked().or(looped.err().map(|_| "brnr panicked"));
        // One in `deep` leaves the stack it took.
        self.stack = json::SHALLOW;
        // Finishing drops what the host holds: on a stack that takes it too.
        let deepest = self.deepest;
        let mut host = Some(self);
        json::on_stack(deepest, || host.take().unwrap().finish(died))
            .unwrap_or_else(|| host.take().unwrap().finish(died))
    }

    fn event_loop(&mut self) {
        loop {
            if self.status.is_some() && !self.stdout_open && !self.stderr_open {
                break;
            }
            if death::panicked().is_some() {
                break;
            }
            let now = Instant::now();
            if self.drain_until.is_some_and(|t| now >= t) {
                // The agent's last output, held back for the editor, is
                // still to come, at the editor's pace (ADR 6).
                if !self.from_agent.full() {
                    break;
                }
                self.drain_until = Some(now + DRAIN);
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

    fn finish(mut self, died: Option<&str>) -> ExitCode {
        // The agent's last bytes on stderr, sent on already, if they ended no
        // line: in the host log and the tail too, before `exited`.
        self.stderr_piece();
        if let Some(reason) = died {
            return self.died(reason);
        }
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
        // Held messages and context that never became a prompt.
        for i in 0..self.sessions.len() {
            self.drop_all(i, "exit");
        }
        self.emit_to_sessions(json!({ "event": "exited", "status": status }));
        self.wait_logged();
        // Let go of the sessions as brnr stops listing the process (ADR 3).
        self.claimed.clear();
        for s in &mut self.sessions {
            if let Hold::Owner(lock) = &mut s.hold {
                lock.take();
            }
        }
        let _ = fs::remove_file(&self.sock_path);
        let _ = fs::remove_file(&self.meta_path);
        self.let_peers_go();
        if self.foreground {
            self.say(format!("brnr: agent exited: {status}"));
        }
        self.close_display();
        // Let the writer deliver what's queued, at the editor's pace (ADR 6);
        // a proxy that has gone fails the write.
        self.link = None;
        if let Some(writer) = self.link_writer.take() {
            self.wait_out(|| writer.is_finished());
        }
        self.release_stderr();
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

    /// A panic (ADR 11): told wherever it still can be, and the agent goes,
    /// as it does when its owner goes (P14).
    fn died(mut self, reason: &str) -> ExitCode {
        self.record_panic(reason);
        if !self.start_done {
            self.startup_failed(reason);
        }
        self.wind_up(reason);
        ExitCode::from(101)
    }

    /// A panic, in the host log, and the agent killed with its process
    /// group.
    fn record_panic(&mut self, reason: &str) {
        self.sink.note(None, json!({ "event": "panic", "error": reason }));
        if self.status.is_none() && self.foreground {
            self.say("brnr: killing the agent".into());
        }
        self.kill_group(libc::SIGKILL);
    }

    /// The rest of a death, once the agent is killed and whoever waits for
    /// the start told: `exited`, with the reason, in the host log, every
    /// session's file and to the peers still reached; the `.sock` and
    /// `.json` files removed; bridges given the time to act on `exited`
    /// they get on any exit; the display, stderr and the log let go of.
    fn wind_up(&mut self, reason: &str) {
        // Held messages that never became a prompt, as on a normal exit
        // (ADR 20); a second panic here mustn't cost the `exited` below.
        let held = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            for i in 0..self.sessions.len() {
                self.drop_all(i, "exit");
            }
        }));
        if held.is_err() {
            self.sink.note(None, json!({ "event": "drop-held-failed" }));
        }
        let status = describe_status(self.status);
        self.emit_to_sessions(json!({ "event": "exited", "status": status, "reason": reason }));
        self.wait_logged();
        let _ = fs::remove_file(&self.sock_path);
        let _ = fs::remove_file(&self.meta_path);
        self.let_peers_go();
        self.close_display();
        self.release_stderr();
        std::mem::replace(&mut self.log, Logger::disabled()).finish();
    }

    /// Exiting, before brnr stops listing the process and its sessions are
    /// let go of: waits up to LOG_FLUSH for its transcript to have what was
    /// recorded, `exited` last, so that `event log`, `session list` and
    /// `session resume` of a session that isn't running read it whole (ADR
    /// 48). A stalled disk holds the sessions no longer.
    fn wait_logged(&self) {
        let (tx, rx) = mpsc::channel();
        self.sink.when_written(move || {
            let _ = tx.send(());
        });
        let _ = rx.recv_timeout(LOG_FLUSH);
    }

    /// Exiting: the last events (`exited`) reach the peers, within
    /// PEER_FLUSH; then started bridges, their stdin closed, should exit,
    /// and those that haven't by BRIDGE_EXIT get SIGTERM. Bridges also see
    /// EOF on their stdin once we exit.
    fn let_peers_go(&mut self) {
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
            sys::kill(pid, libc::SIGTERM);
        }
    }

    /// One of brnr's own messages, on the foreground's stderr.
    pub(super) fn say(&self, text: String) {
        if let Some(display) = &self.display {
            display.note(text);
        }
    }

    /// The display ends once its reader has taken what is queued.
    fn close_display(&mut self) {
        if let Some(display) = self.display.take() {
            display.finish();
            self.wait_out(|| display.done());
        }
    }

    /// Exiting, waits until `done`: until a reader has taken what is queued
    /// for it, however long that takes, as a pipe's writer would. A signal
    /// to the process (Ctrl-C, `kill`) gives up on it.
    fn wait_out(&self, done: impl Fn() -> bool) {
        let tick = Duration::from_millis(10);
        while !done() {
            match self.rx.recv_timeout(tick) {
                Ok(Ev::Signal(_)) => return,
                Err(RecvTimeoutError::Disconnected) => thread::sleep(tick),
                Ok(_) | Err(RecvTimeoutError::Timeout) => {}
            }
        }
    }

    /// What the process wrote on stderr is in the host log before it closes.
    fn release_stderr(&mut self) {
        if let Some(reader) = self.own_stderr.take() {
            death::release_stderr(reader, PEER_FLUSH);
        }
    }

    fn handle(&mut self, ev: Ev) {
        match ev {
            Ev::Link(Some((kind, payload))) => match kind {
                frame::DATA => {
                    self.editor_bytes(&payload);
                    self.to_agent.done(payload.len());
                }
                frame::EOF => self.editor_eof(),
                _ => {}
            },
            Ev::SignalLink(Some((kind, payload))) => match kind {
                frame::SIGNAL if payload.len() == 4 => {
                    self.signal(i32::from_be_bytes(payload.try_into().unwrap()));
                }
                frame::STDOUT_CLOSED => self.editor_stopped_reading(),
                _ => {}
            },
            // The proxy has gone: whichever link says so first.
            Ev::Link(None) | Ev::SignalLink(None) => self.link_gone(),
            Ev::AgentLine(line) => {
                self.agent_line(&line);
                self.from_agent.done(line.len());
            }
            Ev::AgentPiece { bytes, end } => {
                let n = bytes.len();
                self.agent_piece(bytes, end);
                self.from_agent.done(n);
            }
            Ev::AgentStdoutEof => self.agent_stdout_ended(),
            Ev::AgentStderr(bytes) => {
                // On at once, then into lines for the log: nothing that keeps
                // stderr as lines holds it up (ADR 62).
                self.send_link(frame::STDERR, &bytes);
                self.from_agent.done(bytes.len());
                self.stderr_lines(&bytes);
            }
            Ev::AgentStderrEof => {
                self.stderr_piece();
                self.stderr_open = false;
            }
            Ev::AgentExited => {
                // Whatever the agent left running goes too, including after
                // a spontaneous exit or a crash during setup (ADR 11, P14).
                // Before reaping, its pid (and group id) can't be reused.
                self.kill_group(libc::SIGKILL);
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
            Ev::PeerRequest { peer, req } => {
                death::test_panic(&req);
                self.peer_request(peer, req);
            }
            Ev::PeerClosed { peer } => {
                self.peers.remove(&peer);
            }
            Ev::BridgeStderr { label, line } => {
                self.sink
                    .note(None, json!({ "event": "bridge-stderr", "bridge": label, "text": line }));
            }
            Ev::Signal(sig) => self.host_signal(sig),
            Ev::StartGone { peer } => self.start_gone(peer),
            Ev::Panicked => {} // See `event_loop`.
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

    /// The agent's stderr, sent on already, as lines for the host log and
    /// the tail: each line is recorded once it ends, a longer one than
    /// `PIECE` in pieces of that (ADR 51).
    fn stderr_lines(&mut self, mut bytes: &[u8]) {
        while !bytes.is_empty() {
            let room = PIECE - self.stderr_line.len();
            let (n, end) = match bytes[..bytes.len().min(room)].iter().position(|&b| b == b'\n') {
                Some(i) => (i + 1, true),
                None => (bytes.len().min(room), false),
            };
            self.stderr_line.extend_from_slice(&bytes[..n]);
            bytes = &bytes[n..];
            if end || self.stderr_line.len() == PIECE {
                self.stderr_piece();
            }
        }
    }

    /// The line of stderr so far, whole or not, into the host log and the
    /// tail.
    fn stderr_piece(&mut self) {
        let piece = take(&mut self.stderr_line);
        if piece.is_empty() {
            return;
        }
        self.sink.msg(None, Dir::AgentStderr, &piece);
        // The rest of a long line, whose start is kept already (ADR 51).
        let rest = std::mem::replace(&mut self.stderr_mid_line, !piece.ends_with(b"\n"));
        if self.start_done || rest {
            return;
        }
        if self.stderr_tail.len() == STDERR_TAIL {
            self.stderr_tail.pop_front();
        }
        self.stderr_tail.push_back(tail_line(&piece));
    }

    /// `error`, ending with the agent's last lines on stderr if it wrote any,
    /// the one it is in the middle of too: a prompt that waits for an answer
    /// ends none ("login required: ").
    pub(super) fn with_stderr(&self, error: &str) -> String {
        let partial = (!self.stderr_mid_line && !self.stderr_line.is_empty())
            .then(|| tail_line(&self.stderr_line));
        let skip = usize::from(partial.is_some() && self.stderr_tail.len() == STDERR_TAIL);
        let lines: String = (self.stderr_tail.iter().skip(skip))
            .chain(&partial)
            .map(|l| format!("\n  {l}"))
            .collect();
        if lines.is_empty() {
            return error.to_owned();
        }
        format!("{error}. The agent's last lines on stderr:{lines}")
    }

    // ---- the editor's side of the link --------------------------------

    fn editor_attached(&self) -> bool {
        self.editor
    }

    /// A signal the proxy caught reaches the agent as that signal.
    fn signal(&mut self, sig: c_int) {
        if self.status.is_none() {
            self.sink.note(None, json!({ "event": "signal", "signal": sig }));
            sys::kill(self.agent_pid, sig);
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

    /// The agent's stdout has ended: the agent closed it or exited, or the
    /// editor stopped reading it (`editor_stopped_reading`). So does the
    /// editor's, after what came before, while the agent may run on and
    /// write on stderr, as with the agent run directly (P1, ADR 62). brnr
    /// writes nothing more there of its own either (`write_editor`).
    fn agent_stdout_ended(&mut self) {
        self.stdout_open = false;
        self.sink.note(None, json!({ "event": "agent-stdout-ended" }));
        self.send_link(frame::EOF, &[]);
    }

    fn editor_stopped_reading(&mut self) {
        self.sink.note(None, json!({ "event": "editor-stopped-reading" }));
        // The agent gets EPIPE, just as it would directly.
        self.stop_stdout = None;
    }

    /// The editor went away (the proxy did, which either of its links says):
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
        if self.link.is_some() {
            self.send_link_owned(kind, payload.to_vec());
        }
    }

    /// As `send_link`, with no copy made of `payload`: a piece of a long
    /// line can be 32 MiB (ADR 51).
    fn send_link_owned(&mut self, kind: u8, payload: Vec<u8>) {
        let Some(link) = &self.link else { return };
        self.from_agent.add(payload.len());
        if link.send((kind, payload)).is_err() {
            // The writer failed and shut the link down: the proxy has gone,
            // which the reader thread or the signal link reports
            // (`link_gone`).
            self.link = None;
        }
    }

    fn write_agent(&mut self, bytes: &[u8]) {
        let Some(agent) = &self.agent_in else { return };
        self.to_agent.add(bytes.len());
        if agent.send(bytes.to_vec()).is_err() {
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
            libc::SIGUSR1 | libc::SIGUSR2 => {
                sys::kill(self.agent_pid, sig);
            }
            _ if self.stop_requested => {
                if self.foreground {
                    self.say("brnr: killing the agent".into());
                }
                self.kill_group(libc::SIGKILL);
            }
            _ => {
                if self.foreground {
                    self.say("brnr: stopping (again to kill)".into());
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
    /// than carry on where nobody knows about it (brnr session new gives up
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
        let group = sys::kill(-self.agent_pid, sig);
        // SAFETY: getpgid(2) takes a pid and touches no memory of ours.
        if !group || unsafe { libc::getpgid(self.agent_pid) } != self.agent_pid {
            sys::kill(self.agent_pid, sig);
        }
    }

    /// Startup failed after the agent was spawned.
    fn kill_all(&mut self) {
        self.kill_group(libc::SIGKILL);
        for &pid in &self.bridge_pids {
            sys::kill(pid, libc::SIGTERM);
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

/// Writes queued frames to the proxy, as fast as it reads them, until the
/// queue closes or a write fails: the proxy has gone. Then the link is shut
/// down, so the reader thread sees it gone too.
fn write_link(mut link: UnixStream, frames: Receiver<(u8, Vec<u8>)>, from_agent: Backlog) {
    for (kind, payload) in frames {
        if frame::write(&mut link, kind, &payload).is_err() {
            let _ = link.shutdown(std::net::Shutdown::Both);
            break;
        }
        from_agent.done(payload.len());
    }
    from_agent.close();
}

/// Writes queued bytes to the agent's stdin, as fast as it reads them, until
/// the queue closes or the agent closes its end. Dropping `stdin` on the way
/// out closes it.
fn write_agent_stdin(mut stdin: ChildStdin, queue: Receiver<Vec<u8>>, to_agent: Backlog) {
    for bytes in queue {
        if stdin.write_all(&bytes).is_err() {
            break;
        }
        to_agent.done(bytes.len());
    }
    to_agent.close();
}

fn read_signals(mut signals: PipeReader, tx: SyncSender<Ev>) {
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

/// Frames from the proxy until the link is gone. While the agent's stdin is
/// a cap behind, the link isn't read: what the editor writes waits in it,
/// and the proxy, then the editor, block writing; stdin's EOF waits behind
/// it, as it would on a pipe. Signals don't, nor does the proxy going away:
/// they come on the signal link (see `read_signal_link`).
fn read_link(link: UnixStream, tx: SyncSender<Ev>, to_agent: Backlog) {
    let mut reader = BufReader::new(link);
    loop {
        to_agent.room(None);
        match frame::read(&mut reader) {
            Ok(Some(frame)) => {
                if frame.0 == frame::DATA {
                    to_agent.add(frame.1.len());
                }
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

/// Frames from the proxy on the signal link until it is gone. Never held
/// back: a signal reaches the agent at once, and the proxy going away is
/// heard, however far behind the agent is reading its stdin.
fn read_signal_link(link: UnixStream, tx: SyncSender<Ev>) {
    let mut reader = BufReader::new(link);
    loop {
        match frame::read(&mut reader) {
            Ok(Some(frame)) => {
                if tx.send(Ev::SignalLink(Some(frame))).is_err() {
                    return;
                }
            }
            Ok(None) | Err(_) => {
                let _ = tx.send(Ev::SignalLink(None));
                return;
            }
        }
    }
}

/// Splits the agent's stdout into lines until EOF, or until `stop` closes,
/// which drops our end so the agent gets EPIPE like it would directly. Past
/// the cap of what is on its way to the editor (or the event loop) it waits,
/// and the agent blocks writing, as it would to the editor directly. A line
/// past `LINE_BYTES` goes on in pieces as they come (ADR 51).
fn read_agent_stdout(
    mut out: ChildStdout,
    stop: PipeReader,
    tx: SyncSender<Ev>,
    from_agent: Backlog,
) {
    let mut fds = [
        libc::pollfd { fd: out.as_raw_fd(), events: libc::POLLIN, revents: 0 },
        libc::pollfd { fd: stop.as_raw_fd(), events: libc::POLLIN, revents: 0 },
    ];
    let mut buf = vec![0; 64 * 1024];
    let mut line = Vec::new();
    // Inside a line past LINE_BYTES.
    let mut long = false;
    loop {
        from_agent.room(None);
        // SAFETY: fds is an array of two valid pollfds, and the count says so.
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
        let mut read = &buf[..n];
        while !read.is_empty() {
            let end = read.iter().position(|&b| b == b'\n');
            let (now, rest) = read.split_at(end.map_or(read.len(), |i| i + 1));
            line.extend_from_slice(now);
            read = rest;
            let sent = if long || line.len() > LINE_BYTES {
                long = end.is_none();
                from_agent.add(line.len());
                tx.send(Ev::AgentPiece { bytes: take(&mut line), end: end.is_some() }).is_ok()
            } else if end.is_some() {
                from_agent.add(line.len());
                tx.send(Ev::AgentLine(take(&mut line))).is_ok()
            } else {
                continue;
            };
            if !sent {
                return;
            }
        }
    }
    from_agent.add(line.len());
    if long {
        let _ = tx.send(Ev::AgentPiece { bytes: line, end: true });
    } else if !line.is_empty() {
        let _ = tx.send(Ev::AgentLine(line));
    }
    let _ = tx.send(Ev::AgentStdoutEof);
}

/// The agent's stderr, as it comes: what each read returns goes on at once,
/// newline or not, held back as its stdout is (ADR 62). In the foreground
/// (`shown`) it also goes to stderr, unchanged (ADR 10): written from here,
/// the agent blocks on a terminal that does.
fn read_agent_stderr(mut err: ChildStderr, tx: SyncSender<Ev>, from_agent: Backlog, shown: bool) {
    let mut terminal = shown.then(|| sys::stdio(2));
    let mut buf = vec![0; PIECE];
    loop {
        from_agent.room(None);
        let n = match err.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(err) if err.kind() == ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        if let Some(out) = &mut terminal
            && crate::proxy::write_all(out, &buf[..n]).is_err()
        {
            terminal = None;
        }
        from_agent.add(n);
        if tx.send(Ev::AgentStderr(buf[..n].to_vec())).is_err() {
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
            // SAFETY: this runs first thing in the host, before anything opens
            // a descriptor or starts a thread: every fd above 2 is one it
            // inherited, and no File of its own holds one. That of /dev/fd,
            // listed too, is closed already (EBADF).
            unsafe { libc::close(fd) };
        }
    }
}

fn set_cloexec(fd: RawFd) {
    // SAFETY: fcntl(2) F_SETFD takes a descriptor number and a flag, and
    // touches no memory.
    unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
}

fn write_atomic(path: &std::path::Path, bytes: &[u8]) {
    let tmp = path.with_extension("json.tmp");
    let written = File::create(&tmp).and_then(|mut f| f.write_all(bytes));
    if written.is_ok() {
        let _ = fs::rename(&tmp, path);
    }
}

/// Blocks until `pid`, a child, has terminated, without reaping it: until
/// it is reaped, its pid (and process group id) can't be another's.
pub fn wait_exited(pid: pid_t) -> io::Result<()> {
    unsafe {
        // SAFETY: info is a siginfo_t (all zeros is a valid one) for waitid(2)
        // to fill.
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
    // SAFETY: status is a valid int for waitpid(2) to fill.
    while unsafe { libc::waitpid(pid, &mut status, 0) } < 0 {
        let err = io::Error::last_os_error();
        if err.kind() != ErrorKind::Interrupted {
            return Err(err);
        }
    }
    Ok(status)
}

/// Whether `pid` is a process of the user's, as every brnr process of
/// theirs is. One that is someone else's (`kill` says EPERM) isn't brnr's,
/// whatever pid a file of brnr's recorded. That a pid is alive doesn't say
/// it is still the process that recorded it: see `gone` in ctl.rs.
pub fn alive(pid: i64) -> bool {
    pid > 0 && sys::kill(pid as pid_t, 0)
}

/// A line of stderr as the tail keeps it: without its line ending, and
/// `STDERR_LINE` bytes of it at most.
fn tail_line(bytes: &[u8]) -> String {
    let line = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    String::from_utf8_lossy(&line[..line.len().min(STDERR_LINE)]).into_owned()
}

/// A JSON-RPC id as a map key: `1` and `"1"` stay distinct.
fn id_key(id: &Value) -> String {
    id.to_string()
}

fn text_block(text: &str) -> Value {
    json!({ "type": "text", "text": text })
}
