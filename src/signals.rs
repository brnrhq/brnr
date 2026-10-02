//! Catching signals sent to the proxy so the host can pass them to the agent,
//! and carrying the caller's signal mask through to the agent.

use std::io::{self, PipeReader};
use std::mem::zeroed;
use std::os::fd::IntoRawFd;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicI32, Ordering::Relaxed};

use libc::{c_int, sigset_t};

/// Signals sent to the proxy that are meant for the program.
const FORWARDED: &[c_int] = &[
    libc::SIGHUP,
    libc::SIGINT,
    libc::SIGQUIT,
    libc::SIGTERM,
    libc::SIGUSR1,
    libc::SIGUSR2,
];

/// Write end of the pipe the handler reports signals on.
static PIPE: AtomicI32 = AtomicI32::new(-1);

/// Installs handlers for the forwarded signals and returns the pipe they are
/// reported on, one byte per signal. Signals the caller ignores stay ignored,
/// so the host, and through it the agent, inherits SIG_IGN (nohup).
pub fn install() -> PipeReader {
    let (rx, tx) = io::pipe().expect("pipe");
    let tx = tx.into_raw_fd();
    unsafe {
        // A full pipe must drop a signal rather than block the handler.
        libc::fcntl(tx, libc::F_SETFL, libc::fcntl(tx, libc::F_GETFL) | libc::O_NONBLOCK);
        PIPE.store(tx, Relaxed);

        for &sig in FORWARDED {
            let mut old: libc::sigaction = zeroed();
            libc::sigaction(sig, null(), &mut old);
            if old.sa_sigaction == libc::SIG_IGN {
                continue;
            }
            let mut sa: libc::sigaction = zeroed();
            sa.sa_sigaction = on_signal as *const () as usize;
            sa.sa_flags = libc::SA_RESTART;
            libc::sigemptyset(&mut sa.sa_mask);
            libc::sigaction(sig, &sa, null_mut());
        }
    }
    rx
}

extern "C" fn on_signal(sig: c_int) {
    unsafe {
        let saved_errno = *errno();
        let byte = sig as u8;
        libc::write(PIPE.load(Relaxed), (&raw const byte).cast(), 1);
        *errno() = saved_errno;
    }
}

/// The signals blocked in the calling thread. std resets the mask when it
/// spawns a process, so the proxy hands this to the host explicitly.
pub fn current_mask() -> Vec<c_int> {
    unsafe {
        let mut set: sigset_t = zeroed();
        libc::pthread_sigmask(libc::SIG_BLOCK, null(), &mut set);
        (1..65).filter(|&sig| libc::sigismember(&set, sig) == 1).collect()
    }
}

/// Sets the calling thread's mask to exactly `sigs`. Safe between fork and
/// exec.
pub fn set_mask(sigs: &[c_int]) {
    unsafe { libc::pthread_sigmask(libc::SIG_SETMASK, &sigset(sigs), null_mut()) };
}

/// Kills the whole process with `sig`'s default action.
pub fn raise_default(sig: c_int) {
    unsafe {
        let mut dfl: libc::sigaction = zeroed();
        dfl.sa_sigaction = libc::SIG_DFL;
        libc::sigaction(sig, &dfl, null_mut());
        libc::pthread_sigmask(libc::SIG_UNBLOCK, &sigset(&[sig]), null_mut());
        libc::raise(sig);
    }
}

fn sigset(sigs: &[c_int]) -> sigset_t {
    unsafe {
        let mut set: sigset_t = zeroed();
        libc::sigemptyset(&mut set);
        for &sig in sigs {
            libc::sigaddset(&mut set, sig);
        }
        set
    }
}

#[cfg(target_vendor = "apple")]
unsafe fn errno() -> *mut c_int {
    unsafe { libc::__error() }
}

#[cfg(not(target_vendor = "apple"))]
unsafe fn errno() -> *mut c_int {
    unsafe { libc::__errno_location() }
}
