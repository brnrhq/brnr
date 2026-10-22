//! Where things live.
//!
//! - Control sockets and metadata: `$BRNR_DIR`, else
//!   `$XDG_RUNTIME_DIR/brnr`, else `$TMPDIR/brnr-<uid>`. Each host
//!   writes `<id>.sock` and `<id>.json` there; the directory is private to
//!   the user, which is the access control.
//! - Logs: under `$BRNR_HOME`, else `~/.brnr`, laid out like the
//!   agents' own transcripts so they can be joined with them:
//!   `projects/<folder>/<session id>.jsonl` per ACP session, its events,
//!   and `<session id>.acp.jsonl` beside it, its raw ACP, where `<folder>`
//!   is the session's cwd with every non-alphanumeric character turned
//!   into `-` (as `~/.claude/projects` does) and a hash of the cwd, one
//!   folder per cwd (`project_key`, ADR 9.2); and `hosts/<host id>.jsonl`
//!   for what belongs to no session.
//!
//!   Session locks go with the transcripts, `sessions/<session id>.lock`
//!   there (see lock.rs): two processes that write one transcript, however
//!   each found its runtime directory, contend for one lock (ADR 3.9).
//!
//!   A session id in a file name is escaped (`file_name`, ADR 9.2): one name
//!   per id, so two sessions never share a transcript or a lock.
//! - Config: `$BRNR_CONFIG`, else `$XDG_CONFIG_HOME/brnr/config.toml`,
//!   else `~/.config/brnr/config.toml`.
//!
//! How each is found, `file_name` and `project_key` are fixed for 1.x
//! (ADR 3.9).

use std::env;
use std::ffi::OsStr;
use std::fs::{self, DirBuilder, OpenOptions};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use crate::sys;

pub(crate) fn runtime_dir() -> PathBuf {
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
pub(crate) fn ensure_private(dir: &Path) -> io::Result<()> {
    DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    check_private(dir)
}

/// What [`ensure_private`] checks, for a directory that must already exist:
/// what brnr reads there (which processes run, where their sockets are) is
/// only to be trusted if nobody else could have put it there. It is opened
/// without following a symlink and checked through what was opened: the
/// user's own directory, no group or other bits, and on macOS no access
/// control list, which can let others in whatever the mode says. What is in
/// it (sockets, metadata, locks) is guarded by it, so only it is checked
/// (ADR 3.9). One that isn't there is `NotFound`.
pub(crate) fn check_private(dir: &Path) -> io::Result<()> {
    let refused = || io::Error::other("not a private directory owned by this user");
    let private = |m: &fs::Metadata| m.is_dir() && m.uid() == sys::uid() && m.mode() & 0o077 == 0;
    let flags = libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK;
    let opened = match OpenOptions::new().read(true).custom_flags(flags).open(dir) {
        Ok(opened) => opened,
        // The name is looked at only to say why the open failed: a symlink,
        // not a directory, or another user's is refused as such.
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Err(e),
        Err(e) if fs::symlink_metadata(dir).is_ok_and(|m| private(&m)) => return Err(e),
        Err(_) => return Err(refused()),
    };
    if !private(&opened.metadata()?) {
        return Err(refused());
    }
    if sys::has_acl(&opened)? {
        return Err(io::Error::other(acl_refused(dir, true, "")));
    }
    Ok(())
}

/// Why the directory (`dir`) or file at `path` isn't used: it has an access
/// control list (macOS, `sys::has_acl`). `refused` says what brnr won't do,
/// if anything, and then comes the `chmod -N` that removes it, which brnr
/// never runs itself (P4), in brackets, so that what a caller adds after it
/// isn't taken for part of the command.
pub(crate) fn acl_refused(path: &Path, dir: bool, refused: &str) -> String {
    let can = if dir { "reach what is in it" } else { "read or write it" };
    let shell = shell_word(&path.to_string_lossy());
    format!(
        "has an access control list (ls -le), which can let others {can} whatever its \
         mode{refused} (chmod -N {shell} removes it)"
    )
}

pub(crate) fn state_dir() -> PathBuf {
    match env::var_os("BRNR_HOME") {
        Some(dir) => dir.into(),
        None => home().join(".brnr"),
    }
}

pub(crate) fn config_file() -> PathBuf {
    if let Some(file) = env::var_os("BRNR_CONFIG") {
        return file.into();
    }
    let config = match env::var_os("XDG_CONFIG_HOME") {
        Some(dir) => PathBuf::from(dir),
        None => home().join(".config"),
    };
    config.join("brnr").join("config.toml")
}

pub(crate) fn host_log(host_id: &str) -> PathBuf {
    state_dir().join("hosts").join(format!("{host_id}.jsonl"))
}

pub(crate) fn session_log(cwd: &Path, session: &str) -> PathBuf {
    let folder = project_key(cwd);
    state_dir().join("projects").join(folder).join(format!("{}.jsonl", file_name(session)))
}

/// Where the session locks are (see lock.rs): with the transcripts, so
/// that processes that share them share the locks too (ADR 3.9).
pub(crate) fn session_locks() -> PathBuf {
    state_dir().join("sessions")
}

pub(crate) fn session_lock(session: &str) -> PathBuf {
    session_locks().join(format!("{}.lock", file_name(session)))
}

/// A session id as a file name (ADR 9.2): lowercase ASCII letters, digits,
/// `-` and `_` stay, and every other byte of its UTF-8 is `%` and two
/// lowercase hex digits; the empty id is `%`. The agent's text is untrusted
/// (P8): the name has no `/`, no `.` (so it is never `.` or `..`, nor named
/// like another session's raw ACP file), and distinct ids get distinct names,
/// also where the file system ignores case or Unicode normalization.
pub(crate) fn file_name(session: &str) -> String {
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
pub(crate) fn acp_log(events: &Path) -> PathBuf {
    events.with_extension("acp.jsonl")
}

/// Whether `file`, a name in a project folder, is one of the session's whose
/// file name is `name` ([`file_name`]): its events file, its raw ACP, or one
/// of an older brnr's set aside beside them (`<name>.older.jsonl`,
/// `<name>.older-2.acp.jsonl`, …; ADR 5.9). A session's file name has no
/// `.`, so no other session's file is named so.
pub(crate) fn is_sessions_file(name: &str, file: &str) -> bool {
    let rest = file.strip_prefix(name).and_then(|r| r.strip_prefix('.'));
    let Some(rest) = rest.and_then(|r| r.strip_suffix("jsonl")) else { return false };
    let rest = rest.strip_suffix("acp.").unwrap_or(rest);
    let Some(older) = rest.strip_prefix("older") else { return rest.is_empty() };
    match older.strip_suffix('.') {
        Some("") => true,
        Some(n) => n
            .strip_prefix('-')
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())),
        None => false,
    }
}

/// Whether `path`, in a project folder, is a session's events file.
pub(crate) fn is_events_log(path: &Path) -> bool {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    name.ends_with(".jsonl") && !name.ends_with(".acp.jsonl")
}

/// The folder name a session's logs go in, one per cwd (ADR 9.2):
/// `<claude>-<hash>`, `<claude>` the cwd as `~/.claude/projects` names it
/// and `<hash>` the low 32 bits of FNV-1a 64 over its bytes, as 8 lowercase
/// hex digits. The cwd is taken as given, with no trailing `/`
/// ([`without_trailing_slash`]):
/// nothing is looked up on the file system, so the name is the same however
/// the file system changes. Two cwds whose hashes collide share a folder:
/// their sessions stay apart by id, and each record names its cwd.
pub fn project_key(cwd: &Path) -> String {
    let cwd = without_trailing_slash(cwd);
    format!("{}-{:08x}", claude_project_key(cwd), fnv1a(cwd.as_os_str().as_bytes()) as u32)
}

/// `cwd` as `~/.claude/projects` names its folder: every character other
/// than ASCII letters and digits replaced by `-`. It was the whole folder
/// name before ADR 9.2's hash; a transcript of then is an older brnr's,
/// which isn't continued (ADR 5.9).
pub fn claude_project_key(cwd: &Path) -> String {
    cwd.to_string_lossy().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
}

/// `cwd` without its trailing `/`s, but `/` itself as it is: the cwd a
/// folder is named for.
pub(crate) fn without_trailing_slash(cwd: &Path) -> &Path {
    let bytes = cwd.as_os_str().as_bytes();
    let end = bytes.iter().rposition(|&b| b != b'/').map_or(bytes.len().min(1), |i| i + 1);
    Path::new(OsStr::from_bytes(&bytes[..end]))
}

/// FNV-1a 64 of `bytes`. Written here, not `DefaultHasher`, whose output
/// may change with Rust: a folder's name never may within 1.x (ADR 8.4).
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        hash = (hash ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// `BRNR_DIR=<dir>` as a shell reads it, `dir` one word whatever is in it:
/// the assignment that reaches a process started with runtime directory
/// `dir`, in a command shown to be run (ADR 3.9).
pub(crate) fn brnr_dir_is(dir: &Path) -> String {
    format!("BRNR_DIR={}", shell_word(&dir.to_string_lossy()))
}

/// `word` as one shell word: as it is when nothing in it is special to a
/// shell, else in single quotes, with each `'` in it written `'\''`.
pub(crate) fn shell_word(word: &str) -> String {
    let plain = |c: char| c.is_ascii_alphanumeric() || "_-.,/:=+@%".contains(c);
    if !word.is_empty() && word.chars().all(plain) {
        return word.to_owned();
    }
    format!("'{}'", word.replace('\'', r"'\''"))
}

/// Expands a leading `~/`.
pub(crate) fn expand(path: &str) -> PathBuf {
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
    use std::os::unix::fs::{PermissionsExt, symlink};

    use super::*;

    /// A private directory is checked through what was opened (ADR 3.9):
    /// one that isn't there is `NotFound`, as the commands take it for no
    /// processes; a symlink to a private one, a file, and one others can
    /// search are refused; on macOS, so is one with an access control list,
    /// which is left there.
    #[test]
    fn adr_0003_a_private_directory_is_checked_through_what_was_opened() {
        let dir = env::temp_dir().join(format!("brnr-paths-{}-private", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let (private, link, file) = (dir.join("private"), dir.join("link"), dir.join("file"));
        let err = check_private(&private).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound, "{err}");
        ensure_private(&private).unwrap();
        check_private(&private).unwrap();
        symlink(&private, &link).unwrap();
        fs::write(&file, "").unwrap();
        for refused in [&link, &file] {
            let err = check_private(refused).unwrap_err().to_string();
            assert_eq!(err, "not a private directory owned by this user", "{}", refused.display());
        }
        fs::set_permissions(&private, fs::Permissions::from_mode(0o711)).unwrap();
        assert!(check_private(&private).is_err());
        fs::set_permissions(&private, fs::Permissions::from_mode(0o700)).unwrap();
        #[cfg(target_os = "macos")]
        {
            let chmod = |args: &[&str]| {
                let ok = std::process::Command::new("chmod").args(args).arg(&private).status();
                assert!(ok.unwrap().success(), "chmod {args:?}");
            };
            chmod(&["+a", "everyone allow list"]);
            let err = check_private(&private).unwrap_err().to_string();
            let want = format!("whatever its mode (chmod -N {} removes it)", private.display());
            assert!(err.starts_with("has an access control list (ls -le)"), "{err}");
            assert!(err.ends_with(&want), "{err}");
            assert!(check_private(&private).is_err(), "the ACL is gone");
            chmod(&["-N"]);
            check_private(&private).unwrap();
        }
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn adr_0003_brnr_dir_is_one_shell_word() {
        let is = |dir: &str| brnr_dir_is(Path::new(dir));
        assert_eq!(is("/run/user/1000/brnr"), "BRNR_DIR=/run/user/1000/brnr");
        assert_eq!(is("/tmp/my run"), "BRNR_DIR='/tmp/my run'");
        assert_eq!(is("/tmp/it's"), r"BRNR_DIR='/tmp/it'\''s'");
        assert_eq!(is("/tmp/$x;`y`*"), "BRNR_DIR='/tmp/$x;`y`*'");
        assert_eq!(is("~/run"), "BRNR_DIR='~/run'");
        assert_eq!(is(""), "BRNR_DIR=''");
    }

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

    /// The folder names, written down once: the rule is fixed for 1.x
    /// (ADR 3.9), so these never change.
    #[test]
    fn adr_0009_a_cwd_has_a_folder_of_its_own() {
        let key = |cwd: &str| project_key(Path::new(cwd));
        assert_eq!(key("/"), "--860189fe");
        assert_eq!(key("/a-b"), "-a-b-b38a6786");
        assert_eq!(key("/a/b"), "-a-b-b38ee0cc");
        assert_eq!(key("/A"), "-A-b49d0f8d");
        assert_eq!(key("/a"), "-a-b49cd92d");
        assert_eq!(key("/Users/me/src"), "-Users-me-src-3db3a304");
        // A trailing `/` is the same cwd.
        assert_eq!(key("/a/b/"), key("/a/b"));
        assert_eq!(key("/a/b//"), key("/a/b"));
        assert_eq!(key("//"), key("/"));
        // Alike as `~/.claude/projects` names them, apart here; `/A` and
        // `/a` too, where the file system ignores case.
        assert_ne!(key("/a-b").to_lowercase(), key("/a/b").to_lowercase());
        assert_ne!(key("/A").to_lowercase(), key("/a").to_lowercase());
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

    /// A session's files are its own two and an older brnr's set aside
    /// beside them; never another session's (ADR 5.9, ADR 8.1).
    #[test]
    fn adr_0008_a_sessions_files_are_its_own_and_those_set_aside() {
        let ours = [
            "s.jsonl",
            "s.acp.jsonl",
            "s.older.jsonl",
            "s.older.acp.jsonl",
            "s.older-2.jsonl",
            "s.older-12.acp.jsonl",
        ];
        for file in ours {
            assert!(is_sessions_file("s", file), "{file}");
        }
        let others = [
            "s1.jsonl",
            "s-1.older.jsonl",
            "s.lock",
            "s.jsonl.bak",
            "s.older-.jsonl",
            "s.older-x.jsonl",
            "s.other.jsonl",
            "s.acp.older.jsonl",
            "s..jsonl",
            "x.s.jsonl",
        ];
        for file in others {
            assert!(!is_sessions_file("s", file), "{file}");
        }
    }

    #[test]
    fn adr_0009_distinct_ids_get_distinct_names() {
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
