//! Starting the host, from the proxy or from `brnr start`.

use std::env;
use std::ffi::OsStr;
use std::io;
use std::os::fd::RawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;

/// `brnr host`: the running binary, as the host.
pub fn host_command() -> io::Result<Command> {
    let mut cmd = Command::new(env::current_exe()?);
    cmd.arg("host");
    Ok(cmd)
}

/// A bare program name (no `/`) that is installed next to the running
/// binary, such as the bundled adapters `brnr-claude` and `brnr-codex`. Found
/// there even when that directory isn't on the editor's PATH.
pub fn bundled(program: &OsStr) -> Option<PathBuf> {
    if program.as_bytes().contains(&b'/') {
        return None;
    }
    let path = install_dir().ok()?.join(program);
    path.is_file().then_some(path)
}

fn install_dir() -> io::Result<PathBuf> {
    let exe = env::current_exe()?;
    let dir = exe.parent().ok_or_else(|| io::Error::other("no directory for current exe"))?;
    Ok(dir.to_owned())
}

/// Starts `cmd` as a grandchild in a new session, so that once the
/// intermediate child exits it is reparented to launchd/init: out of our
/// process group and out from under us in the process tree. `fd` is moved
/// to `target` in the new process, the only descriptor it gets from us
/// besides what `cmd` sets up for stdio.
pub fn detached(cmd: &mut Command, fd: RawFd, target: RawFd) -> io::Result<()> {
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
    intermediate.wait()?;
    Ok(())
}
