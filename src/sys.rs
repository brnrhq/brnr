//! The system calls brnr makes that are sound whatever their arguments, as
//! safe functions. The others are `unsafe` where they are made, each with why
//! it is sound there.

use std::fs::File;
use std::io;
use std::mem::ManuallyDrop;
use std::os::fd::{AsRawFd, FromRawFd, RawFd};

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

/// Ends stdout for whoever reads it, as closing it would: fd 1 is left open
/// on /dev/null, so that no file opened later is taken for stdout, and what
/// writes to it then writes nowhere.
pub fn end_stdout() -> io::Result<()> {
    let null = File::options().write(true).open("/dev/null")?;
    loop {
        // SAFETY: dup2(2) takes no pointers; `null` is open, and fd 1 is the
        // process's (see `stdio`): what holds it goes on to hold /dev/null.
        if unsafe { libc::dup2(null.as_raw_fd(), 1) } >= 0 {
            return Ok(());
        }
        let err = io::Error::last_os_error();
        if err.kind() != io::ErrorKind::Interrupted {
            return Err(err);
        }
    }
}

/// The kernel's name, release and machine (`uname -srm`).
pub fn uname() -> Option<(String, String, String)> {
    // SAFETY: utsname is plain data, for which all zeros is a valid value.
    let mut name: libc::utsname = unsafe { std::mem::zeroed() };
    // SAFETY: uname(3) fills the utsname it is given, which is ours and
    // large enough, with NUL-terminated strings.
    if unsafe { libc::uname(&mut name) } != 0 {
        return None;
    }
    // c_char is i8 on some targets and u8 on others (aarch64 Linux): taking
    // its byte is right for both, where `as u8` is a cast clippy flags on one.
    let field = |chars: &[libc::c_char]| {
        let bytes: Vec<u8> =
            chars.iter().map(|c| c.to_ne_bytes()[0]).take_while(|&b| b != 0).collect();
        String::from_utf8_lossy(&bytes).into_owned()
    };
    Some((field(&name.sysname), field(&name.release), field(&name.machine)))
}
