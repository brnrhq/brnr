//! Catching signals sent to the proxy so the host can pass them to the agent
//! (and those that end `brnr notify`, so it stops its command first), and
//! carrying the caller's signal mask through to the agent; letting a host in
//! the foreground write to its terminal.

use std::io::{self, PipeReader};
use std::mem::zeroed;
use std::os::fd::IntoRawFd;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering::Relaxed};

use libc::{c_int, sigset_t};

/// Signals sent to the proxy that are meant for the program.
const FORWARDED: &[c_int] =
    &[libc::SIGHUP, libc::SIGINT, libc::SIGQUIT, libc::SIGTERM, libc::SIGUSR1, libc::SIGUSR2];

/// Write end of the pipe the handler reports signals on.
static PIPE: AtomicI32 = AtomicI32::new(-1);

/// Installs handlers for the forwarded signals and returns the pipe they are
/// reported on, one byte per signal. Signals the caller ignores stay ignored,
/// so the host, and through it the agent, inherits SIG_IGN (nohup).
pub fn install() -> PipeReader {
    catch(FORWARDED)
}

/// Installs handlers for `sigs`, as [`install`] does for the forwarded
/// ones (`brnr notify` stops what it runs first, on those that end it).
/// One process catches one set.
pub fn catch(sigs: &[c_int]) -> PipeReader {
    let (rx, tx) = io::pipe().expect("pipe");
    let tx = tx.into_raw_fd();
    // SAFETY: fcntl(2) and sigaction(2) are given tx (ours: into_raw_fd took
    // it from the pipe) and pointers to valid sigactions or null. All
    // zeros is a valid sigaction, and on_signal has the signature of an
    // sa_handler (no SA_SIGINFO).
    unsafe {
        // A full pipe must drop a signal rather than block the handler.
        libc::fcntl(tx, libc::F_SETFL, libc::fcntl(tx, libc::F_GETFL) | libc::O_NONBLOCK);
        PIPE.store(tx, Relaxed);

        for &sig in sigs {
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
    // SAFETY: a signal handler may only make async-signal-safe calls, as these
    // are: it reads and restores errno through the thread's own errno pointer,
    // and write(2)s a byte from the stack to PIPE (an atomic load).
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
    // SAFETY: all zeros is a valid sigset_t; pthread_sigmask(3) with a null
    // set only fills set, and sigismember reads it.
    unsafe {
        let mut set: sigset_t = zeroed();
        libc::pthread_sigmask(libc::SIG_BLOCK, null(), &mut set);
        (1..65).filter(|&sig| libc::sigismember(&set, sig) == 1).collect()
    }
}

/// Sets the calling thread's mask to exactly `sigs`. Safe between fork and
/// exec.
pub fn set_mask(sigs: &[c_int]) {
    // SAFETY: pthread_sigmask(3) is given a valid set and a null oldset; it is
    // async-signal-safe, as between fork and exec requires.
    unsafe { libc::pthread_sigmask(libc::SIG_SETMASK, &sigset(sigs), null_mut()) };
}

/// SIGTTOU as it was before [`write_from_background`]; `usize::MAX` (also
/// `SIG_ERR`) for unchanged.
static TTOU: AtomicUsize = AtomicUsize::new(usize::MAX);

/// Lets this process write to its terminal from a background process group:
/// under `stty tostop` its first write would otherwise stop it. `brnr start
/// --foreground` runs the host in a group of its own, writing to the
/// terminal.
pub fn write_from_background() {
    // SAFETY: signal(2) with SIG_IGN installs no handler and takes no
    // pointers.
    let old = unsafe { libc::signal(libc::SIGTTOU, libc::SIG_IGN) };
    TTOU.store(old, Relaxed);
}

/// Gives a child SIGTTOU back as this process found it (see
/// [`write_from_background`]). Safe between fork and exec.
pub fn restore_for_child() {
    let old = TTOU.load(Relaxed);
    if old != usize::MAX {
        // SAFETY: old is what signal(2) returned for SIGTTOU, a disposition to
        // put back; signal(2) is async-signal-safe.
        unsafe { libc::signal(libc::SIGTTOU, old) };
    }
}

/// Kills the whole process with `sig`'s default action.
pub fn raise_default(sig: c_int) {
    // SAFETY: a zeroed sigaction set to SIG_DFL and a valid sigset are what
    // sigaction(2) and pthread_sigmask(3) take; raise(3) takes a signal
    // number.
    unsafe {
        let mut dfl: libc::sigaction = zeroed();
        dfl.sa_sigaction = libc::SIG_DFL;
        libc::sigaction(sig, &dfl, null_mut());
        libc::pthread_sigmask(libc::SIG_UNBLOCK, &sigset(&[sig]), null_mut());
        libc::raise(sig);
    }
}

fn sigset(sigs: &[c_int]) -> sigset_t {
    // SAFETY: all zeros is a valid sigset_t, which sigemptyset(3) and
    // sigaddset(3) are given a pointer to.
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
    // SAFETY: __error(3) returns the calling thread's errno and has no
    // preconditions.
    unsafe { libc::__error() }
}

#[cfg(not(target_vendor = "apple"))]
unsafe fn errno() -> *mut c_int {
    // SAFETY: __errno_location(3) returns the calling thread's errno and has
    // no preconditions.
    unsafe { libc::__errno_location() }
}
