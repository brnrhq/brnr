//! Host-death cleanup of the agent's group (proposed ADR 3.7).

use std::io::{self, PipeWriter};
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::process::Command;

use crate::sys;

/// Room for fds `Command::spawn` opens after [`attach`] scans (stdio pipes in
/// the parent, std's exec-error pipe in the child). Far below any soft
/// `RLIMIT_NOFILE` that would make a `_SC_OPEN_MAX` loop expensive.
const SPAWN_FD_MARGIN: i32 = 32;

/// Double-forked before the agent execs, in its group: the watcher pins that group
/// even if the host dies and the agent is reaped. It signals only its own
/// group, never an id recovered from metadata. The host alone keeps the
/// write end; exec closes the agent's copy. Normal group cleanup kills the
/// watcher too. The watcher uses no Rust runtime after fork.
pub(super) fn attach(cmd: &mut Command) -> io::Result<PipeWriter> {
    let (reader, writer) = io::pipe()?;
    let fd = reader.as_raw_fd();
    // Bound by fds open now, not RLIMIT_NOFILE / _SC_OPEN_MAX. Scan here in
    // the host (malloc/readdir are fine); the watcher only closes.
    let through =
        sys::highest_open_fd()?.max(fd).max(writer.as_raw_fd()).saturating_add(SPAWN_FD_MARGIN);
    // SAFETY: the pre-exec closure and fork child use only async-signal-safe
    // system calls and stack data. The child never returns into Rust. Closing
    // all fds also closes std's exec-error pipe, so spawn can finish. The
    // agent reaps only the intermediate; the persistent watcher is already
    // reparented before exec, so it cannot interfere with agent child waits
    // or be moved by the agent's setpgid(child, ...) calls.
    unsafe {
        cmd.pre_exec(move || {
            let _keep_reader = &reader;
            // Reap the intermediate even when the editor ignored SIGCHLD.
            let sigchld = libc::signal(libc::SIGCHLD, libc::SIG_DFL);
            let intermediate = match libc::fork() {
                -1 => return Err(io::Error::last_os_error()),
                0 => {
                    match libc::fork() {
                        -1 => libc::_exit(
                            io::Error::last_os_error().raw_os_error().unwrap_or(libc::EAGAIN),
                        ),
                        0 => {}
                        _ => libc::_exit(0),
                    }
                    let mut mask = std::mem::zeroed();
                    libc::sigfillset(&mut mask);
                    libc::sigprocmask(libc::SIG_SETMASK, &mask, std::ptr::null_mut());
                    if libc::dup2(fd, 0) < 0 {
                        sys::kill(0, libc::SIGKILL);
                        libc::_exit(1);
                    }
                    // Keep only fd 0 (the lifetime read end). Linux:
                    // close_range(2); else close through the pre-fork high-water
                    // mark (man close(2)).
                    sys::close_fds_from(1, through);
                    let mut byte = 0u8;
                    // Nobody writes data. A read error also fails closed.
                    while libc::read(0, (&mut byte as *mut u8).cast(), 1) > 0 {}
                    sys::kill(0, libc::SIGKILL);
                    libc::_exit(1);
                }
                pid => pid,
            };
            let mut status = 0;
            while libc::waitpid(intermediate, &mut status, 0) < 0 {
                let err = io::Error::last_os_error();
                if err.kind() != io::ErrorKind::Interrupted {
                    return Err(err);
                }
            }
            libc::signal(libc::SIGCHLD, sigchld);
            if !libc::WIFEXITED(status) || libc::WEXITSTATUS(status) != 0 {
                let errno =
                    if libc::WIFEXITED(status) { libc::WEXITSTATUS(status) } else { libc::EIO };
                return Err(io::Error::from_raw_os_error(errno));
            }
            Ok(())
        });
    }
    Ok(writer)
}
