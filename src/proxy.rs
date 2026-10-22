//! The editor-facing process: `brnr acp [options] [-- <agent> [args...]]`,
//! a proxy between the editor and the host.
//!
//! The editor launches this as if it were the agent. It resolves the
//! profile and the agent into the host's start request (see request.rs),
//! starts the host (see host/) in a session of its own, which runs the
//! agent, and from then on only relays: stdin and stdout carry ACP to and
//! from the host, stdout ending when the agent's does, however long the
//! agent runs on; the agent's stderr comes out of the proxy's stderr as it
//! comes, newline or not; signals the proxy receives are handed to the host;
//! and the proxy exits with the agent's exact wait status. When the agent's
//! stdin closes, so does the proxy's, and the editor's next write fails as
//! it would on the agent's pipe. Signals, and its stdout failing, go on a
//! socket of their own, the signal link (see frame.rs): they don't wait
//! behind the editor's input while the agent isn't reading it, as they
//! wouldn't with the agent run directly. The agent never holds the editor's
//! file descriptors and is out of reach of the editor's process group and
//! process tree; when the proxy goes, the host stops it as if the editor had
//! run it.

use std::env;
use std::ffi::OsString;
use std::fs::File;
use std::io::{self, BufReader, ErrorKind, PipeReader, PipeWriter, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::process::{ExitCode, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

use libc::c_int;
use serde_json::Value;

use crate::request::{self, Request, Role};
use crate::{config, frame, signals, spawn, sys};

const USAGE: &str = "usage: brnr acp [--profile <p>] [--strict] [-- <agent> [args...]]";

/// `brnr acp --help`: the usage, and what it is.
const HELP: &str = "What an editor runs as its ACP agent, in place of the agent itself:
    brnr acp -- brnr-claude-adapter
The agent runs in a process of its own, which brnr's other commands can
reach (brnr session list, prompt send, event watch, permission allow, ...); it
stops when the editor goes.";

/// The fds the host finds its end of the link and of the signal link on.
const HOST_LINK_FD: c_int = 3;
const HOST_SIGNAL_LINK_FD: c_int = 4;

#[derive(Default)]
struct Options {
    profile: Option<String>,
    /// `--strict`: stable ACP to the letter (ADR 2.1), as `strict = true` in
    /// the profile.
    strict: bool,
}

/// The signal link, written from the thread relaying signals and from the
/// one relaying the host's frames.
type SignalLink = Arc<Mutex<UnixStream>>;

pub(crate) fn main(args: impl Iterator<Item = OsString>) -> ExitCode {
    // Help on stdout and 0, a usage error on stderr and 2 (ADR 8.3).
    let (opts, program) = match parse_args(args) {
        Ok(Some(parsed)) => parsed,
        Ok(None) => {
            println!("{USAGE}\n\n{HELP}");
            return ExitCode::SUCCESS;
        }
        Err(msg) => {
            eprintln!("brnr acp: {msg}\n{USAGE}");
            return ExitCode::from(2);
        }
    };

    // A config error is the editor's to see, as the process's own failures
    // are, and exits 1, as it does in the control commands (ADR 8.3).
    let request = match resolve(opts, program, signals::current_mask()) {
        Ok(request) => request,
        Err(msg) => {
            eprintln!("brnr acp: {msg}");
            return ExitCode::FAILURE;
        }
    };
    let signals = signals::install();
    let ((link, theirs), (signal_link, their_signal_link)) =
        match UnixStream::pair().and_then(|link| Ok((link, UnixStream::pair()?))) {
            Ok(pairs) => pairs,
            Err(err) => {
                eprintln!("brnr acp: socketpair: {err}");
                return ExitCode::FAILURE;
            }
        };
    let (stdin_closed, close_stdin) = match io::pipe() {
        Ok(pipe) => pipe,
        Err(err) => {
            eprintln!("brnr acp: pipe: {err}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(err) = start_host(&request, theirs, their_signal_link) {
        eprintln!("brnr acp: starting its process: {err}");
        return ExitCode::FAILURE;
    }

    let input = link.try_clone().expect("clone socket");
    thread::spawn(move || relay_stdin(input, stdin_closed));
    let signal_link: SignalLink = Arc::new(Mutex::new(signal_link));
    let s = signal_link.clone();
    thread::spawn(move || relay_signals(signals, s));
    run(link, signal_link, close_stdin)
}

/// Splits `[options] [-- <agent> [args...]]`; `None` for `-h` or `--help`,
/// as an option before any `--` (ADR 8.1).
fn parse_args(
    mut args: impl Iterator<Item = OsString>,
) -> Result<Option<(Options, Vec<String>)>, String> {
    let mut opts = Options::default();
    while let Some(arg) = args.next() {
        if arg == "--" {
            break;
        }
        let arg = arg.into_string().map_err(|a| format!("unknown option: {a:?}"))?;
        let (key, inline) = match arg.split_once('=') {
            Some((key, value)) => (key.to_owned(), Some(value.to_owned())),
            None => (arg, None),
        };
        let mut value = || -> Result<String, String> {
            match &inline {
                Some(value) => Ok(value.clone()),
                None => args
                    .next()
                    .ok_or(format!("{key} needs a value"))?
                    .into_string()
                    .map_err(|_| format!("bad value for {key}")),
            }
        };
        match key.as_str() {
            "-h" | "--help" if inline.is_none() => return Ok(None),
            "--profile" => opts.profile = Some(value()?),
            "--strict" if inline.is_none() => opts.strict = true,
            "--strict" => return Err("--strict takes no value".into()),
            _ => return Err(format!("unknown option: {key}")),
        }
    }
    let agent = args
        .map(|a| a.into_string().map_err(|a| format!("the agent's command isn't UTF-8: {a:?}")))
        .collect::<Result<Vec<String>, String>>()?;
    Ok(Some((opts, agent)))
}

/// The editor's process, as its start request has it: the profile's shared
/// and editor parts, the agent, our cwd, the editor's signal mask.
fn resolve(opts: Options, agent: Vec<String>, mask: Vec<c_int>) -> Result<Request, String> {
    let profile = config::load(opts.profile.as_deref()).map_err(|e| e.message)?;
    let cwd = env::current_dir().map_err(|e| format!("cwd: {e}"))?;
    let editor = request::Editor {
        proxy_pid: std::process::id(),
        sigmask: mask,
        experimental: profile.editor.experimental.clone(),
        client_capabilities: profile.editor.client_capabilities.clone(),
        echo_as: profile.editor.echo_as,
        features: profile.editor.features.clone(),
    };
    let role = Role::Editor(editor);
    // The editor's log has the message alone (ADR 8.7).
    let mut request =
        Request::new(opts.profile, &profile, agent, cwd, role).map_err(|e| e.message)?;
    request.strict |= opts.strict;
    Ok(request)
}

/// Starts the host detached (see spawn.rs) with its stdout and stderr on
/// /dev/null, writes it the request on its stdin, and closes that; its only
/// connections to us are then its ends of the link and the signal link,
/// moved to fds 3 and 4.
fn start_host(request: &Request, link: UnixStream, signal_link: UnixStream) -> io::Result<()> {
    let mut cmd = spawn::host_command()?;
    cmd.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null());
    let fds = [(link.as_raw_fd(), HOST_LINK_FD), (signal_link.as_raw_fd(), HOST_SIGNAL_LINK_FD)];
    let (stdin, _) = spawn::detached(&mut cmd, &fds)?;
    request.send(stdin.expect("piped"))
}

/// Frames from the host → our stdout and stderr, until the agent's exit
/// status arrives. Dropping `close_stdin` has `relay_stdin` close our stdin.
fn run(link: UnixStream, signal_link: SignalLink, close_stdin: PipeWriter) -> ExitCode {
    let mut reader = BufReader::new(link);
    let mut stdout = sys::stdio(1);
    let mut stderr = sys::stdio(2);
    let mut stdout_open = true;
    let mut close_stdin = Some(close_stdin);
    loop {
        let (kind, payload) = match frame::read(&mut reader) {
            Ok(Some(frame)) => frame,
            Ok(None) | Err(_) => {
                eprintln!("brnr acp: lost the connection to its process");
                return ExitCode::FAILURE;
            }
        };
        match kind {
            frame::DATA => {
                if stdout_open && write_all(&mut stdout, &payload).is_err() {
                    // The caller stopped reading: the host closes the
                    // agent's stdout, as a closed pipe would.
                    stdout_open = false;
                    send(&signal_link, frame::STDOUT_CLOSED, &[]);
                }
            }
            // The agent's stdout has ended: ours does, for the editor to
            // see EOF now rather than when we exit (ADR 2.6).
            frame::EOF => {
                stdout_open = false;
                if let Err(err) = sys::end_stdout() {
                    eprintln!("brnr acp: closing stdout: {err}");
                }
            }
            // The agent's stdin has closed: ours does, for the editor's
            // next write to fail as it would on the agent's pipe (ADR 2.6).
            frame::STDIN_CLOSED => drop(close_stdin.take()),
            frame::STDERR => {
                let _ = write_all(&mut stderr, &payload);
            }
            frame::FAILED => {
                let failure: Value = serde_json::from_slice(&payload).unwrap_or_default();
                let msg = failure["error"].as_str().unwrap_or("its process failed to start");
                eprintln!("brnr acp: {msg}");
                if crate::bug::is_panic(msg) {
                    eprintln!("{}", crate::bug::link(msg, "acp"));
                }
                return ExitCode::from(failure["code"].as_u64().unwrap_or(1) as u8);
            }
            frame::EXIT if payload.len() == 4 => {
                return mirror(i32::from_be_bytes(payload.try_into().unwrap()));
            }
            _ => {}
        }
    }
}

/// Exits the way the agent did, so our parent's waitpid() sees an identical
/// status.
fn mirror(status: c_int) -> ExitCode {
    if libc::WIFEXITED(status) {
        return ExitCode::from(libc::WEXITSTATUS(status) as u8);
    }
    signals::raise_default(libc::WTERMSIG(status));
    ExitCode::FAILURE
}

/// Our stdin → the host, then EOF so the host knows exactly when the caller
/// closed ours. The only writer of the link. When `closed` is readable (its
/// writer has gone: the agent's stdin has closed), our stdin is closed
/// instead, with no EOF: the host has no one to give it to.
fn relay_stdin(mut link: UnixStream, closed: PipeReader) {
    let mut input = sys::stdio(0);
    let mut buf = vec![0; 64 * 1024];
    let mut polled = true;
    loop {
        // In poll, not read: a read of fd 0 under way holds the editor's pipe
        // open (Linux), whatever another thread does to fd 0, and the
        // editor's next write would go through. Woken, this thread closes it.
        match wait_input(&input, &closed, polled) {
            Input::Ready => {}
            Input::Closed => {
                if let Err(err) = sys::end_stdin() {
                    eprintln!("brnr acp: closing stdin: {err}");
                }
                return;
            }
            Input::Unpolled => polled = false,
        }
        let n = match input.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(err) if err.kind() == ErrorKind::Interrupted => continue,
            Err(err) if err.kind() == ErrorKind::WouldBlock => {
                if !polled {
                    wait_ready(&*input, libc::POLLIN);
                }
                continue;
            }
            Err(_) => break,
        };
        if frame::write(&mut link, frame::DATA, &buf[..n]).is_err() {
            return;
        }
    }
    let _ = frame::write(&mut link, frame::EOF, &[]);
}

/// What `wait_input` found.
enum Input {
    /// Stdin can be read, or is at its end.
    Ready,
    /// Stdin is to be closed.
    Closed,
    /// Stdin can't be polled (macOS can't poll some devices): it is read
    /// blocking, as it was before, and closed once the read that waits
    /// returns.
    Unpolled,
}

/// Waits until stdin can be read or `closed` is readable, whichever comes
/// first (`closed` if both are); unless `stdin` is polled, only looks to see
/// whether `closed` is.
fn wait_input(input: &File, closed: &PipeReader, stdin: bool) -> Input {
    let mut fds = [closed.as_raw_fd(), input.as_raw_fd()].map(|fd| libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    });
    let (fds, timeout) = if stdin { (&mut fds[..], -1) } else { (&mut fds[..1], 0) };
    loop {
        match sys::poll(fds, timeout) {
            Ok(_) => break,
            Err(err) if err.kind() == ErrorKind::Interrupted => {}
            Err(_) => return Input::Unpolled,
        }
    }
    if fds[0].revents != 0 {
        Input::Closed
    } else if stdin && fds[1].revents & libc::POLLNVAL != 0 {
        Input::Unpolled
    } else {
        Input::Ready
    }
}

/// `write_all` that also works on a non-blocking descriptor, which an
/// editor may give us: O_NONBLOCK belongs to the open file, which the editor
/// may share, so it is waited out rather than cleared.
pub(crate) fn write_all(out: &mut File, mut bytes: &[u8]) -> io::Result<()> {
    while !bytes.is_empty() {
        match out.write(bytes) {
            Ok(0) => return Err(ErrorKind::WriteZero.into()),
            Ok(n) => bytes = &bytes[n..],
            Err(err) if err.kind() == ErrorKind::Interrupted => {}
            Err(err) if err.kind() == ErrorKind::WouldBlock => wait_ready(out, libc::POLLOUT),
            Err(err) => return Err(err),
        }
    }
    Ok(())
}

/// Waits until a non-blocking `fd` can be read or written (`events`).
fn wait_ready(fd: &impl AsRawFd, events: libc::c_short) {
    let mut p = [libc::pollfd { fd: fd.as_raw_fd(), events, revents: 0 }];
    while sys::poll(&mut p, -1).is_err_and(|err| err.kind() == ErrorKind::Interrupted) {}
}

/// The signals we get → the host, on the signal link: never behind our stdin.
fn relay_signals(mut signals: PipeReader, signal_link: SignalLink) {
    let mut sig = [0];
    loop {
        match signals.read(&mut sig) {
            Ok(1) => {}
            Err(err) if err.kind() == ErrorKind::Interrupted => continue,
            _ => return,
        }
        if !send(&signal_link, frame::SIGNAL, &(sig[0] as i32).to_be_bytes()) {
            return;
        }
    }
}

fn send(signal_link: &SignalLink, kind: u8, payload: &[u8]) -> bool {
    frame::write(&mut *signal_link.lock().unwrap(), kind, payload).is_ok()
}
