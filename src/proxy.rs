//! The editor-facing process: `brnr proxy [options] [-- <program> [args...]]`.
//!
//! The editor launches this as if it were the agent. It starts a host (see
//! host/) in a session of its own, which runs the agent, and from then on
//! only relays: stdin and stdout carry ACP to and from the host, the agent's
//! stderr comes out of the proxy's stderr, signals the proxy receives are
//! handed to the host, and the proxy exits with the agent's exact wait
//! status, or with 0 if the session carries on headless. The agent never
//! holds the editor's file descriptors and is out of reach of the editor's
//! process group and process tree, so what happens to it when the proxy
//! dies is the host's --on-disconnect policy.

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

use crate::{frame, signals, spawn};

const USAGE: &str = "usage: brnr proxy [--profile <name>] [--name <name>] \
                     [--on-disconnect direct|headless] [-- <program> [args...]]";

/// The fd the host finds its end of the link on.
const HOST_LINK_FD: c_int = 3;

#[derive(Default)]
struct Options {
    profile: Option<String>,
    name: Option<String>,
    on_disconnect: Option<String>,
}

type Link = Arc<Mutex<UnixStream>>;

pub fn main(args: impl Iterator<Item = OsString>) -> ExitCode {
    let (opts, program) = match parse_args(args) {
        Ok(parsed) => parsed,
        Err(msg) => {
            eprintln!("brnr proxy: {msg}\n{USAGE}");
            return ExitCode::from(2);
        }
    };

    let mask = signals::current_mask();
    let signals = signals::install();
    let (link, theirs) = match UnixStream::pair() {
        Ok(pair) => pair,
        Err(err) => {
            eprintln!("brnr proxy: socketpair: {err}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(err) = start_host(&opts, &mask, theirs, program) {
        eprintln!("brnr proxy: starting host: {err}");
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
            "--name" => opts.name = Some(value()?),
            "--on-disconnect" => opts.on_disconnect = Some(value()?),
            _ => return Err(format!("unknown option: {key}")),
        }
    }
    Ok((opts, args.collect()))
}

/// Starts the host detached (see spawn.rs) with its stdio on /dev/null; its
/// only connection to us is `theirs`, moved to fd 3.
fn start_host(
    opts: &Options,
    mask: &[c_int],
    theirs: UnixStream,
    program: Vec<OsString>,
) -> io::Result<()> {
    let mut cmd = spawn::host_command()?;
    cmd.arg("--link-fd")
        .arg(HOST_LINK_FD.to_string())
        .arg("--proxy-pid")
        .arg(std::process::id().to_string());
    for (flag, value) in [
        ("--profile", &opts.profile),
        ("--name", &opts.name),
        ("--on-disconnect", &opts.on_disconnect),
    ] {
        if let Some(value) = value {
            cmd.arg(flag).arg(value);
        }
    }
    if !mask.is_empty() {
        let list: Vec<String> = mask.iter().map(c_int::to_string).collect();
        cmd.arg("--sigmask").arg(list.join(","));
    }
    if !program.is_empty() {
        cmd.arg("--").args(program);
    }
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    spawn::detached(&mut cmd, theirs.as_raw_fd(), HOST_LINK_FD).map(drop)
}

/// Frames from the host → our stdout and stderr, until the agent's exit
/// status arrives or the session goes headless.
fn run(link: UnixStream, writer: Link) -> ExitCode {
    let mut reader = BufReader::new(link);
    let mut stdout = ManuallyDrop::new(unsafe { File::from_raw_fd(1) });
    let mut stderr = ManuallyDrop::new(unsafe { File::from_raw_fd(2) });
    let mut stdout_open = true;
    loop {
        let (kind, payload) = match frame::read(&mut reader) {
            Ok(Some(frame)) => frame,
            Ok(None) | Err(_) => {
                eprintln!("brnr proxy: lost the connection to the host");
                return ExitCode::FAILURE;
            }
        };
        match kind {
            frame::DATA => {
                if stdout_open && stdout.write_all(&payload).is_err() {
                    // The caller stopped reading; the host treats that as the
                    // editor going away.
                    stdout_open = false;
                    send(&writer, frame::STDOUT_CLOSED, &[]);
                }
            }
            frame::STDERR => {
                let _ = stderr.write_all(&payload);
            }
            frame::FAILED => {
                let failure: Value = serde_json::from_slice(&payload).unwrap_or_default();
                let msg = failure["error"].as_str().unwrap_or("host failed to start");
                eprintln!("brnr proxy: {msg}");
                return ExitCode::from(failure["code"].as_u64().unwrap_or(1) as u8);
            }
            frame::EXIT if payload.len() == 4 => {
                return mirror(i32::from_be_bytes(payload.try_into().unwrap()));
            }
            frame::DETACHED => return ExitCode::SUCCESS,
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
            Err(_) => break,
        };
        if !send(&link, frame::DATA, &buf[..n]) {
            return;
        }
    }
    send(&link, frame::EOF, &[]);
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
