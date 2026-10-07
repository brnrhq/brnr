//! The editor-facing process: `brnr acp [options] [-- <program> [args...]]`,
//! a proxy between the editor and the host.
//!
//! The editor launches this as if it were the agent. It resolves the
//! profile and the agent into the host's start request (see request.rs),
//! starts the host (see host/) in a session of its own, which runs the
//! agent, and from then on only relays: stdin and stdout carry ACP to and
//! from the host, the agent's stderr comes out of the proxy's stderr,
//! signals the proxy receives are handed to the host, and the proxy exits
//! with the agent's exact wait status. The agent never holds the editor's
//! file descriptors and is out of reach of the editor's process group and
//! process tree; when the proxy goes, the host stops it as if the editor had
//! run it.

use std::env;
use std::ffi::OsString;
use std::fs::File;
use std::io::{self, BufReader, ErrorKind, PipeReader, Read, Write};
use std::mem::ManuallyDrop;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::net::UnixStream;
use std::process::{ExitCode, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

use libc::c_int;
use serde_json::Value;

use crate::request::{self, Request, Role};
use crate::{config, frame, signals, spawn};

const USAGE: &str = "usage: brnr acp [--profile <name>] [--strict] [-- <program> [args...]]";

/// `brnr acp --help`: the usage, and what it is.
const HELP: &str = "What an editor runs as its ACP agent, in place of the agent itself:
    brnr acp -- brnr-claude-adapter
The agent runs in a process of its own, which brnr's other commands can
reach (brnr list, send, watch, approve, ...); it stops when the editor goes.";

/// The fd the host finds its end of the link on.
const HOST_LINK_FD: c_int = 3;

#[derive(Default)]
struct Options {
    profile: Option<String>,
    /// `--strict`: stable ACP to the letter (ADR 41), as `strict = true` in
    /// the profile.
    strict: bool,
}

type Link = Arc<Mutex<UnixStream>>;

pub fn main(args: impl Iterator<Item = OsString>) -> ExitCode {
    let args: Vec<OsString> = args.collect();
    if args.first().is_some_and(|a| a == "-h" || a == "--help") {
        println!("{USAGE}\n\n{HELP}");
        return ExitCode::SUCCESS;
    }
    let (opts, program) = match parse_args(args.into_iter()) {
        Ok(parsed) => parsed,
        Err(msg) => {
            eprintln!("brnr acp: {msg}\n{USAGE}");
            return ExitCode::from(2);
        }
    };

    // A config error is the editor's to see, as the process's own failures
    // are.
    let request = match resolve(opts, program, signals::current_mask()) {
        Ok(request) => request,
        Err(msg) => {
            eprintln!("brnr acp: {msg}");
            return ExitCode::from(2);
        }
    };
    let signals = signals::install();
    let (link, theirs) = match UnixStream::pair() {
        Ok(pair) => pair,
        Err(err) => {
            eprintln!("brnr acp: socketpair: {err}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(err) = start_host(&request, theirs) {
        eprintln!("brnr acp: starting its process: {err}");
        return ExitCode::FAILURE;
    }

    let writer: Link = Arc::new(Mutex::new(link.try_clone().expect("clone socket")));
    let w = writer.clone();
    thread::spawn(move || relay_stdin(w));
    let w = writer.clone();
    thread::spawn(move || relay_signals(signals, w));
    run(link, writer)
}

/// Splits `[options] [-- <program> [args...]]`.
fn parse_args(
    mut args: impl Iterator<Item = OsString>,
) -> Result<(Options, Vec<OsString>), String> {
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
            "--profile" => opts.profile = Some(value()?),
            "--strict" if inline.is_none() => opts.strict = true,
            "--strict" => return Err("--strict takes no value".into()),
            _ => return Err(format!("unknown option: {key}")),
        }
    }
    Ok((opts, args.collect()))
}

/// The editor's process, as its start request has it: the profile's shared
/// and editor parts, the agent, our cwd, the editor's signal mask.
fn resolve(opts: Options, program: Vec<OsString>, mask: Vec<c_int>) -> Result<Request, String> {
    let profile = config::load(opts.profile.as_deref())?;
    let agent = (program.into_iter())
        .map(|a| a.into_string().map_err(|a| format!("the agent's command isn't UTF-8: {a:?}")))
        .collect::<Result<Vec<String>, String>>()?;
    let cwd = env::current_dir().map_err(|e| format!("cwd: {e}"))?;
    let editor = request::Editor {
        proxy_pid: std::process::id(),
        sigmask: mask,
        experimental: profile.editor.experimental.clone(),
        features: profile.editor.features.clone(),
    };
    let mut request = Request::new(opts.profile, &profile, agent, cwd, Role::Editor(editor))?;
    request.strict |= opts.strict;
    Ok(request)
}

/// Starts the host detached (see spawn.rs) with its stdout and stderr on
/// /dev/null, writes it the request on its stdin, and closes that; its only
/// connection to us is then `theirs`, moved to fd 3.
fn start_host(request: &Request, theirs: UnixStream) -> io::Result<()> {
    let mut cmd = spawn::host_command()?;
    cmd.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null());
    let stdin = spawn::detached(&mut cmd, theirs.as_raw_fd(), HOST_LINK_FD)?;
    request.send(stdin.expect("piped"))
}

/// Frames from the host → our stdout and stderr, until the agent's exit
/// status arrives.
fn run(link: UnixStream, writer: Link) -> ExitCode {
    let mut reader = BufReader::new(link);
    let mut stdout = ManuallyDrop::new(unsafe { File::from_raw_fd(1) });
    let mut stderr = ManuallyDrop::new(unsafe { File::from_raw_fd(2) });
    let mut stdout_open = true;
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
                    // The caller stopped reading; the host treats that as the
                    // editor going away.
                    stdout_open = false;
                    send(&writer, frame::STDOUT_CLOSED, &[]);
                }
            }
            frame::STDERR => {
                let _ = write_all(&mut stderr, &payload);
            }
            frame::FAILED => {
                let failure: Value = serde_json::from_slice(&payload).unwrap_or_default();
                let msg = failure["error"].as_str().unwrap_or("its process failed to start");
                eprintln!("brnr acp: {msg}");
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
/// closed ours.
fn relay_stdin(link: Link) {
    let mut input = ManuallyDrop::new(unsafe { File::from_raw_fd(0) });
    let mut buf = vec![0; 64 * 1024];
    loop {
        let n = match input.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(err) if err.kind() == ErrorKind::Interrupted => continue,
            Err(err) if err.kind() == ErrorKind::WouldBlock => {
                wait_ready(&*input, libc::POLLIN);
                continue;
            }
            Err(_) => break,
        };
        if !send(&link, frame::DATA, &buf[..n]) {
            return;
        }
    }
    send(&link, frame::EOF, &[]);
}

/// `write_all` that also works on a non-blocking descriptor, which an
/// editor may give us: O_NONBLOCK belongs to the open file, which the editor
/// may share, so it is waited out rather than cleared.
fn write_all(out: &mut File, mut bytes: &[u8]) -> io::Result<()> {
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
    let mut p = libc::pollfd { fd: fd.as_raw_fd(), events, revents: 0 };
    while unsafe { libc::poll(&mut p, 1, -1) } < 0
        && io::Error::last_os_error().kind() == ErrorKind::Interrupted
    {}
}

fn relay_signals(mut signals: PipeReader, link: Link) {
    let mut sig = [0];
    loop {
        match signals.read(&mut sig) {
            Ok(1) => {}
            Err(err) if err.kind() == ErrorKind::Interrupted => continue,
            _ => return,
        }
        if !send(&link, frame::SIGNAL, &(sig[0] as i32).to_be_bytes()) {
            return;
        }
    }
}

fn send(link: &Link, kind: u8, payload: &[u8]) -> bool {
    frame::write(&mut *link.lock().unwrap(), kind, payload).is_ok()
}
