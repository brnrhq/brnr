//! Starting the host, from the proxy or from `brnr start`.

use std::env;
use std::ffi::OsStr;
use std::io;
use std::os::fd::RawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{ChildStdin, Command};

/// `brnr host`: the running binary, as the host. It is told the path brnr
/// was started by, so it looks for adapters where this process does (see
/// [`bundled`]).
pub fn host_command() -> io::Result<Command> {
    let mut cmd = Command::new(env::current_exe()?);
    if let Some(started_as) = started_as() {
        cmd.arg0(started_as);
    }
    cmd.arg("host");
    Ok(cmd)
}

/// A bare program name (no `/`) that is installed next to brnr, such as the
/// adapters `brnr-claude-adapter` and `brnr-codex-adapter`. Found there even when that
/// directory isn't on the editor's PATH.
///
/// "Next to brnr" is the running binary's directory, and the directory of
/// the path brnr was started by if that differs: started through a symlink
/// (Homebrew links `bin/brnr` into its prefix's `bin`), Linux reports the
/// link's target as the running binary, while the adapters are linked next
/// to the link.
pub fn bundled(program: &OsStr) -> Option<PathBuf> {
    if program.as_bytes().contains(&b'/') {
        return None;
    }
    install_dirs().into_iter().map(|dir| dir.join(program)).find(|path| path.is_file())
}

fn install_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(dir) = env::current_exe().ok().and_then(|exe| exe.parent().map(PathBuf::from)) {
        dirs.push(dir);
    }
    if let Some(dir) = started_as().and_then(|path| path.parent().map(PathBuf::from))
        && !dirs.contains(&dir)
    {
        dirs.push(dir);
    }
    dirs
}

/// The path brnr was started by (`argv[0]`), made absolute, if it was a
/// path rather than a name looked up on PATH.
fn started_as() -> Option<PathBuf> {
    let arg0 = env::args_os().next()?;
    if !arg0.as_bytes().contains(&b'/') {
        return None;
    }
    std::path::absolute(arg0).ok()
}

/// Starts `cmd` as a grandchild in a new session, so that once the
/// intermediate child exits it is reparented to launchd/init: out of our
/// process group and out from under us in the process tree. `fd` is moved
/// to `target` in the new process, the only descriptor it gets from us
/// besides what `cmd` sets up for stdio. Returns the new process's stdin if
/// `cmd` asked for a pipe.
pub fn detached(cmd: &mut Command, fd: RawFd, target: RawFd) -> io::Result<Option<ChildStdin>> {
    unsafe {
        cmd.pre_exec(move || {
            // dup2 clears FD_CLOEXEC on the copy, except when the fd is
            // already `target` and dup2 does nothing.
            if fd == target {
                libc::fcntl(fd, libc::F_SETFD, 0);
            } else if libc::dup2(fd, target) < 0 {
                return Err(io::Error::last_os_error());
            }
            if libc::setsid() < 0 {
                return Err(io::Error::last_os_error());
            }
            match libc::fork() {
                -1 => Err(io::Error::last_os_error()),
                0 => Ok(()),
                _ => libc::_exit(0),
            }
        })
    };
    // spawn() returns once the grandchild has exec'd (or failed to); the
    // child it hands back is the intermediate, which has already exited.
    let program = PathBuf::from(cmd.get_program());
    let mut intermediate =
        cmd.spawn().map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", program.display())))?;
    let stdin = intermediate.stdin.take();
    intermediate.wait()?;
    Ok(stdin)
}
