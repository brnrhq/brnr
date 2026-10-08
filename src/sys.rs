//! The system calls brnr makes that are sound whatever their arguments, as
//! safe functions. The others are `unsafe` where they are made, each with why
//! it is sound there.

use std::fs::File;
use std::mem::ManuallyDrop;
use std::os::fd::{FromRawFd, RawFd};

use libc::{c_int, pid_t};

/// Sends `sig` to `pid` (a negative pid: its process group; 0: only checks
/// it could be sent). Whether it was.
pub fn kill(pid: pid_t, sig: c_int) -> bool {
    // SAFETY: kill(2) takes no pointers and touches no memory of ours; any
    // pid and signal are valid arguments (a bad one is an error, EINVAL,
    // ESRCH or EPERM).
    unsafe { libc::kill(pid, sig) == 0 }
}

/// The user brnr runs as.
pub fn uid() -> u32 {
    // SAFETY: getuid(2) takes nothing and can't fail.
    unsafe { libc::getuid() }
}

/// Stdin, stdout or stderr (`fd` 0, 1 or 2) as a `File`, unbuffered and
/// unlocked, that leaves the descriptor open when dropped.
pub fn stdio(fd: RawFd) -> ManuallyDrop<File> {
    assert!((0..=2).contains(&fd), "fd {fd} isn't stdio");
    // SAFETY: the standard descriptors belong to the process for its whole
    // life (io::stdout() writes to fd 1 on the same terms), and ManuallyDrop
    // keeps this File from closing one: it only borrows it.
    ManuallyDrop::new(unsafe { File::from_raw_fd(fd) })
}
