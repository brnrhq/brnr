//! Starting the host, from the proxy or from `brnr start`.

use std::env;
use std::ffi::OsStr;
use std::io::{self, Read};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command};

/// `brnr host`: the running binary, as the host. Its argv is only that: the
/// rest, the agent's command among it, goes in the start request (see
/// request.rs), which has the agent and the bridges' commands found already
/// (see [`bundled`]).
pub fn host_command() -> io::Result<Command> {
    let mut cmd = Command::new(env::current_exe()?);
    cmd.arg("host");
    Ok(cmd)
}

/// A bare program name (no `/`) that is installed next to brnr, such as the
/// adapters `brnr-claude-adapter` and `brnr-codex-adapter`, or `brnr` for a
/// bridge. Found there even when that directory isn't on the editor's PATH.
/// Looked for by whoever starts a process, `brnr start` or `brnr acp`, which
/// ran by the path the user gave (ADR 8, ADR 38).
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
/// process group and out from under us in the process tree. Each of `fds`,
/// `(fd, target)`, is moved to `target` in the new process: the only
/// descriptors it gets from us besides what `cmd` sets up for stdio. Returns
/// the new process's stdin if `cmd` asked for a pipe (the host's, for its
/// start request), and its pid.
pub fn detached(
    cmd: &mut Command,
    fds: &[(RawFd, RawFd)],
) -> io::Result<(Option<ChildStdin>, u32)> {
    // The intermediate child writes the new process's pid here as it goes;
    // the new process doesn't keep it past exec.
    let (mut pid_reader, pid_writer) = io::pipe()?;
    let mut pid_fd = pid_writer.as_raw_fd();
    let mut fds = fds.to_vec();
    unsafe {
        cmd.pre_exec(move || {
            move_fds(&mut fds, Some(&mut pid_fd))?;
            if libc::setsid() < 0 {
                return Err(io::Error::last_os_error());
            }
            match libc::fork() {
                -1 => Err(io::Error::last_os_error()),
                0 => Ok(()),
                pid => {
                    let bytes = pid.to_ne_bytes();
                    libc::write(pid_fd, bytes.as_ptr().cast(), bytes.len());
                    libc::_exit(0)
                }
            }
        })
    };
    // spawn() returns once the grandchild has exec'd (or failed to); the
    // child it hands back is the intermediate, which has already exited.
    let program = PathBuf::from(cmd.get_program());
    let spawned = cmd.spawn();
    drop(pid_writer);
    let mut intermediate =
        spawned.map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", program.display())))?;
    let stdin = intermediate.stdin.take();
    intermediate.wait()?;
    let mut pid = [0; size_of::<libc::pid_t>()];
    pid_reader.read_exact(&mut pid)?;
    Ok((stdin, libc::pid_t::from_ne_bytes(pid) as u32))
}

/// Starts `cmd` as our child in a process group of its own, with each of
/// `fds` moved as for [`detached`]: `brnr start --foreground`, which waits
/// for it and passes it the signals it gets, so a terminal's Ctrl-C reaches
/// it once.
pub fn child(cmd: &mut Command, fds: &[(RawFd, RawFd)]) -> io::Result<Child> {
    let mut fds = fds.to_vec();
    unsafe { cmd.pre_exec(move || move_fds(&mut fds, None)) };
    cmd.process_group(0);
    let program = PathBuf::from(cmd.get_program());
    cmd.spawn().map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", program.display())))
}

/// Between fork and exec: moves each `(fd, target)` to `target`, open
/// across exec there, and `keep` out of the targets' way. Each is first
/// copied above every target, so none is overwritten before it has moved,
/// whichever targets the fds are on; the copies close at exec.
fn move_fds(fds: &mut [(RawFd, RawFd)], keep: Option<&mut RawFd>) -> io::Result<()> {
    let above = fds.iter().map(|&(_, target)| target + 1).max().unwrap_or(0);
    let movable = fds.iter_mut().map(|(fd, _)| fd).chain(keep);
    for fd in movable {
        if *fd < above {
            *fd = unsafe { libc::fcntl(*fd, libc::F_DUPFD_CLOEXEC, above) };
            if *fd < 0 {
                return Err(io::Error::last_os_error());
            }
        }
    }
    // dup2 clears FD_CLOEXEC on the copy.
    for &mut (fd, target) in fds {
        if unsafe { libc::dup2(fd, target) } < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}
