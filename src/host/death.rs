//! A process's own death is recorded (ADR 11 in docs/adr).
//!
//! A detached process has no terminal: what it writes on stderr goes to its
//! host log (`host-stderr`), through a pipe on fd 2 that a thread reads.
//!
//! A panic, on any of its threads, ends the process. The hook shows it on
//! stderr as Rust does, then only takes note of it and wakes the event loop:
//! it takes no lock of brnr's and waits on no thread, so it can't deadlock
//! on whatever the panicking thread held, whether that is the event loop or
//! the logger. The event loop, which holds
//! what there is to tell (the sessions, the peers), then records the panic
//! in the host log and `exited`, with the reason, there and in every
//! session's file, sends `exited` to the peers it can still reach, removes
//! the `.sock` and `.json` files, and stops the agent (see `Host::died`).
//! A panic on the event loop itself unwinds to it (see `Host::run`).
//!
//! What can't be told: a panic on the logger's thread leaves nothing to
//! record with, and an abort (a stack overflow, a panic while panicking)
//! runs no hook at all.

use std::env;
use std::fs::OpenOptions;
use std::io::{self, BufRead, BufReader};
use std::os::fd::AsRawFd;
use std::panic::{self, PanicHookInfo};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::mpsc::SyncSender;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::Ev;
use crate::log::Sink;

/// Why the process is dying: the first panic.
static PANICKED: OnceLock<String> = OnceLock::new();

/// The event loop's channel, to wake it.
static WAKE: OnceLock<SyncSender<Ev>> = OnceLock::new();

/// `BRNR_TEST_PANIC` was set (see [`test_panic`]).
static TEST_PANIC: AtomicBool = AtomicBool::new(false);

/// Installs the hook. The panic is shown on stderr too, as Rust shows it:
/// in the foreground on the terminal, detached in the host log.
pub(super) fn install() {
    TEST_PANIC.store(env::var_os("BRNR_TEST_PANIC").is_some(), Relaxed);
    let shown = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        shown(info);
        if PANICKED.set(describe(info)).is_ok()
            && let Some(wake) = WAKE.get()
        {
            // Full, the event loop is busy and sees it next time round.
            let _ = wake.try_send(Ev::Panicked);
        }
    }));
}

/// From now on a panic wakes the event loop on `wake`.
pub(super) fn wake(wake: SyncSender<Ev>) {
    let _ = WAKE.set(wake);
}

/// Why the process is dying, if it is.
pub(super) fn panicked() -> Option<&'static str> {
    PANICKED.get().map(String::as_str)
}

/// `brnr panicked at src/host/acp.rs:120:5: <message>`.
fn describe(info: &PanicHookInfo) -> String {
    let payload = info.payload();
    let message = (payload.downcast_ref::<&str>().copied())
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("?");
    match info.location() {
        Some(at) => {
            format!("brnr panicked at {}:{}:{}: {message}", at.file(), at.line(), at.column())
        }
        None => format!("brnr panicked: {message}"),
    }
}

/// For the tests, with `BRNR_TEST_PANIC` in the process's environment: a
/// request `{"cmd": "panic", "on": "loop" | "thread"}` panics on the event
/// loop or on a thread of its own. Without it, the request is unknown.
pub(super) fn test_panic(req: &Value) {
    if !TEST_PANIC.load(Relaxed) || req["cmd"] != "panic" {
        return;
    }
    match req["on"].as_str() {
        Some("loop") => panic!("a test asked for it"),
        Some("thread") => drop(thread::spawn(|| panic!("a test asked for it"))),
        _ => {}
    }
}

/// Points fd 2 at a pipe whose lines go to the host log, as `host-stderr`.
/// Returns the thread reading it, for [`release_stderr`].
pub(super) fn capture_stderr(sink: Sink) -> Option<JoinHandle<()>> {
    let (reader, writer) = io::pipe().ok()?;
    if unsafe { libc::dup2(writer.as_raw_fd(), 2) } < 0 {
        return None;
    }
    drop(writer);
    Some(thread::spawn(move || {
        for line in BufReader::new(reader).split(b'\n').map_while(Result::ok) {
            if !line.trim_ascii().is_empty() {
                let text = String::from_utf8_lossy(&line);
                sink.note(None, json!({ "event": "host-stderr", "text": text }));
            }
        }
    }))
}

/// Puts fd 2 back on /dev/null, so the reader gets the rest of what was
/// written, then EOF; waits for it to be through, for up to `timeout`.
pub(super) fn release_stderr(reader: JoinHandle<()>, timeout: Duration) {
    if let Ok(null) = OpenOptions::new().write(true).open("/dev/null") {
        unsafe { libc::dup2(null.as_raw_fd(), 2) };
    }
    let until = Instant::now() + timeout;
    while !reader.is_finished() && Instant::now() < until {
        thread::sleep(Duration::from_millis(10));
    }
}
