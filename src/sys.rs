//! The system calls brnr makes that are sound whatever their arguments, as
//! safe functions. The others are `unsafe` where they are made, each with why
//! it is sound there.

use std::ffi::{CStr, CString};
use std::fs::File;
use std::io;
use std::io::ErrorKind;
use std::mem::ManuallyDrop;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

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
    onto_null(1, File::options().write(true).open("/dev/null")?)
}

/// Ends stdin for whoever writes to it, as closing it would (their next
/// write fails, EPIPE, once no one else holds it): fd 0 is left open on
/// /dev/null, as `end_stdout` leaves fd 1, and what reads it then reads EOF.
pub(crate) fn end_stdin() -> io::Result<()> {
    onto_null(0, File::open("/dev/null")?)
}

/// Puts `null` in the place of stdin or stdout, `fd` 0 or 1.
fn onto_null(fd: RawFd, null: File) -> io::Result<()> {
    loop {
        // SAFETY: dup2(2) takes no pointers; `null` is open, and `fd` is the
        // process's (see `stdio`): what holds it goes on to hold /dev/null.
        if unsafe { libc::dup2(null.as_raw_fd(), fd) } >= 0 {
            return Ok(());
        }
        let err = io::Error::last_os_error();
        if err.kind() != io::ErrorKind::Interrupted {
            return Err(err);
        }
    }
}

/// Waits, for up to `timeout` ms (-1: as long as it takes), until one of
/// `fds` is ready for its events; how many are.
pub(crate) fn poll(fds: &mut [libc::pollfd], timeout: c_int) -> io::Result<usize> {
    // SAFETY: the pointer and the count are the slice's, of valid pollfds;
    // poll(2) writes only their revents. Any fd, events and timeout are
    // valid arguments (a bad fd is POLLNVAL).
    match unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, timeout) } {
        -1 => Err(io::Error::last_os_error()),
        n => Ok(n as usize),
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

/// How long a connection a full backlog turned away waits to be tried again.
const BACKLOG_AGAIN: Duration = Duration::from_millis(20);

/// A connection to the Unix socket at `path`, as `UnixStream::connect`
/// makes one, waiting at most `wait` for its listener to take it: on Linux
/// a listener whose backlog is full makes a connection wait for as long as
/// it stays full (macOS refuses it, `ConnectionRefused`). Past `wait`,
/// `TimedOut`; any other error is the system's, as std gives it. The stream
/// blocks, and isn't inherited by a child, as std's.
pub(crate) fn connect_within(path: &Path, wait: Duration) -> io::Result<UnixStream> {
    let until = Instant::now().checked_add(wait);
    let (addr, len) = sockaddr_un(path)?;
    // SAFETY: socket(2) takes no pointers; any arguments are valid (a bad
    // one is an error).
    let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: socket(2) returned a new descriptor that nothing else owns.
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    // macOS has no SOCK_CLOEXEC or SOCK_NONBLOCK: both are set after.
    set_flag(fd.as_fd(), Flags::Descriptor, libc::FD_CLOEXEC, true)?;
    set_flag(fd.as_fd(), Flags::Status, libc::O_NONBLOCK, true)?;
    loop {
        // SAFETY: `addr` is a sockaddr_un of ours and `len` no more than its
        // size; connect(2) reads only those.
        if unsafe { libc::connect(fd.as_raw_fd(), (&raw const addr).cast(), len) } == 0 {
            break;
        }
        let err = io::Error::last_os_error();
        match err.raw_os_error() {
            // A full backlog, on Linux (EWOULDBLOCK is EAGAIN on both):
            // nothing says when it has room, so it is tried again.
            Some(libc::EAGAIN) => {
                let left =
                    until.map_or(BACKLOG_AGAIN, |u| u.saturating_duration_since(Instant::now()));
                if left.is_zero() {
                    return Err(ErrorKind::TimedOut.into());
                }
                std::thread::sleep(left.min(BACKLOG_AGAIN));
            }
            // Under way: interrupted, a connection goes on by itself.
            Some(libc::EINPROGRESS | libc::EINTR) => {
                connected(fd.as_fd(), until)?;
                break;
            }
            _ => return Err(err),
        }
    }
    set_flag(fd.as_fd(), Flags::Status, libc::O_NONBLOCK, false)?;
    Ok(UnixStream::from(fd))
}

/// The address of the socket at `path`, and its length, as std makes them:
/// a path with a NUL, or too long for `sun_path` with the NUL that ends it,
/// is `InvalidInput`.
fn sockaddr_un(path: &Path) -> io::Result<(libc::sockaddr_un, libc::socklen_t)> {
    // SAFETY: sockaddr_un is plain data, for which all zeros is a valid value.
    let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    addr.sun_family = libc::AF_UNIX as libc::sa_family_t;
    let bytes = path.as_os_str().as_bytes();
    if bytes.contains(&0) {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            "paths must not contain interior null bytes",
        ));
    }
    if bytes.len() >= addr.sun_path.len() {
        return Err(io::Error::new(ErrorKind::InvalidInput, "path must be shorter than SUN_LEN"));
    }
    // c_char is i8 on some targets and u8 on others (see `uname`).
    for (to, &from) in addr.sun_path.iter_mut().zip(bytes) {
        *to = libc::c_char::from_ne_bytes([from]);
    }
    // The NUL that ends the path is counted, as std counts it.
    let len = std::mem::offset_of!(libc::sockaddr_un, sun_path) + bytes.len() + 1;
    Ok((addr, len as libc::socklen_t))
}

/// Waits until `until` (none: for good) for the connection under way on
/// `fd` to be made, and says how it went.
fn connected(fd: BorrowedFd, until: Option<Instant>) -> io::Result<()> {
    loop {
        // In whole milliseconds, rounded up, so the wait isn't cut short.
        let ms = until.map_or(-1, |u| {
            let left = u.saturating_duration_since(Instant::now()).as_nanos().div_ceil(1_000_000);
            c_int::try_from(left).unwrap_or(c_int::MAX)
        });
        let mut poll = libc::pollfd { fd: fd.as_raw_fd(), events: libc::POLLOUT, revents: 0 };
        // SAFETY: poll(2) reads and writes the one pollfd it is given, ours.
        match unsafe { libc::poll(&mut poll, 1, ms) } {
            0 => return Err(ErrorKind::TimedOut.into()),
            n if n > 0 => break,
            _ => {
                let err = io::Error::last_os_error();
                if err.kind() != ErrorKind::Interrupted {
                    return Err(err);
                }
            }
        }
    }
    let mut err: c_int = 0;
    let mut len = size_of::<c_int>() as libc::socklen_t;
    // SAFETY: getsockopt(2) writes at most `len` bytes to `err`, an int of
    // ours `len` long, and how many it wrote to `len`.
    let got = unsafe {
        libc::getsockopt(
            fd.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_ERROR,
            (&raw mut err).cast(),
            &mut len,
        )
    };
    if got != 0 {
        return Err(io::Error::last_os_error());
    }
    if err != 0 {
        return Err(io::Error::from_raw_os_error(err));
    }
    Ok(())
}

/// Which of a descriptor's flags [`set_flag`] sets.
enum Flags {
    /// The descriptor's own (`F_GETFD`, `F_SETFD`).
    Descriptor,
    /// The open file's (`F_GETFL`, `F_SETFL`).
    Status,
}

/// Sets `flag` among `fd`'s `flags`, or clears it.
fn set_flag(fd: BorrowedFd, flags: Flags, flag: c_int, on: bool) -> io::Result<()> {
    let (get, set) = match flags {
        Flags::Descriptor => (libc::F_GETFD, libc::F_SETFD),
        Flags::Status => (libc::F_GETFL, libc::F_SETFL),
    };
    // SAFETY: fcntl(2) with F_GETFD or F_GETFL takes an open descriptor and
    // nothing else, and returns its flags or -1.
    let was = unsafe { libc::fcntl(fd.as_raw_fd(), get) };
    if was < 0 {
        return Err(io::Error::last_os_error());
    }
    let now = if on { was | flag } else { was & !flag };
    // SAFETY: with F_SETFD or F_SETFL it takes the descriptor and an int,
    // any of which is a valid argument (a bad one is an error, or ignored).
    if now != was && unsafe { libc::fcntl(fd.as_raw_fd(), set, now) } < 0 {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::os::unix::net::UnixListener;
    use std::path::PathBuf;

    /// A private directory for test `name`'s socket, and the socket's path.
    fn socket(name: &str) -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("brnr-sys-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("s.sock");
        (dir, path)
    }

    /// The flags `get` (`F_GETFD`, `F_GETFL`) reads of `fd`.
    fn flags(fd: BorrowedFd, get: c_int) -> c_int {
        // SAFETY: fcntl(2) with F_GETFD or F_GETFL takes an open descriptor
        // and nothing else.
        unsafe { libc::fcntl(fd.as_raw_fd(), get) }
    }

    /// A connection to a listener whose backlog is full waits at most its
    /// wait, and fails, rather than waiting until there is room: on Linux
    /// it would wait for good (`TimedOut`); macOS refuses it at once
    /// (ADR 10.4).
    #[test]
    fn adr_0010_a_connection_a_full_backlog_holds_waits_at_most_its_wait() {
        let (dir, path) = socket("full");
        let listener = UnixListener::bind(&path).unwrap();
        // SAFETY: listen(2) takes no pointers; the listener's descriptor is
        // open, and listening again only sets its backlog, to the least.
        assert_eq!(unsafe { libc::listen(listener.as_raw_fd(), 0) }, 0);
        let mut queued = Vec::new();
        let full = loop {
            match connect_within(&path, Duration::ZERO) {
                Ok(stream) => queued.push(stream),
                Err(e) => break e,
            }
            assert!(queued.len() < 1000, "the backlog never filled");
        };
        let wait = Duration::from_millis(300);
        let start = Instant::now();
        let err = connect_within(&path, wait).unwrap_err();
        let took = start.elapsed();
        assert!(took < wait + Duration::from_secs(1), "{took:?}");
        #[cfg(target_os = "linux")]
        {
            assert_eq!((full.kind(), err.kind()), (ErrorKind::TimedOut, ErrorKind::TimedOut));
            assert!(took >= wait, "{took:?}");
            // Room made while it waits, it gets in. The listener is kept
            // until then: closed, it would refuse.
            let accepting = std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(100));
                let accepted = listener.accept().map(drop);
                (listener, accepted)
            });
            connect_within(&path, Duration::from_secs(5)).unwrap();
            accepting.join().unwrap().1.unwrap();
        }
        #[cfg(not(target_os = "linux"))]
        {
            let kinds = (full.kind(), err.kind());
            assert_eq!(kinds, (ErrorKind::ConnectionRefused, ErrorKind::ConnectionRefused));
            drop(listener);
        }
        drop(queued);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A connection taken is a stream that blocks and that a child doesn't
    /// inherit, as std's; one to no socket, or a path too long for one,
    /// fails as std's does.
    #[test]
    fn a_connection_within_a_wait_is_as_stds() {
        let (dir, path) = socket("plain");
        let listener = UnixListener::bind(&path).unwrap();
        let mut stream = connect_within(&path, Duration::from_secs(5)).unwrap();
        assert_eq!(flags(stream.as_fd(), libc::F_GETFL) & libc::O_NONBLOCK, 0);
        assert_ne!(flags(stream.as_fd(), libc::F_GETFD) & libc::FD_CLOEXEC, 0);
        // A read waits for its timeout, as a blocking stream's does.
        let wait = Duration::from_millis(200);
        stream.set_read_timeout(Some(wait)).unwrap();
        let start = Instant::now();
        let err = stream.read(&mut [0; 8]).unwrap_err();
        assert!(matches!(err.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut), "{err}");
        assert!(start.elapsed() >= wait - Duration::from_millis(20), "{:?}", start.elapsed());
        listener.accept().unwrap().0.write_all(b"hi").unwrap();
        let mut said = String::new();
        stream.read_to_string(&mut said).unwrap();
        assert_eq!(said, "hi");

        let missing = connect_within(&dir.join("none.sock"), Duration::from_secs(5));
        assert_eq!(missing.unwrap_err().kind(), ErrorKind::NotFound);
        let long = dir.join("x".repeat(200));
        let too_long = connect_within(&long, Duration::from_secs(5));
        assert_eq!(too_long.unwrap_err().kind(), ErrorKind::InvalidInput);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
