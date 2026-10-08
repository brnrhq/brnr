//! Session locks (ADR 3 in docs/adr). A process holds an exclusive `flock` on
//! `sessions/<id>.lock` in the runtime directory for each session it serves,
//! with its pid and the session's id written inside: taking a session is
//! taking its lock. The kernel lets go of it when the process ends, however
//! it ends, so nothing has to be cleaned up; a process that closes a session
//! removes the file as it lets go.
//!
//! Whether a lock is held can be seen without asking its process (`holder`,
//! `all`): a process that doesn't answer but holds a session's lock is
//! serving it. Looking takes a shared lock for a moment, so a lock that is
//! held is tried again for a little while before it counts as taken.

use std::fs::{self, File, OpenOptions};
use std::io::{self, ErrorKind, Read, Seek, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::thread::sleep;
use std::time::Duration;

use serde_json::{Value, json};

use crate::paths;

/// How many times, and how far apart, a lock that is held is looked at
/// again: someone looking holds it for a moment, and a process that has just
/// taken it writes its pid right after.
const TRIES: u32 = 20;
const PAUSE: Duration = Duration::from_millis(5);

/// This process's hold on a session. Dropping it lets go.
pub struct Lock {
    path: PathBuf,
    file: File,
}

impl Drop for Lock {
    fn drop(&mut self) {
        // Removed while still held: whoever opened it meanwhile finds it
        // gone once they have it, and tries again (see `take`).
        if same(&self.file, &self.path) {
            let _ = fs::remove_file(&self.path);
        }
    }
}

pub enum Error {
    /// Another process holds it: its pid.
    Held(u32),
    Io(io::Error),
}

/// A lock file, what it says, and the process holding it, if one does.
pub struct Entry {
    pub path: PathBuf,
    pub session: Option<String>,
    pub pid: Option<u32>,
}

/// Takes `session`'s lock, or says which process holds it.
pub fn take(session: &str) -> Result<Lock, Error> {
    paths::ensure_private(&paths::session_locks()).map_err(Error::Io)?;
    let path = paths::session_lock(session);
    let mut tries = 0;
    loop {
        // What its holder wrote stays until the lock is this process's.
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&path)
            .map_err(Error::Io)?;
        if !flock(&file, libc::LOCK_EX).map_err(Error::Io)? {
            tries += 1;
            if tries < TRIES {
                sleep(PAUSE);
                continue;
            }
            return match read(&mut file) {
                Some((pid, _)) => Err(Error::Held(pid)),
                None => {
                    Err(Error::Io(io::Error::other("held by a process that doesn't say which")))
                }
            };
        }
        if !same(&file, &path) {
            continue; // Its holder removed it as it let go.
        }
        let record = json!({ "pid": std::process::id(), "session": session });
        file.set_len(0)
            .and_then(|()| file.write_all(format!("{record}\n").as_bytes()))
            .map_err(Error::Io)?;
        return Ok(Lock { path, file });
    }
}

/// The pid of the process holding `session`'s lock, if one does.
pub fn holder(session: &str) -> Option<u32> {
    look(paths::session_lock(session))?.pid
}

/// Every lock file, held or not.
pub fn all() -> Vec<Entry> {
    let Ok(dir) = fs::read_dir(paths::session_locks()) else { return Vec::new() };
    dir.flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "lock"))
        .filter_map(look)
        .collect()
}

/// Removes a lock file nobody holds, as its holder would have: one left by a
/// process that died.
pub fn remove(path: &Path) -> bool {
    let Ok(file) = open(path) else { return false };
    matches!(flock(&file, libc::LOCK_EX), Ok(true))
        && same(&file, path)
        && fs::remove_file(path).is_ok()
}

/// What the lock file at `path` says, and who holds it. `None` for one that
/// is gone, or held by a process that doesn't say which.
fn look(path: PathBuf) -> Option<Entry> {
    let mut file = open(&path).ok()?;
    for _ in 0..TRIES {
        let said = read(&mut file);
        if flock(&file, libc::LOCK_SH).ok()? {
            // Nobody's: what it says is a process's that has gone.
            return Some(Entry { path, session: said.map(|(_, s)| s), pid: None });
        }
        // Held: by the process it names, once that has written itself in.
        if let Some((pid, session)) = read(&mut file).filter(|(pid, _)| alive(*pid)) {
            return Some(Entry { path, session: Some(session), pid: Some(pid) });
        }
        sleep(PAUSE);
    }
    None
}

/// A lock file, to read: never through a symlink.
fn open(path: &Path) -> io::Result<File> {
    OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW).open(path)
}

/// The pid and session a lock file names.
fn read(file: &mut File) -> Option<(u32, String)> {
    let mut text = String::new();
    file.rewind().ok()?;
    file.read_to_string(&mut text).ok()?;
    let record: Value = serde_json::from_str(&text).ok()?;
    let pid = u32::try_from(record["pid"].as_u64()?).ok()?;
    Some((pid, record["session"].as_str()?.to_owned()))
}

/// `flock(op | LOCK_NB)`: false if someone else holds it.
fn flock(file: &File, op: libc::c_int) -> io::Result<bool> {
    loop {
        // SAFETY: flock(2) takes file's descriptor, open while file is
        // borrowed, and touches no memory.
        if unsafe { libc::flock(file.as_raw_fd(), op | libc::LOCK_NB) } == 0 {
            return Ok(true);
        }
        let err = io::Error::last_os_error();
        match err.kind() {
            ErrorKind::Interrupted => continue,
            ErrorKind::WouldBlock => return Ok(false),
            _ => return Err(err),
        }
    }
}

/// Whether `file` is still the one at `path`.
fn same(file: &File, path: &Path) -> bool {
    match (file.metadata(), fs::symlink_metadata(path)) {
        (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
        _ => false,
    }
}

fn alive(pid: u32) -> bool {
    crate::host::alive(pid.into())
}
