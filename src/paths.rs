//! Where things live.
//!
//! - Control sockets and metadata: `$BRNR_DIR`, else
//!   `$XDG_RUNTIME_DIR/brnr`, else `$TMPDIR/brnr-<uid>`. Each host
//!   writes `<id>.sock` and `<id>.json` there, and holds
//!   `sessions/<session id>.lock` for each session it serves (see lock.rs);
//!   the directory is private to the user, which is the access control.
//! - Logs: under `$BRNR_HOME`, else `~/.brnr`, laid out like the
//!   agents' own transcripts so they can be joined with them:
//!   `projects/<folder>/<session id>.jsonl` per ACP session, its events,
//!   and `<session id>.acp.jsonl` beside it, its raw ACP, where `<folder>`
//!   is the session's cwd with every non-alphanumeric character turned
//!   into `-` (as `~/.claude/projects` does); and `hosts/<host id>.jsonl`
//!   for what belongs to no session.
//!
//!   A session id in a file name is escaped (`file_name`, ADR 53): one name
//!   per id, so two sessions never share a transcript or a lock.
//! - Config: `$BRNR_CONFIG`, else `$XDG_CONFIG_HOME/brnr/config.toml`,
//!   else `~/.config/brnr/config.toml`.

use std::env;
use std::fs::{self, DirBuilder};
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::{Path, PathBuf};

use crate::sys;

pub fn runtime_dir() -> PathBuf {
    if let Some(dir) = env::var_os("BRNR_DIR") {
        return dir.into();
    }
    if let Some(dir) = env::var_os("XDG_RUNTIME_DIR") {
        return PathBuf::from(dir).join("brnr");
    }
    env::temp_dir().join(format!("brnr-{}", sys::uid()))
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
    if !meta.is_dir() || meta.uid() != sys::uid() || meta.mode() & 0o077 != 0 {
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
    state_dir()
        .join("projects")
        .join(project_key(cwd))
        .join(format!("{}.jsonl", file_name(session)))
}

/// Where the session locks are (see lock.rs).
pub fn session_locks() -> PathBuf {
    runtime_dir().join("sessions")
}

pub fn session_lock(session: &str) -> PathBuf {
    session_locks().join(format!("{}.lock", file_name(session)))
}

/// A session id as a file name (ADR 53): lowercase ASCII letters, digits,
/// `-` and `_` stay, and every other byte of its UTF-8 is `%` and two
/// lowercase hex digits; the empty id is `%`. The agent's text is untrusted
/// (P8): the name has no `/`, no `.` (so it is never `.` or `..`, nor named
/// like another session's raw ACP file), and distinct ids get distinct names,
/// also where the file system ignores case or Unicode normalization.
fn file_name(session: &str) -> String {
    if session.is_empty() {
        return "%".into();
    }
    let mut name = String::with_capacity(session.len());
    for b in session.bytes() {
        match b {
            b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' => name.push(b as char),
            _ => name.push_str(&format!("%{b:02x}")),
        }
    }
    name
}

/// The raw ACP file beside a session's events file (`session_log`).
pub fn acp_log(events: &Path) -> PathBuf {
    events.with_extension("acp.jsonl")
}

/// Whether `path`, in a project folder, is a session's events file.
pub fn is_events_log(path: &Path) -> bool {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    name.ends_with(".jsonl") && !name.ends_with(".acp.jsonl")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sessions_two_files() {
        let name = |session: &str| {
            let events = session_log(Path::new("/w"), session);
            let acp = acp_log(&events);
            assert!(is_events_log(&events) && !is_events_log(&acp), "{session}");
            let file = |p: &Path| p.file_name().unwrap().to_string_lossy().into_owned();
            (file(&events), file(&acp))
        };
        assert_eq!(name("s-1"), ("s-1.jsonl".into(), "s-1.acp.jsonl".into()));
        assert_eq!(name("a/b"), ("a%2fb.jsonl".into(), "a%2fb.acp.jsonl".into()));
        // Not session `x`'s raw file.
        assert_eq!(name("x.acp"), ("x%2eacp.jsonl".into(), "x%2eacp.acp.jsonl".into()));
    }

    /// Ids that the former `_` for anything unsafe, or a file system that
    /// ignores case or normalization, would have put in one file.
    const COLLIDING: &[&str] = &[
        "a/b",
        "a_b",
        "a%2fb",
        "a%2Fb",
        "a.b",
        "x",
        "x.acp",
        "x_acp",
        "x%2eacp",
        "Sess-1",
        "sess-1",
        "SESS-1",
        "\u{e9}",
        "e\u{301}",
        "_",
        "",
        "%",
        ".",
        "..",
        "../x",
        "\u{1f600}",
        "\u{1f601}",
    ];

    #[test]
    fn adr_0053_distinct_ids_get_distinct_names() {
        let dir = session_log(Path::new("/w"), "s").parent().unwrap().to_owned();
        let mut seen = std::collections::HashMap::new();
        for id in COLLIDING {
            let events = session_log(Path::new("/w"), id);
            let lock = session_lock(id);
            assert_eq!(events.parent(), Some(dir.as_path()), "{id:?}");
            assert_eq!(lock.parent(), Some(session_locks().as_path()), "{id:?}");
            assert_eq!(acp_log(&events).parent(), Some(dir.as_path()), "{id:?}");
            assert!(is_events_log(&events) && !is_events_log(&acp_log(&events)), "{id:?}");
            assert!(lock.extension().is_some_and(|e| e == "lock"), "{id:?}");
            // Folded as a case-insensitive file system would.
            let name = file_name(id).to_lowercase();
            assert!(!name.contains(['/', '.']), "{id:?}: {name}");
            if let Some(other) = seen.insert(name.clone(), id) {
                panic!("{other:?} and {id:?} are both {name}");
            }
        }
        // No events file is another session's raw ACP.
        for (a, b) in COLLIDING.iter().flat_map(|a| COLLIDING.iter().map(move |b| (a, b))) {
            assert_ne!(session_log(Path::new("/w"), a), acp_log(&session_log(Path::new("/w"), b)));
        }
    }
}
