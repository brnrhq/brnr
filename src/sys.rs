//! The system calls brnr makes that are sound whatever their arguments, as
//! safe functions. The others are `unsafe` where they are made, each with why
//! it is sound there.

use std::ffi::{CStr, CString};
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

/// Whether `file` has an extended ACL. On macOS one grants access its mode
/// doesn't show (`ls -le`); on Linux an ACL's grants show in the mode's
/// group bits, its mask, so there is none to look for apart.
#[cfg(target_os = "macos")]
pub(crate) fn has_acl(file: &File) -> io::Result<bool> {
    // libSystem's, from <sys/acl.h>; the libc crate doesn't have them.
    unsafe extern "C" {
        fn acl_get_fd_np(fd: c_int, kind: c_int) -> *mut libc::c_void;
        fn acl_free(acl: *mut libc::c_void) -> c_int;
    }
    const ACL_TYPE_EXTENDED: c_int = 0x0000_0100;
    // SAFETY: acl_get_fd_np takes a descriptor and a type, any of which is
    // a valid argument (a bad one is an error), and returns memory of its
    // own or null.
    let acl = unsafe { acl_get_fd_np(file.as_raw_fd(), ACL_TYPE_EXTENDED) };
    if acl.is_null() {
        let e = io::Error::last_os_error();
        return if e.raw_os_error() == Some(libc::ENOENT) { Ok(false) } else { Err(e) };
    }
    // SAFETY: `acl` is what acl_get_fd_np allocated, freed once, here.
    unsafe { acl_free(acl) };
    Ok(true)
}

/// Whether `file` has an extended ACL. On macOS one grants access its mode
/// doesn't show (`ls -le`); on Linux an ACL's grants show in the mode's
/// group bits, its mask, so there is none to look for apart.
#[cfg(not(target_os = "macos"))]
pub(crate) fn has_acl(_file: &File) -> io::Result<bool> {
    Ok(false)
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

/// The names in the directory `dir` is open on, but `.` and `..`, all of
/// them each time, read through the descriptor (fdopendir(3) of a
/// duplicate), not by a path that could name another directory by now.
pub(crate) fn names(dir: &File) -> io::Result<Vec<CString>> {
    // SAFETY: fcntl(2) with F_DUPFD_CLOEXEC takes an open descriptor and a
    // number, and returns a new descriptor or -1.
    let fd = unsafe { libc::fcntl(dir.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `fd` is a descriptor of ours that nothing else holds; on
    // success the DIR owns it, and closedir closes it.
    let stream = unsafe { libc::fdopendir(fd) };
    if stream.is_null() {
        let e = io::Error::last_os_error();
        // SAFETY: fdopendir failed, so `fd` is still ours alone to close.
        unsafe { libc::close(fd) };
        return Err(e);
    }
    // The duplicate shares the offset of `dir`, which an earlier listing
    // left at the end.
    // SAFETY: `stream` is open.
    unsafe { libc::rewinddir(stream) };
    let mut names = Vec::new();
    let result = loop {
        // SAFETY: errno is this thread's; readdir(3) sets it only on an
        // error, so it is cleared to tell one from the end.
        unsafe { *errno() = 0 };
        // SAFETY: `stream` is open until closedir below; the entry readdir
        // returns is valid until the next call on it, and its name is
        // copied before then.
        let entry = unsafe { libc::readdir(stream) };
        if entry.is_null() {
            // SAFETY: as above.
            let e = unsafe { *errno() };
            break if e == 0 { Ok(()) } else { Err(io::Error::from_raw_os_error(e)) };
        }
        // SAFETY: d_name is a NUL-terminated string within the entry.
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
        if !matches!(name.to_bytes(), b"." | b"..") {
            names.push(name.to_owned());
        }
    };
    // SAFETY: `stream` is open, and closed once, here.
    unsafe { libc::closedir(stream) };
    result.map(|()| names)
}

/// This thread's errno.
#[cfg(target_os = "macos")]
fn errno() -> *mut c_int {
    // SAFETY: __error(3) takes nothing and returns this thread's errno.
    unsafe { libc::__error() }
}

/// This thread's errno.
#[cfg(not(target_os = "macos"))]
fn errno() -> *mut c_int {
    // SAFETY: __errno_location takes nothing and returns this thread's errno.
    unsafe { libc::__errno_location() }
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
