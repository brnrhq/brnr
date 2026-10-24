//! Session locks (ADR 3.9 in docs/adr). A process holds an exclusive `flock` on
//! `sessions/<id>.lock` in the state directory, beside the transcripts it
//! guards (ADR 3.9), for each session it serves, with its pid, the session's
//! id and its runtime directory written inside: taking a session is taking
//! its lock. The kernel lets go of it when the process ends, however it
//! ends, so nothing has to be cleaned up; a process that closes a session
//! removes the file as it lets go.
//!
//! Whether a lock is held can be seen without asking its process (`holder`,
//! `all`): a process that doesn't answer but holds a session's lock is
//! serving it. Looking takes a shared lock for a moment, so a lock that is
//! held is tried again for a little while before it counts as taken.
//!
//! A process takes one lock at a time ([`TAKING`]): a prune holds a
//! session's lock while it deletes the session's files ([`holding`], ADR
//! 9.5), and a take by another of the process's threads waits for it,
//! rather than find the lock held by its own process.

use std::fs::{self, File, OpenOptions};
use std::io::{self, ErrorKind, Read, Seek, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::thread::sleep;
use std::time::Duration;

use serde_json::{Value, json};

use crate::paths;

/// How many times, and how far apart, a lock that is held is looked at
/// again: someone looking holds it for a moment, and a process that has just
/// taken it writes its pid right after.
const TRIES: u32 = 20;
const PAUSE: Duration = Duration::from_millis(5);

/// Held while a thread of this process takes a lock, and while a prune
/// holds one ([`holding`]): two locks of one process's are never taken at
/// once, so a thread never finds a session's lock held by its own process.
static TAKING: Mutex<()> = Mutex::new(());

/// This process's hold on a session. Dropping it lets go.
pub(crate) struct Lock {
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

pub(crate) enum Error {
    /// Another process holds it: its pid.
    Held(u32),
    /// It can't be taken: why, naming the path. Neither held nor free, and
    /// never taken for either (ADR 3.9).
    Io(String),
}

/// A lock file, what it says, and the process holding it, if one does, with
/// the runtime directory that process was started with.
pub(crate) struct Entry {
    pub(crate) path: PathBuf,
    pub(crate) session: Option<String>,
    pub(crate) pid: Option<u32>,
    pub(crate) dir: Option<PathBuf>,
}

impl Entry {
    /// Whether its holder's runtime directory is another than this
    /// process's: a process whose socket and metadata the commands run here
    /// don't see (ADR 3.9).
    pub(crate) fn elsewhere(&self) -> bool {
        let here = paths::runtime_dir();
        self.pid.is_some() && self.dir.as_deref().is_some_and(|dir| !same_dir(dir, &here))
    }
}

/// Takes `session`'s lock, or says which process holds it.
pub(crate) fn take(session: &str) -> Result<Lock, Error> {
    let _one = TAKING.lock().unwrap_or_else(PoisonError::into_inner);
    take_now(session)
}

/// Takes `session`'s lock, runs `then` while it is held, and lets go: what
/// a prune does with a session (ADR 9.5). No other thread of this process
/// takes a lock meanwhile; one that tries waits until `then` is done, and
/// then takes it as if the prune had not been.
pub(crate) fn holding<T>(session: &str, then: impl FnOnce() -> T) -> Result<T, Error> {
    let _one = TAKING.lock().unwrap_or_else(PoisonError::into_inner);
    let lock = take_now(session)?;
    let done = then();
    drop(lock);
    Ok(done)
}

fn take_now(session: &str) -> Result<Lock, Error> {
    let dir = paths::session_locks();
    paths::ensure_private(&dir).map_err(|e| Error::Io(format!("{}: {e}", dir.display())))?;
    let path = paths::session_lock(session);
    let failed = |e: io::Error| Error::Io(format!("{}: {e}", path.display()));
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
            .map_err(failed)?;
        if !flock(&file, libc::LOCK_EX).map_err(failed)? {
            tries += 1;
            if tries < TRIES {
                sleep(PAUSE);
                continue;
            }
            return match read(&mut file) {
                Some((pid, ..)) => Err(Error::Held(pid)),
                None => Err(failed(io::Error::other("held by a process that doesn't say which"))),
            };
        }
        if !same(&file, &path) {
            continue; // Its holder removed it as it let go.
        }
        // Absolute, so that it names the directory from any cwd.
        let dir = paths::runtime_dir();
        let dir = std::path::absolute(&dir).unwrap_or(dir).to_string_lossy().into_owned();
        let record = json!({ "pid": std::process::id(), "session": session, "dir": dir });
        file.set_len(0)
            .and_then(|()| file.write_all(format!("{record}\n").as_bytes()))
            .map_err(failed)?;
        return Ok(Lock { path, file });
    }
}

/// The pid of the process holding `session`'s lock, if one does.
pub(crate) fn holder(session: &str) -> Option<u32> {
    look(paths::session_lock(session))?.pid
}

/// The runtime directory of the process holding `session`'s lock, when it
/// isn't this process's: the commands run here can't reach that process,
/// and `BRNR_DIR` set to it can (ADR 3.9).
pub(crate) fn holder_dir(session: &str) -> Option<PathBuf> {
    look(paths::session_lock(session)).filter(Entry::elsewhere)?.dir
}

/// Every lock file, held or not.
pub(crate) fn all() -> Vec<Entry> {
    let Ok(dir) = fs::read_dir(paths::session_locks()) else { return Vec::new() };
    dir.flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "lock"))
        .filter_map(look)
        .collect()
}

/// What is where a lock file goes but isn't a file (a directory, a
/// symlink): its session can't be locked (ADR 3.9). Each, and what it is.
pub(crate) fn unusable() -> Vec<(PathBuf, &'static str)> {
    let Ok(dir) = fs::read_dir(paths::session_locks()) else { return Vec::new() };
    let what = |kind: fs::FileType| match () {
        () if kind.is_file() => None,
        () if kind.is_dir() => Some("a directory"),
        () if kind.is_symlink() => Some("a symlink"),
        () => Some("not a file"),
    };
    let mut found: Vec<_> = dir
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|e| e == "lock"))
        .filter_map(|e| Some((e.path(), what(e.file_type().ok()?)?)))
        .collect();
    found.sort();
    found
}

/// Removes a lock file nobody holds, as its holder would have: one left by a
/// process that died.
pub(crate) fn remove(path: &Path) -> bool {
    let Ok(file) = open(path) else { return false };
    matches!(flock(&file, libc::LOCK_EX), Ok(true))
        && same(&file, path)
        && fs::remove_file(path).is_ok()
}

/// What the lock file at `path` says, and who holds it. `None` for one that
/// is gone, or held by a process that doesn't say which.
fn look(path: PathBuf) -> Option<Entry> {
    let mut file = open(&path).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None; // Nobody's lock: see `unusable`.
    }
    for _ in 0..TRIES {
        let said = read(&mut file);
        if flock(&file, libc::LOCK_SH).ok()? {
            // Nobody's: what it says is a process's that has gone.
            let session = said.map(|(_, s, _)| s);
            return Some(Entry { path, session, pid: None, dir: None });
        }
        // Held: by the process it names, once that has written itself in.
        if let Some((pid, session, dir)) = read(&mut file).filter(|(pid, ..)| alive(*pid)) {
            return Some(Entry { path, session: Some(session), pid: Some(pid), dir });
        }
        sleep(PAUSE);
    }
    None
}

/// A lock file, to read: never through a symlink, nor waiting on a FIFO in
/// its place.
fn open(path: &Path) -> io::Result<File> {
    let flags = libc::O_NOFOLLOW | libc::O_NONBLOCK;
    OpenOptions::new().read(true).custom_flags(flags).open(path)
}

/// The pid and session a lock file names, and its holder's runtime
/// directory.
fn read(file: &mut File) -> Option<(u32, String, Option<PathBuf>)> {
    let mut text = String::new();
    file.rewind().ok()?;
    file.read_to_string(&mut text).ok()?;
    let record: Value = serde_json::from_str(&text).ok()?;
    let pid = u32::try_from(record["pid"].as_u64()?).ok()?;
    let dir = record["dir"].as_str().map(PathBuf::from);
    Some((pid, record["session"].as_str()?.to_owned(), dir))
}

/// Whether `a` and `b` name one directory, however each is spelled.
fn same_dir(a: &Path, b: &Path) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
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
