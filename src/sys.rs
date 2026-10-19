//! The system calls brnr makes that are sound whatever their arguments, as
//! safe functions. The others are `unsafe` where they are made, each with why
//! it is sound there.

use std::ffi::CStr;
use std::fs::File;
use std::io;
use std::mem::ManuallyDrop;
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, RawFd};

use libc::{c_int, c_uint, mode_t, pid_t};

/// Sends `sig` to `pid` (a negative pid: its process group; 0: only checks
/// it could be sent). Whether it was.
pub(crate) fn kill(pid: pid_t, sig: c_int) -> bool {
    // SAFETY: kill(2) takes no pointers and touches no memory of ours; any
    // pid and signal are valid arguments (a bad one is an error, EINVAL,
    // ESRCH or EPERM).
    unsafe { libc::kill(pid, sig) == 0 }
}

/// Whether `pid` has exited: it is gone, isn't the user's, or is a zombie
/// its parent hasn't reaped yet, which `kill(pid, 0)` can't tell from a
/// live process (a host is reparented, and nothing may reap it soon).
#[cfg(target_os = "macos")]
pub(crate) fn exited(pid: pid_t) -> bool {
    if !kill(pid, 0) {
        return true;
    }
    // SAFETY: proc_bsdinfo is plain data, for which all zeros is a valid
    // value.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = size_of::<libc::proc_bsdinfo>() as c_int;
    // SAFETY: the buffer is `info`, and `size` is its size, as proc_pidinfo
    // fills it.
    let got =
        unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&raw mut info).cast(), size) };
    // One of the user's that can't be looked at has no task left.
    got != size || info.pbi_status == libc::SZOMB
}

/// Whether `pid` has exited: it is gone, isn't the user's, or is a zombie
/// its parent hasn't reaped yet, which `kill(pid, 0)` can't tell from a
/// live process (a host is reparented, and nothing may reap it soon).
#[cfg(not(target_os = "macos"))]
pub(crate) fn exited(pid: pid_t) -> bool {
    if !kill(pid, 0) {
        return true;
    }
    // `pid (comm) S …`: the state follows the last `)`, as comm may hold one.
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else { return true };
    let state = stat.rsplit_once(')').and_then(|(_, rest)| rest.trim_start().chars().next());
    matches!(state, Some('Z' | 'X'))
}

/// The user brnr runs as.
pub(crate) fn uid() -> u32 {
    // SAFETY: getuid(2) takes nothing and can't fail.
    unsafe { libc::getuid() }
}

/// Stdin, stdout or stderr (`fd` 0, 1 or 2) as a `File`, unbuffered and
/// unlocked, that leaves the descriptor open when dropped.
pub(crate) fn stdio(fd: RawFd) -> ManuallyDrop<File> {
    assert!((0..=2).contains(&fd), "fd {fd} isn't stdio");
    // SAFETY: the standard descriptors belong to the process for its whole
    // life (io::stdout() writes to fd 1 on the same terms), and ManuallyDrop
    // keeps this File from closing one: it only borrows it.
    ManuallyDrop::new(unsafe { File::from_raw_fd(fd) })
}

/// Ends stdout for whoever reads it, as closing it would: fd 1 is left open
/// on /dev/null, so that no file opened later is taken for stdout, and what
/// writes to it then writes nowhere.
pub(crate) fn end_stdout() -> io::Result<()> {
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
pub(crate) fn uname() -> Option<(String, String, String)> {
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

/// Opens `name` in the directory `dir`, as openat(2) does with `flags` (and
/// `O_CLOEXEC`), creating it with `mode` if `flags` says to. The mode is an
/// unsigned int, as a variadic argument is passed (mode_t is narrower on
/// macOS).
pub(crate) fn openat(dir: BorrowedFd, name: &CStr, flags: c_int, mode: c_uint) -> io::Result<File> {
    // SAFETY: `dir` is an open descriptor for as long as it is borrowed, and
    // `name` a NUL-terminated string; openat(2) reads only those, and the
    // mode.
    let fd = unsafe { libc::openat(dir.as_raw_fd(), name.as_ptr(), flags | libc::O_CLOEXEC, mode) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: openat(2) returned a new descriptor that nothing else owns.
    Ok(unsafe { File::from_raw_fd(fd) })
}

/// Makes the directory `name` in the directory `dir`, as mkdirat(2) does.
pub(crate) fn mkdirat(dir: BorrowedFd, name: &CStr, mode: mode_t) -> io::Result<()> {
    // SAFETY: as for `openat`: an open descriptor, a NUL-terminated string.
    if unsafe { libc::mkdirat(dir.as_raw_fd(), name.as_ptr(), mode) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Removes the name `name` from the directory `dir`, as unlinkat(2) does
/// with `flags`. A symlink's name is removed, never what it points at.
pub(crate) fn unlinkat(dir: BorrowedFd, name: &CStr, flags: c_int) -> io::Result<()> {
    // SAFETY: as for `openat`: an open descriptor, a NUL-terminated string.
    if unsafe { libc::unlinkat(dir.as_raw_fd(), name.as_ptr(), flags) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
