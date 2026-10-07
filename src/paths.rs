//! Where things live.
//!
//! - Control sockets and metadata: `$BRNR_DIR`, else
//!   `$XDG_RUNTIME_DIR/brnr`, else `$TMPDIR/brnr-<uid>`. Each host
//!   writes `<id>.sock` and `<id>.json` there, and holds
//!   `sessions/<session id>.lock` for each session it serves (see lock.rs);
//!   the directory is private to the user, which is the access control.
//! - Logs: under `$BRNR_HOME`, else `~/.brnr`, laid out like the
//!   agents' own transcripts so they can be joined with them:
//!   `projects/<folder>/<session id>.jsonl` per ACP session, where
//!   `<folder>` is the session's cwd with every non-alphanumeric character
//!   turned into `-` (as `~/.claude/projects` does), and
//!   `hosts/<host id>.jsonl` for what belongs to no session.
//! - Config: `$BRNR_CONFIG`, else `$XDG_CONFIG_HOME/brnr/config.toml`,
//!   else `~/.config/brnr/config.toml`.

use std::env;
use std::fs::{self, DirBuilder};
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::{Path, PathBuf};

pub fn runtime_dir() -> PathBuf {
    if let Some(dir) = env::var_os("BRNR_DIR") {
        return dir.into();
    }
    if let Some(dir) = env::var_os("XDG_RUNTIME_DIR") {
        return PathBuf::from(dir).join("brnr");
    }
    env::temp_dir().join(format!("brnr-{}", unsafe { libc::getuid() }))
}

/// Creates `dir` (mode 0700) if needed and refuses one that someone else
/// owns or others can access, or that is a symlink (which could point at any
/// private directory of ours).
pub fn ensure_private(dir: &Path) -> io::Result<()> {
    DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    check_private(dir)
}

/// What [`ensure_private`] checks, for a directory that must already exist:
/// what brnr reads there (which processes run, where their sockets are) is
/// only to be trusted if nobody else could have put it there.
pub fn check_private(dir: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(dir)?;
    if !meta.is_dir() || meta.uid() != unsafe { libc::getuid() } || meta.mode() & 0o077 != 0 {
        return Err(io::Error::other("not a private directory owned by this user"));
    }
    Ok(())
}

pub fn state_dir() -> PathBuf {
    match env::var_os("BRNR_HOME") {
        Some(dir) => dir.into(),
        None => home().join(".brnr"),
    }
}

pub fn config_file() -> PathBuf {
    if let Some(file) = env::var_os("BRNR_CONFIG") {
        return file.into();
    }
    let config = match env::var_os("XDG_CONFIG_HOME") {
        Some(dir) => PathBuf::from(dir),
        None => home().join(".config"),
    };
    config.join("brnr").join("config.toml")
}

pub fn host_log(host_id: &str) -> PathBuf {
    state_dir().join("hosts").join(format!("{host_id}.jsonl"))
}

pub fn session_log(cwd: &Path, session: &str) -> PathBuf {
    let name = file_name(session);
    state_dir().join("projects").join(project_key(cwd)).join(format!("{name}.jsonl"))
}

/// Where the session locks are (see lock.rs).
pub fn session_locks() -> PathBuf {
    runtime_dir().join("sessions")
}

pub fn session_lock(session: &str) -> PathBuf {
    session_locks().join(format!("{}.lock", file_name(session)))
}

/// A session id as a file name: anything but ASCII letters, digits, `-`,
/// `_` and `.` becomes `_` (the agent's text is untrusted).
fn file_name(session: &str) -> String {
    session
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || "-_.".contains(c) { c } else { '_' })
        .collect()
}

/// The folder name a project's logs go in: its path with every character
/// other than ASCII letters and digits replaced by `-`.
pub fn project_key(cwd: &Path) -> String {
    cwd.to_string_lossy().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
}

/// Expands a leading `~/`.
pub fn expand(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => home().join(rest),
        None if path == "~" => home(),
        None => path.into(),
    }
}

fn home() -> PathBuf {
    env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| "/".into())
}
