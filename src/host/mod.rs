//! `brnr host`: owns the agent process and its pipes for the agent's whole life.
//!
//! Started detached by `brnr proxy` for an editor (with `--link-fd`), detached
//! by `brnr start` for a session that is headless from the start (with
//! `--ready-fd`), or by hand, in the foreground, for a headless session (see
//! [`USAGE`]). It is the hub between three kinds of peer:
//!
//! - the agent, over its stdio;
//! - the ACP owner: the editor, through the proxy on the link, or the host
//!   itself when no editor is attached (see acp.rs);
//! - any number of bridges, speaking JSON lines rather than ACP: children
//!   started from the profile, and processes on the control socket such as
//!   brnr (see control.rs).
//!
//! When the editor goes away, the `on_disconnect` policy decides: `direct`
//! does what a directly spawned agent would have got (stdin closed, then
//! SIGKILL; a signal the proxy catches reaches the agent as that signal),
//! and `headless` keeps the agent running with the host as its client.

mod acp;
mod control;
mod requests;
mod state;

use std::collections::HashMap;
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

use crate::config;
use crate::frame;
use crate::log::{self, Dir, Ids, Logger, Sink};
use crate::paths;
use crate::signals;
use crate::spawn;

use acp::{AgentRequest, Pending, Session};
pub use control::check_bridge;
use control::{Closer, Peer};
use requests::{HostRequest, SetupStep};

/// The options for running it by hand. `brnr proxy` and `brnr start` also pass
/// --link-fd, --ready-fd, --proxy-pid, --on-disconnect and --sigmask.
const USAGE: &str = "usage: brnr host [--profile <name>] [--name <name>] [--cwd <dir>] \
[--prompt <text> | --prompt -] [--resume <session>] [--mode <mode>] [--set <option>=<value>]... \
[--permissions <policy>] [--stop-when-idle] [--quiet] [-- <agent> [args...]]

Runs a headless ACP session in the foreground: the host starts the agent,
opens a session in the current directory (or --cwd), or resumes one, and
sends --prompt if given. It shows the session as it goes (--quiet: not).
Talk to it with brnr (send, watch, approve, stop, ...). Ctrl-C stops it
gracefully; press it again to kill the agent. Exits with the agent's exit
status.";

/// How long to keep forwarding output after the agent exits, in case
/// something it started still holds its stdout open.
const DRAIN: Duration = Duration::from_millis(500);

/// How long the host, exiting, waits for peers to be sent what's queued for
/// them; one that stopped reading doesn't hold it up for longer.
const PEER_FLUSH: Duration = Duration::from_secs(2);

/// How long one write to the proxy may block before the host treats the
/// link as gone. Only the writer thread waits; the host carries on.
const LINK_WRITE_TIMEOUT: Duration = Duration::from_secs(30);

/// `brnr stop`: stdin is closed once what is queued for it is written, then
/// the agent's process group gets SIGTERM, then SIGKILL.
const STOP_TERM_AFTER: Duration = Duration::from_secs(5);
const STOP_KILL_AFTER: Duration = Duration::from_secs(5);

/// What the host does when the editor goes away without a handoff.
#[derive(Clone, Copy, PartialEq)]
pub enum Policy {
    /// Behave as if the editor had run the agent directly.
    Direct,
    /// Keep the agent running, with the host as its client.
    Headless,
}

impl Policy {
    pub fn parse(name: &str) -> Result<Policy, String> {
        match name {
            "direct" => Ok(Policy::Direct),
            "headless" => Ok(Policy::Headless),
            other => Err(format!("unknown on_disconnect policy: {other}")),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Policy::Direct => "direct",
            Policy::Headless => "headless",
        }
    }
}

/// How the host answers permission requests while no editor is attached.
#[derive(Clone, Copy)]
pub enum Permissions {
    /// Tell the bridges and wait for an approve or deny.
    Ask,
    AutoAllow,
    AutoDeny,
}

impl Permissions {
    pub fn parse(name: &str) -> Result<Permissions, String> {
        match name {
            "ask" => Ok(Permissions::Ask),
            "auto-allow" => Ok(Permissions::AutoAllow),
            "auto-deny" => Ok(Permissions::AutoDeny),
            other => Err(format!("unknown permissions policy: {other}")),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Permissions::Ask => "ask",
            Permissions::AutoAllow => "auto-allow",
            Permissions::AutoDeny => "auto-deny",
        }
    }
}

/// ACP's tool kinds, which permission rules can name.
pub const TOOL_KINDS: &[&str] = &[
    "read",
    "edit",
    "delete",
    "move",
    "search",
    "execute",
    "think",
    "fetch",
    "switch_mode",
    "other",
];

/// The permission policy for each tool kind, with a default for the rest.
#[derive(Clone)]
pub struct PermissionRules {
    pub default: Permissions,
    pub kinds: Vec<(String, Permissions)>,
}

impl PermissionRules {
    pub fn parse(spec: Option<&config::PermissionsSpec>) -> Result<PermissionRules, String> {
        let mut rules = PermissionRules { default: Permissions::Ask, kinds: Vec::new() };
        match spec {
            None => {}
            Some(config::PermissionsSpec::One(policy)) => {
                rules.default = Permissions::parse(policy)?
            }
            Some(config::PermissionsSpec::ByKind(map)) => {
                for (kind, policy) in map {
                    let policy = Permissions::parse(policy)?;
                    if kind == "default" {
                        rules.default = policy;
                    } else if TOOL_KINDS.contains(&kind.as_str()) {
                        rules.kinds.push((kind.clone(), policy));
                    } else {
                        let kinds = TOOL_KINDS.join(", ");
                        return Err(format!(
                            "unknown tool kind {kind:?} (kinds: default, {kinds})"
                        ));
                    }
                }
            }
        }
        Ok(rules)
    }

    pub fn for_kind(&self, kind: &str) -> Permissions {
        self.kinds.iter().find(|(k, _)| k == kind).map_or(self.default, |(_, p)| *p)
    }

    /// `"ask"`, or `{"default": "ask", "read": "auto-allow", …}`.
    pub fn describe(&self) -> Value {
        if self.kinds.is_empty() {
            return json!(self.default.name());
        }
        let mut map = serde_json::Map::new();
        map.insert("default".into(), json!(self.default.name()));
        for (kind, policy) in &self.kinds {
            map.insert(kind.clone(), json!(policy.name()));
        }
        Value::Object(map)
    }
}

/// Passed by `brnr proxy` or `brnr start`; not a user interface.
#[derive(Default)]
struct Args {
    link_fd: Option<RawFd>,
    ready_fd: Option<RawFd>,
    proxy_pid: Option<u32>,
    profile: Option<String>,
    name: Option<String>,
    on_disconnect: Option<String>,
    sigmask: Vec<c_int>,
    prompt: Option<String>,
    cwd: Option<String>,
    /// Seconds `brnr start` waits for the session.
    start_timeout: Option<u64>,
    resume: Option<String>,
    mode: Option<String>,
    set: Vec<(String, String)>,
    permissions: Option<String>,
    stop_when_idle: bool,
    quiet: bool,
    program: Vec<OsString>,
}

pub fn main(args: impl Iterator<Item = OsString>) -> ExitCode {
    let mut args = match parse_args(args) {
        Ok(args) => args,
        Err(None) => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(Some(msg)) => {
            eprintln!("brnr host: {msg}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    if let Some(dir) = &args.cwd
        && let Err(err) = std::env::set_current_dir(paths::expand(dir))
    {
        eprintln!("brnr host: {dir}: {err}");
        return ExitCode::from(2);
    }
    if args.prompt.as_deref() == Some("-") {
        let mut text = String::new();
        if let Err(err) = io::stdin().read_to_string(&mut text) {
            eprintln!("brnr host: stdin: {err}");
            return ExitCode::from(2);
        }
        args.prompt = Some(text);
    }
    let keep: Vec<RawFd> = [args.link_fd, args.ready_fd].into_iter().flatten().collect();
    close_inherited_fds(&keep);
    keep.iter().for_each(|&fd| set_cloexec(fd));
    let link = args.link_fd.map(|fd| unsafe { UnixStream::from_raw_fd(fd) });
    let ready = args.ready_fd.map(|fd| unsafe { File::from_raw_fd(fd) });
    let mut failure = Failure {
        link: link.as_ref().and_then(|l| l.try_clone().ok()),
        ready: ready.as_ref().and_then(|r| r.try_clone().ok()),
    };

    match Host::start(args, link, ready) {
        Ok(host) => host.run(),
        Err((msg, code)) => {
            failure.report(&msg, code);
            ExitCode::from(code)
        }
    }
}

const OPTIONS: &[&str] = &[
    "--link-fd",
    "--ready-fd",
    "--proxy-pid",
    "--profile",
    "--name",
    "--on-disconnect",
    "--prompt",
    "--cwd",
    "--sigmask",
    "--start-timeout",
    "--resume",
    "--mode",
    "--set",
    "--permissions",
];

/// Options without a value.
const FLAGS: &[&str] = &["--stop-when-idle", "--quiet"];

fn number<T: std::str::FromStr>(key: &str, value: &str) -> Result<T, Option<String>> {
    value.parse().map_err(|_| Some(format!("{key}: not a number: {value}")))
}

/// `Err(None)` asks for the usage.
fn parse_args(mut args: impl Iterator<Item = OsString>) -> Result<Args, Option<String>> {
    let mut a = Args::default();
    while let Some(arg) = args.next() {
        if arg == "--" {
            a.program = args.collect();
            break;
        }
        let arg = arg.into_string().map_err(|a| Some(format!("unknown option: {a:?}")))?;
        if arg == "-h" || arg == "--help" {
            return Err(None);
        }
        match arg.as_str() {
            "--stop-when-idle" => {
                a.stop_when_idle = true;
                continue;
            }
            "--quiet" => {
                a.quiet = true;
                continue;
            }
            _ => {}
        }
        debug_assert!(!FLAGS.contains(&arg.as_str()));
        let (key, inline) = match arg.split_once('=') {
            Some((key, value)) if OPTIONS.contains(&key) => {
                (key.to_owned(), Some(value.to_owned()))
            }
            _ => (arg, None),
        };
        if !OPTIONS.contains(&key.as_str()) {
            return Err(Some(format!("unknown option: {key}")));
        }
        let value = match inline {
            Some(value) => value,
            None => args
                .next()
                .ok_or_else(|| format!("{key} needs a value"))?
                .into_string()
                .map_err(|_| format!("bad value for {key}"))?,
        };
        match key.as_str() {
            "--link-fd" => a.link_fd = Some(number(&key, &value)?),
            "--ready-fd" => a.ready_fd = Some(number(&key, &value)?),
            "--proxy-pid" => a.proxy_pid = Some(number(&key, &value)?),
            "--profile" => a.profile = Some(value),
            "--name" => a.name = Some(value),
            "--on-disconnect" => a.on_disconnect = Some(value),
            "--prompt" => a.prompt = Some(value),
            "--cwd" => a.cwd = Some(value),
            "--start-timeout" => a.start_timeout = Some(number(&key, &value)?),
            "--resume" => a.resume = Some(value),
            "--mode" => a.mode = Some(value),
            "--set" => match value.split_once('=') {
                Some((k, v)) => a.set.push((k.to_owned(), v.to_owned())),
                None => return Err(Some(format!("--set takes <option>=<value>, not {value}"))),
            },
            "--permissions" => a.permissions = Some(value),
            "--sigmask" => {
                a.sigmask = value.split(',').filter_map(|s| s.parse().ok()).collect();
            }
            _ => unreachable!("checked against OPTIONS"),
        }
    }
    Ok(a)
}

/// Where a startup failure is reported: the proxy, or brnr start.
struct Failure {
    link: Option<UnixStream>,
    ready: Option<File>,
}

impl Failure {
    fn report(&mut self, msg: &str, code: u8) {
        eprintln!("brnr host: {msg}");
        if let Some(link) = &mut self.link {
            let report = json!({ "error": msg, "code": code }).to_string();
            let _ = frame::write(link, frame::FAILED, report.as_bytes());
        }
        if let Some(ready) = &mut self.ready {
            let _ = writeln!(ready, "{}", json!({ "ok": false, "error": msg }));
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
    BridgeExited {
        label: String,
        status: Option<i32>,
        pid: pid_t,
    },
    /// A signal sent to the host itself.
    Signal(c_int),
}

enum StopStage {
    Term,
    Kill,
}

struct Host {
    /// Run by hand in a terminal: no proxy, no brnr waiting.
    manual: bool,
    /// By hand, a failed start has been reported on stderr.
    startup_reported: bool,
    info: Value,
    host_id: String,
    policy: Policy,
    permissions: PermissionRules,
    /// How long an unanswered permission request waits before it is denied.
    permission_timeout: Option<Duration>,
    agent_pid: pid_t,
    /// Bytes for the agent's stdin. Written on their own thread, so an agent
    /// that stops reading can't stall the host; dropping this closes the
    /// agent's stdin once what's queued is written.
    agent_in: Option<Sender<Vec<u8>>>,
    /// Dropping this makes the stdout reader close the agent's stdout.
    stop_stdout: Option<PipeWriter>,
    /// Frames for the link writer while an editor is attached. Writes happen
    /// on their own thread so an editor that stops reading can't stall the
    /// host: signals, the control socket and bridges keep working.
    link: Option<Sender<(u8, Vec<u8>)>>,
    link_writer: Option<thread::JoinHandle<()>>,
    /// brnr start waits on this for the first session.
    ready: Option<File>,
    /// When brnr start gives up waiting for the session.
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
    /// Every unanswered request to the agent → the session it is about.
    client_requests: HashMap<String, Option<String>>,
    /// Requests the host itself sent the agent as its client.
    host_requests: HashMap<String, HostRequest>,
    /// Requests from the agent to its client that are unanswered.
    agent_requests: Vec<AgentRequest>,
    /// Request id of every unanswered prompt → its session.
    prompt_session: HashMap<String, String>,
    next_id: u64,
    next_permission: u64,
    /// Bytes from the editor after the last complete line.
    editor_buf: Vec<u8>,
    /// Headless start: the first prompt, sent once the session exists.
    first_prompt: Option<String>,
    /// Headless start: resume this session instead of opening a new one.
    resume: Option<String>,
    /// Headless start: mode and config options to set before the prompt.
    setup: std::collections::VecDeque<SetupStep>,
    /// Headless start: the session being opened.
    starting: Option<String>,
    /// MCP servers for the sessions the host opens, as ACP has them.
    mcp_servers: Vec<Value>,
    /// Stop once a turn has ended and nothing is running or held.
    stop_when_idle: bool,
    /// By hand: show the session's events on stdout.
    show_events: bool,
    /// What the agent said it can do in `initialize`.
    agent_caps: Value,
    auth_methods: Value,
    next_message: u64,
    started: Instant,

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
    fn start(a: Args, link: Option<UnixStream>, ready: Option<File>) -> Result<Host, (String, u8)> {
        let profile = config::load(a.profile.as_deref()).map_err(|e| (e, 2))?;
        let policy = a
            .on_disconnect
            .as_deref()
            .or(profile.on_disconnect.as_deref())
            .map_or(Ok(Policy::Direct), Policy::parse)
            .map_err(|e| (e, 2))?;
        let mut permissions =
            PermissionRules::parse(profile.permissions.as_ref()).map_err(|e| (e, 2))?;
        if let Some(policy) = &a.permissions {
            permissions.default = Permissions::parse(policy).map_err(|e| (e, 2))?;
        }
        let mcp_servers = profile
            .mcp_servers
            .iter()
            .map(config::McpServer::to_acp)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| (e, 2))?;
        let mut config_options: std::collections::BTreeMap<String, String> =
            profile.config.clone().unwrap_or_default();
        config_options.extend(a.set.iter().cloned());
        let setup = requests::setup_steps(
            a.mode.clone().or(profile.mode.clone()),
            config_options.into_iter().collect(),
        );
        for bridge in &profile.bridges {
            control::check_bridge(bridge).map_err(|e| (e, 2))?;
        }
        if a.prompt.as_deref().is_some_and(|p| p.trim().is_empty()) {
            return Err(("--prompt is empty".into(), 2));
        }
        // An editor's name is its own business; a headless session's name
        // is how brnr finds it, so it has to be unique.
        if link.is_none()
            && let Some(name) = &a.name
            && let Some(id) = running_named(name)
        {
            return Err((format!("a host named {name} is already running ({id})"), 2));
        }
        let mut program = a.program.clone();
        if program.is_empty() {
            program = profile.agent.iter().flatten().map(|s| paths::expand(s).into()).collect();
        }
        if program.is_empty() {
            return Err(("no agent: give one after -- or set agent in the profile".into(), 2));
        }
        let cwd = std::env::current_dir().map_err(|e| (format!("cwd: {e}"), 1))?;

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

        if let Some(bundled) = spawn::bundled(&program[0]) {
            program[0] = bundled.into();
        }
        let mut cmd = Command::new(&program[0]);
        cmd.args(&program[1..]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        // Its own process group: a terminal's Ctrl-C (when run by hand) is
        // for the host, which stops the agent its own way.
        cmd.process_group(0);
        let mask = a.sigmask.clone();
        // std resets the mask in the child; give the agent the editor's.
        unsafe {
            cmd.pre_exec(move || {
                signals::set_mask(&mask);
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
        let log = if profile.log.unwrap_or(true) {
            Logger::start(ids, a.proxy_pid).map_err(|e| {
                cleanup();
                unsafe { libc::kill(agent_pid, libc::SIGKILL) };
                (format!("log: {e}"), 1)
            })?
        } else {
            Logger::disabled()
        };
        let sink = log.sink();

        let argv: Vec<String> = program.iter().map(|s| s.to_string_lossy().into_owned()).collect();
        let info = json!({
            "id": id,
            "host_id": host_id,
            "name": a.name,
            "profile": a.profile,
            "host_pid": std::process::id(),
            "proxy_pid": a.proxy_pid,
            "agent_pid": agent_pid,
            "agent": argv,
            "cwd": cwd.to_string_lossy(),
            "host_log": log.host_log().map(|p| p.to_string_lossy().into_owned()),
            "socket": sock_path.to_string_lossy(),
            "on_disconnect": policy.name(),
            "permissions": permissions.describe(),
            "started": log::rfc3339(started),
        });
        write_atomic(&meta_path, format!("{info:#}\n").as_bytes());
        sink.note(None, json!({ "event": "started", "info": info }));

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

        let manual = link.is_none() && ready.is_none();
        let start_deadline = (a.start_timeout)
            .filter(|_| ready.is_some())
            .map(|secs| Instant::now() + Duration::from_secs(secs));
        let mut host = Host {
            manual,
            startup_reported: false,
            info,
            host_id,
            policy,
            permissions,
            permission_timeout: profile.permission_timeout.map(Duration::from_secs),
            agent_pid,
            agent_in: Some(agent_in),
            stop_stdout: Some(stop_tx),
            link,
            link_writer,
            ready,
            start_deadline,
            log,
            sink,
            rx,
            sock_path,
            meta_path,
            cwd: cwd.clone(),
            sessions: Vec::new(),
            pending: HashMap::new(),
            client_requests: HashMap::new(),
            host_requests: HashMap::new(),
            agent_requests: Vec::new(),
            prompt_session: HashMap::new(),
            next_id: 0,
            next_permission: 0,
            editor_buf: Vec::new(),
            first_prompt: a.prompt,
            resume: a.resume,
            setup,
            starting: None,
            mcp_servers,
            stop_when_idle: a.stop_when_idle || profile.stop_when_idle.unwrap_or(false),
            show_events: manual && !a.quiet,
            agent_caps: Value::Null,
            auth_methods: Value::Null,
            next_message: 0,
            started: Instant::now(),
            peers: HashMap::new(),
            bridge_pids: Vec::new(),
            status: None,
            stdout_open: true,
            stderr_open: true,
            drain_until: None,
            stop_requested: false,
            stopping: None,
        };
        for (n, bridge) in profile.bridges.iter().enumerate() {
            if let Err(err) = host.start_bridge(n, bridge, &tx) {
                host.kill_all();
                let _ = fs::remove_file(&host.sock_path);
                let _ = fs::remove_file(&host.meta_path);
                return Err((err, 2));
            }
        }
        if host.link.is_some() {
            let ready = json!({ "id": id, "host_pid": std::process::id(), "agent_pid": agent_pid });
            host.send_link(frame::READY, ready.to_string().as_bytes());
        } else {
            host.begin_headless_start();
        }
        if host.manual {
            eprintln!(
                "brnr host: {id}: started {} (pid {agent_pid}) in {}; Ctrl-C to stop",
                argv.join(" "),
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
            let stop_at = self.stopping.as_ref().map(|(t, _)| *t);
            let permission_at = self.next_permission_deadline();
            let wake = [self.drain_until, stop_at, self.start_deadline, permission_at]
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
            self.handle(ev);
        }
        self.finish()
    }

    fn finish(mut self) -> ExitCode {
        for i in 0..self.sessions.len() {
            self.flush_agent_message(i);
        }
        if let Some(status) = self.status {
            self.send_link(frame::EXIT, &status.to_be_bytes());
        }
        self.startup_failed("the agent exited before the session started");
        let status = describe_status(self.status);
        // Held messages that never became a prompt.
        let undelivered: Vec<Value> = self
            .sessions
            .iter()
            .flat_map(|s| {
                s.held.iter().map(|h| json!({ "session": s.id, "message": h.id, "text": h.text }))
            })
            .collect();
        let mut event = json!({ "event": "exited", "status": status, "undelivered": undelivered });
        self.emit(event.clone());
        event["ts"] = json!(log::rfc3339(SystemTime::now()));
        event["host_id"] = json!(self.host_id);
        // Each session's transcript says how it ended, too.
        for s in &self.sessions {
            self.sink.note(Some(&s.id), event.clone());
        }
        let _ = fs::remove_file(&self.sock_path);
        let _ = fs::remove_file(&self.meta_path);
        // Bridges also see EOF on their stdin once we exit.
        // The last events (`exited`) reach the peers before we exit, and only
        // then do started bridges get SIGTERM.
        self.flush_peers(PEER_FLUSH);
        for &pid in &self.bridge_pids {
            unsafe { libc::kill(pid, libc::SIGTERM) };
        }
        if self.manual {
            eprintln!("brnr host: agent exited: {status}");
        }
        // Let the writer deliver what's queued (it gives up on a proxy that
        // stopped reading; see LINK_WRITE_TIMEOUT).
        self.link = None;
        if let Some(writer) = self.link_writer.take() {
            let _ = writer.join();
        }
        self.log.finish();
        match self.status {
            // By hand, exit as the agent did, like a shell reports it.
            Some(s) if self.manual && libc::WIFEXITED(s) => {
                ExitCode::from(libc::WEXITSTATUS(s) as u8)
            }
            Some(s) if self.manual => ExitCode::from(128 + libc::WTERMSIG(s) as u8),
            None if self.manual => ExitCode::FAILURE,
            _ => ExitCode::SUCCESS,
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
            Ev::BridgeExited { label, status, pid } => {
                self.bridge_pids.retain(|&p| p != pid);
                self.sink.note(
                    None,
                    json!({ "event": "bridge-exited", "bridge": label, "status": status }),
                );
            }
        }
    }

    // ---- the editor's side of the link --------------------------------

    fn editor_attached(&self) -> bool {
        self.link.is_some()
    }

    fn signal(&mut self, sig: c_int) {
        let detaching = matches!(sig, libc::SIGHUP | libc::SIGINT | libc::SIGTERM);
        if self.policy == Policy::Headless && detaching {
            self.go_headless(&format!("proxy got signal {sig}"));
        } else if self.status.is_none() {
            self.sink.note(None, json!({ "event": "signal", "signal": sig }));
            unsafe { libc::kill(self.agent_pid, sig) };
        }
    }

    fn editor_eof(&mut self) {
        if self.policy == Policy::Headless {
            return self.go_headless("editor closed stdin");
        }
        let rest = take(&mut self.editor_buf);
        if !rest.is_empty() {
            self.record(None, Dir::EditorToAgent, &rest);
            self.write_agent(&rest);
        }
        self.sink.note(None, json!({ "event": "editor-closed-stdin" }));
        self.agent_in = None;
    }

    fn editor_stopped_reading(&mut self) {
        self.sink.note(None, json!({ "event": "editor-stopped-reading" }));
        match self.policy {
            Policy::Headless => self.go_headless("editor stopped reading"),
            // The agent gets EPIPE, just as it would directly.
            Policy::Direct => self.stop_stdout = None,
        }
    }

    fn link_gone(&mut self) {
        if self.link.is_none() || self.status.is_some() {
            self.link = None;
            return; // Already headless, or the proxy left after the agent.
        }
        match self.policy {
            Policy::Headless => self.go_headless("editor disconnected"),
            Policy::Direct => {
                self.link = None;
                self.sink.note(None, json!({ "event": "editor-disconnected", "policy": "direct" }));
                self.agent_in = None;
                unsafe { libc::kill(self.agent_pid, libc::SIGKILL) };
            }
        }
    }

    /// The editor is gone (or going) and the session carries on with the
    /// host as the agent's client. The proxy is told to exit 0.
    fn go_headless(&mut self, reason: &str) {
        if self.link.is_none() || self.status.is_some() {
            return;
        }
        self.send_link(frame::DETACHED, &[]);
        self.link = None;
        self.editor_buf.clear();
        self.sink.set_proxy(None);
        self.info["proxy_pid"] = Value::Null;
        write_atomic(&self.meta_path, format!("{:#}\n", self.info).as_bytes());
        self.sink
            .note(None, json!({ "event": "owner-changed", "owner": "host", "reason": reason }));
        self.emit(json!({ "event": "owner_changed", "owner": "host", "reason": reason }));
        self.take_over_agent_requests();
    }

    fn send_link(&mut self, kind: u8, payload: &[u8]) {
        if let Some(link) = &self.link
            && link.send((kind, payload.to_vec())).is_err()
        {
            // The writer gave up; the reader thread reports the disconnect.
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
                if self.manual {
                    eprintln!("brnr host: killing the agent");
                }
                self.kill_group(libc::SIGKILL);
            }
            _ => {
                if self.manual {
                    eprintln!("brnr host: stopping (again to kill)");
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

    /// brnr start stops waiting for the session at the deadline, so the
    /// host gives up too rather than carry on where nobody knows about it.
    fn fire_start_timer(&mut self, now: Instant) {
        if self.start_deadline.is_some_and(|t| now >= t) {
            self.start_deadline = None;
            if self.ready.is_some() {
                self.fail_start("timed out waiting for the session");
            }
        }
    }

    /// Signals the agent's process group (it leads its own; see `start`).
    /// Only while the agent hasn't been reaped, so the group id is ours.
    fn kill_group(&self, sig: c_int) {
        if self.status.is_none() {
            unsafe { libc::kill(-self.agent_pid, sig) };
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
/// fails or times out.
fn write_link(mut link: UnixStream, frames: Receiver<(u8, Vec<u8>)>) {
    let _ = link.set_write_timeout(Some(LINK_WRITE_TIMEOUT));
    for (kind, payload) in frames {
        if frame::write(&mut link, kind, &payload).is_err() {
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

/// The id of a running host called `name`, from the metadata files.
pub fn running_named(name: &str) -> Option<String> {
    fs::read_dir(paths::runtime_dir())
        .ok()?
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| serde_json::from_slice::<Value>(&fs::read(e.path()).ok()?).ok())
        .find(|meta| meta["name"] == name && meta["host_pid"].as_i64().is_some_and(alive))
        .map(|meta| meta["id"].as_str().unwrap_or("?").to_owned())
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
