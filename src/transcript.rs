//! Reading transcripts (ADR 5.1 in docs/adr), as log.rs writes them: where
//! they are, a record as brnr reads it, the first and last records of a
//! file, and a record as the event `event watch` would have shown.
//!
//! A session's events file is `projects/<folder>/<name>.jsonl` under the
//! state directory, its raw ACP `<name>.acp.jsonl` beside it, and a host
//! log `hosts/<run id>.jsonl` (paths.rs). A transcript is read only where
//! the logger could have written it (ADR 9.3), by its rules for every part
//! of the path ([`open`]): nothing is looked in or read through a symlink,
//! the state directory, `projects`, `hosts` and a project folder included,
//! nor is what isn't the user's own directory or regular file, or has
//! another hard link or an access control list. What is written there is
//! refused anyway; read, it would be whatever the link points at.
//!
//! A record is a line that is a JSON object: `{"ts","host_id","pid",
//! "proxy_pid","agent_pid"[,"session"],…}`, then `"event":{…}` for a
//! host event, or `["seq",]"dir"` and `"msg"` (`"raw"` for a line that
//! wasn't JSON) for an ACP message. It is read as brnr reads what it wrote
//! (ADR 2.5): a lone surrogate is U+FFFD, and a record nests up to
//! `json::OWN_DEPTH` (json.rs). A line that isn't a record, or nests deeper,
//! is passed over: cut short by a
//! process that died mid-write, at a file's end or before what another
//! process wrote, or broken otherwise. A line cut short is never taken for
//! a record: an object ends only at its last byte.
//!
//! A record an older brnr wrote, its head naming the process `host_pid` and
//! the session `session_id`, isn't read (ADR 5.9, P9): its names are not
//! this brnr's. A file of them is no transcript to `session list`, `session
//! resume` or a load; `event log` and `doctor` say an older brnr wrote it
//! (P3).

use std::ffi::{CString, OsStr};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, ErrorKind};
use std::mem::{replace, take};
use std::os::fd::AsFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::{json, paths, render, sys};

/// How much of a file a read backwards takes at a time.
const CHUNK: u64 = 64 * 1024;

/// Every session's events file under the state directory.
pub(crate) fn events_files() -> Vec<PathBuf> {
    events_files_in(&paths::state_dir())
}

fn events_files_in(root: &Path) -> Vec<PathBuf> {
    let entries = folders(root).into_iter().flat_map(|f| fs::read_dir(f).into_iter().flatten());
    let files = entries.flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_file()));
    files.map(|e| e.path()).filter(|p| paths::is_events_log(p)).collect()
}

/// `session`'s events files under the state directory: in each project
/// folder, the one named for its id (ADR 9.2), where there is one. A session
/// resumed in another folder has one in each.
pub(crate) fn of_session(session: &str) -> Vec<PathBuf> {
    of_session_in(&paths::state_dir(), session)
}

fn of_session_in(root: &Path, session: &str) -> Vec<PathBuf> {
    let name = format!("{}.jsonl", paths::file_name(session));
    let files = folders(root).into_iter().map(|folder| folder.join(&name));
    files.filter(|path| fs::symlink_metadata(path).is_ok_and(|m| m.is_file())).collect()
}

/// `session`'s transcript: of its events files in every project folder
/// ([`of_session`]), those whose records name it, as brnr finds a
/// transcript by the id its records carry (ADR 9.2), oldest first, by when
/// each one's first record was recorded. Not one an older brnr wrote
/// ([`head`]). An id is one session, whatever folders its files are in
/// (ADR 5.1).
pub(crate) fn files_of(session: &str) -> Vec<PathBuf> {
    files_of_in(&paths::state_dir(), session)
}

fn files_of_in(root: &Path, session: &str) -> Vec<PathBuf> {
    let firsts =
        of_session_in(root, session).into_iter().filter_map(|path| Some((first(&path)?, path)));
    let mut found: Vec<(Value, PathBuf)> =
        firsts.filter(|(r, _)| r["session"] == session).collect();
    found.sort_by(|(a, _), (b, _)| a["ts"].as_str().cmp(&b["ts"].as_str()));
    found.into_iter().map(|(_, path)| path).collect()
}

/// Whether brnr has a transcript of `session`, in any project folder: an
/// events file of it holding a record beyond its openings
/// ([`has_records`], ADR 5.6).
pub(crate) fn kept(session: &str) -> bool {
    of_session(session).iter().any(|path| has_records(path))
}

/// The host logs under the state directory (`hosts/<run id>.jsonl`), in
/// order.
pub(crate) fn host_logs() -> Vec<PathBuf> {
    let mut found = entries(&paths::state_dir(), "hosts", |t| t.is_file());
    found.retain(|p| p.extension().is_some_and(|e| e == "jsonl"));
    found
}

/// The project folders under `root`, a state directory, in order; not one
/// that is a symlink.
fn folders(root: &Path) -> Vec<PathBuf> {
    entries(root, "projects", |t| t.is_dir())
}

/// What is in `root`'s `sub` that is `kind`, in order (none that is a
/// symlink): none if `root` or `sub` isn't a directory of the user's, as
/// the logger would refuse to write there ([`walk`]). The names are listed
/// by path; what is then read is opened from `root` again ([`open`]).
fn entries(root: &Path, sub: &str, kind: fn(&fs::FileType) -> bool) -> Vec<PathBuf> {
    if walk(root, &[OsStr::new(sub)], false).is_err() {
        return Vec::new();
    }
    let entries = fs::read_dir(root.join(sub)).into_iter().flatten().flatten();
    let mut found: Vec<PathBuf> =
        entries.filter(|e| e.file_type().is_ok_and(|t| kind(&t))).map(|e| e.path()).collect();
    found.sort();
    found
}

/// Opens the transcript at `path` to read it, by the logger's rules for
/// writing it (ADR 9.3, `log::open_private`): from the state directory it
/// is in (above `hosts/<file>` or `projects/<folder>/<file>`; for a file
/// laid out otherwise, its folder), each part opened from the one before
/// without following a symlink, and checked through what was opened. A
/// symlink, one that isn't a directory or regular file, one of another
/// user's, one with an access control list, or a file with another hard
/// link is refused; a FIFO is refused, not waited on. Nothing is made
/// private, and no access control list removed: reading changes nothing.
pub(crate) fn open(path: &Path) -> io::Result<File> {
    let up = |n| path.ancestors().nth(n).filter(|p| !p.as_os_str().is_empty());
    let named = |n, name: &str| up(n).and_then(Path::file_name).is_some_and(|f| f == name);
    // A project folder is a cwd's key, never `hosts` (paths::project_key).
    let n = match (named(1, "hosts"), named(2, "projects")) {
        (true, _) => 2,
        (_, true) => 3,
        _ => 1,
    };
    let root = up(n).unwrap_or(Path::new("."));
    let names: Vec<&OsStr> = path.strip_prefix(root).unwrap_or(path).iter().collect();
    walk(root, &names, true)
}

/// Opens `names` below the directory `root`, each from the one before
/// (`openat`, `O_NOFOLLOW`), checking each through what was opened (so what
/// is checked is what is read): `root` and each but the last a directory,
/// and the last a regular file with no other name if `file`, all the
/// user's. An error names the part it is about, but for the last, which the
/// caller names as the path it opened.
fn walk(root: &Path, names: &[&OsStr], file: bool) -> io::Result<File> {
    let dirs = libc::O_DIRECTORY | libc::O_NOFOLLOW;
    let opened = OpenOptions::new().read(true).custom_flags(dirs).open(root);
    let mut here = root.to_owned();
    let mut at = check(opened, &here, false, names.is_empty())?;
    for (i, name) in names.iter().enumerate() {
        here.push(name);
        let last = i + 1 == names.len();
        let Ok(c_name) = CString::new(name.as_bytes()) else {
            return Err(refused(&here, last, "is not a plain path"));
        };
        if matches!(name.to_str(), Some("." | "..")) {
            return Err(refused(&here, last, "is not a plain path"));
        }
        let flags = match last && file {
            true => libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK,
            false => libc::O_RDONLY | dirs,
        };
        at = check(sys::openat(at.as_fd(), &c_name, flags, 0), &here, last && file, last)?;
    }
    Ok(at)
}

/// `opened`, the part of a transcript's path at `path`, if it is what
/// [`walk`] takes: a regular file with no other name (`file`) or a
/// directory, the user's, with no access control list.
fn check(opened: io::Result<File>, path: &Path, file: bool, last: bool) -> io::Result<File> {
    // A symlink is ELOOP, or ENOTDIR on macOS for a directory; what it is is
    // looked at only for the message.
    let link = || fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink());
    let opened = opened.map_err(|e| match e.raw_os_error() {
        Some(libc::ELOOP | libc::ENOTDIR) if link() => refused(path, last, "is a symlink"),
        Some(libc::ENOTDIR) => refused(path, last, "is not a directory"),
        _ if last => e,
        _ => io::Error::new(e.kind(), format!("{}: {e}", path.display())),
    })?;
    let meta = opened.metadata()?;
    if file && !meta.is_file() {
        return Err(refused(path, last, "is not a regular file"));
    } else if !file && !meta.is_dir() {
        return Err(refused(path, last, "is not a directory"));
    } else if meta.uid() != sys::uid() {
        return Err(refused(path, last, &format!("is owned by uid {}", meta.uid())));
    } else if file && meta.nlink() != 1 {
        // Another name for it could be anywhere: it would be read as a
        // transcript of the user's.
        return Err(refused(path, last, &format!("has {} hard links", meta.nlink())));
    } else if sys::has_acl(&opened)? {
        // macOS: others may have written it, or read it, whatever its mode.
        return Err(refused(path, last, &paths::acl_refused(path, !file, "")));
    }
    Ok(opened)
}

/// Why the part of a transcript's path at `path` isn't read: of the path
/// opened, if it is the `last` part, else naming the part.
fn refused(path: &Path, last: bool, why: &str) -> io::Error {
    let why = if last { why.to_owned() } else { format!("{} {why}", path.display()) };
    io::Error::new(ErrorKind::InvalidInput, why)
}

/// `line` as a record, if it is one.
pub(crate) fn record(line: &[u8]) -> Option<Value> {
    json::parse::<Value>(line, json::OWN_DEPTH).filter(Value::is_object)
}

/// Whether `record` is one an older brnr wrote (ADR 5.9): its head has
/// `host_pid` or `session_id`, where this brnr's has `pid` and `session`.
pub(crate) fn older(record: &Value) -> bool {
    record.get("host_pid").is_some() || record.get("session_id").is_some()
}

/// Whether the file at `path` is one an older brnr wrote: its first record
/// is ([`older`]).
pub(crate) fn by_an_older_brnr(path: &Path) -> bool {
    let Ok(file) = open(path) else { return false };
    let mut lines = BufReader::new(file).split(b'\n').map_while(Result::ok);
    lines.find_map(|line| record(&line)).is_some_and(|r| older(&r))
}

/// `line` as a record of this brnr's, if it is one: none for an older
/// brnr's ([`older`]).
pub(crate) fn head(line: &[u8]) -> Option<Value> {
    record(line).filter(|r| !older(r))
}

/// `record` as the event `event watch` would have shown (ADR 5.1): a host
/// event as the host emitted it (see host/control.rs, `emit`), or an ACP
/// message as an `acp` event, with its position where the record has one
/// (ADR 10.2: the copy of a `session/new` in a session's raw file has none).
/// None for a record that is neither.
pub(crate) fn event(mut record: Value) -> Option<Value> {
    if record["event"]["event"].is_string() {
        return Some(record["event"].take());
    }
    let key = if record.get("msg").is_some() { "msg" } else { "raw" };
    let msg = record.get_mut(key)?.take();
    let mut event = Map::new();
    event.insert("event".into(), "acp".into());
    event.insert("dir".into(), record["dir"].take());
    event.insert("session".into(), record["session"].take());
    event.insert("msg".into(), msg);
    event.insert("ts".into(), record["ts"].take());
    event.insert("host_id".into(), record["host_id"].take());
    event.insert("pid".into(), record["pid"].take());
    if record["seq"].is_u64() {
        event.insert("seq".into(), record["seq"].take());
    }
    Some(Value::Object(event))
}

/// A record's position, read without reading all of it, for a line too
/// long to parse for nothing: an event's `seq` is its last key, so at the
/// record's end; an ACP message's comes before the message. None for a
/// record without one (a note of the logger's).
pub(crate) fn position(line: &[u8]) -> Option<u64> {
    const KEY: &[u8] = b"\"seq\":";
    let digits = |at: &[u8]| -> Option<u64> {
        let end = at.iter().position(|b| !b.is_ascii_digit()).unwrap_or(at.len());
        std::str::from_utf8(&at[..end]).ok()?.parse().ok()
    };
    let last = |within: &[u8]| -> Option<u64> {
        let at = within.windows(KEY.len()).rposition(|w| w == KEY)?;
        digits(&within[at + KEY.len()..])
    };
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    // What the record is follows its head (times, ids, pids and the session,
    // none with a quote unescaped in it), in its first KiB unless the
    // session's id is long, when the record is read whole.
    let head = &line[..line.len().min(1024)];
    let find = |key: &[u8]| head.windows(key.len()).position(|w| w == key);
    match (find(b"\"event\":"), find(b"\"dir\":")) {
        // `…,"event":{…,"seq":12}}`: an event's, ending the record.
        (Some(event), dir) if dir.is_none_or(|d| event < d) => {
            last(&line[line.len().saturating_sub(32)..]).filter(|_| line.ends_with(b"}}"))
        }
        // `…,"session":…,"seq":12,"dir":…`: an ACP message's, before it.
        (_, Some(dir)) => last(&head[..dir]),
        _ => None,
    }
}

/// The first record in the file at `path` that brnr can read, as [`head`]
/// reads it.
pub(crate) fn first(path: &Path) -> Option<Value> {
    let lines = BufReader::new(open(path).ok()?).split(b'\n');
    lines.map_while(Result::ok).find_map(|line| head(&line))
}

/// The last record in the file at `path` that brnr can read and `wanted`
/// takes, as [`head`] reads it. Read backwards, so a long transcript costs
/// no more than its last records.
pub(crate) fn last(path: &Path, wanted: impl Fn(&Value) -> bool) -> Option<Value> {
    Backwards::new(path, u64::MAX)?.find_map(|(_, line)| head(&line).filter(&wanted))
}

/// The lines in the last `bytes` of the file at `path`, in order, but one
/// that starts before them (cut off) and empty ones; the last may be cut
/// short. None if it can't be read.
pub(crate) fn tail(path: &Path, bytes: u64) -> Option<Vec<Vec<u8>>> {
    let lines = Backwards::new(path, bytes)?;
    let from = lines.from;
    let kept = lines.take_while(|(at, _)| *at > from || from == 0);
    let mut kept: Vec<Vec<u8>> = kept.map(|(_, line)| line).filter(|l| !l.is_empty()).collect();
    kept.reverse();
    Some(kept)
}

/// Whether the events file at `path` holds a record of its session beyond
/// its openings, which makes it a transcript of the session to a load (ADR
/// 5.6): a load that failed leaves one holding only `session_opened`
/// records. Read up to the first other record.
pub(crate) fn has_records(path: &Path) -> bool {
    let Ok(file) = open(path) else { return false };
    let lines = BufReader::new(file).split(b'\n').map_while(Result::ok);
    lines.filter_map(|line| head(&line)).any(|r| r["event"]["event"] != "session_opened")
}

/// The lines of a file, last first, each with where it starts, read
/// backwards a [`CHUNK`] at a time: first whatever follows its last newline
/// (empty, or a line cut short), and the first line of what is read last.
struct Backwards {
    file: File,
    /// Where what is read starts: no further back than this.
    from: u64,
    /// Where `rest` starts.
    start: u64,
    /// From `start` to the start of the lines already given.
    rest: Vec<u8>,
    done: bool,
}

impl Backwards {
    /// The lines in the last `bytes` of the file at `path`.
    fn new(path: &Path, bytes: u64) -> Option<Backwards> {
        let file = open(path).ok()?;
        let start = file.metadata().ok()?.len();
        Some(Backwards {
            file,
            from: start.saturating_sub(bytes),
            start,
            rest: Vec::new(),
            done: false,
        })
    }
}

impl Iterator for Backwards {
    type Item = (u64, Vec<u8>);

    fn next(&mut self) -> Option<(u64, Vec<u8>)> {
        loop {
            if let Some(i) = self.rest.iter().rposition(|&b| b == b'\n') {
                let line = self.rest.split_off(i + 1);
                self.rest.truncate(i);
                return Some((self.start + i as u64 + 1, line));
            }
            if self.start == self.from {
                return (!replace(&mut self.done, true)).then(|| (self.from, take(&mut self.rest)));
            }
            let at = self.start.saturating_sub(CHUNK).max(self.from);
            let mut chunk = vec![0; (self.start - at) as usize];
            self.file.read_exact_at(&mut chunk, at).ok()?;
            chunk.extend_from_slice(&self.rest);
            (self.rest, self.start) = (chunk, at);
        }
    }
}

/// One project folder's files of a session's transcript, to read (ADR 5.1):
/// its events file, its raw ACP if that is to be read too, and whether its
/// process may still write to them.
pub(crate) struct Files {
    pub(crate) events: PathBuf,
    pub(crate) acp: Option<PathBuf>,
    pub(crate) growing: bool,
}

/// A session's transcript, read as it grows, as `event log` shows it: the
/// files of each project folder it has files in, merged by their records'
/// `ts` (ADR 5.1).
pub(crate) struct Reader {
    folders: Vec<Folder>,
}

impl Reader {
    /// The files of each folder, oldest first.
    pub(crate) fn open(files: &[Files]) -> Result<Reader, String> {
        let folders = files.iter().map(Folder::open).collect::<Result<_, _>>()?;
        Ok(Reader { folders })
    }

    /// What was written since the last read, in time order: each record as
    /// the event `event watch` would have shown ([`event`]). `all` for
    /// everything read, of a transcript that won't grow; a folder no process
    /// writes to any more is read so anyway.
    ///
    /// The folders' records are merged by `ts`, each folder's in the order
    /// it has them: one process serves the session at a time (ADR 3.9), so
    /// they don't overlap in time, and the records a closing process wrote
    /// after the next one's still come before them.
    pub(crate) fn read(&mut self, all: bool) -> Result<Vec<Value>, String> {
        let mut read = Vec::with_capacity(self.folders.len());
        for folder in &mut self.folders {
            read.push(folder.read(all || !folder.growing)?);
        }
        if read.len() == 1 {
            return Ok(read.remove(0).into_iter().map(|(_, event)| event).collect());
        }
        let mut merged = Vec::with_capacity(read.iter().map(Vec::len).sum());
        let mut read: Vec<_> = read.into_iter().map(|one| one.into_iter().peekable()).collect();
        loop {
            // The earliest next record, the oldest folder's at the same time.
            let mut next: Option<(usize, &str)> = None;
            for (i, one) in read.iter_mut().enumerate() {
                let Some((ts, _)) = one.peek() else { continue };
                if next.is_none_or(|(_, at)| ts.as_str() < at) {
                    next = Some((i, ts));
                }
            }
            let Some((i, _)) = next else { return Ok(merged) };
            merged.extend(read[i].next().map(|(_, event)| event));
        }
    }

    /// Lets go of the ACP held for the next read: it is from before.
    pub(crate) fn forget_held(&mut self) {
        self.folders.iter_mut().for_each(|f| f.held.clear());
    }

    /// Says which of its files end partway through a record: a folder's no
    /// process writes to, and, once its process has `gone`, the one it
    /// wrote to.
    pub(crate) fn cut_short(&self, gone: bool) {
        self.folders.iter().filter(|f| gone || !f.growing).for_each(Folder::cut_short);
    }
}

/// One folder's two files of a session, read as they grow: its events, and
/// its raw ACP when `acp` events are chosen, merged in time order (ADR 5.1).
struct Folder {
    events: Tail,
    acp: Option<Tail>,
    /// When the last record read from the events file was written.
    last: String,
    /// ACP written after it, held for one read (see `read`).
    held: Vec<(String, Value)>,
    /// Its process may still write to it.
    growing: bool,
}

impl Folder {
    fn open(files: &Files) -> Result<Folder, String> {
        let acp = files.acp.as_deref().map(Tail::open).transpose()?;
        let (events, growing) = (Tail::open(&files.events)?, files.growing);
        Ok(Folder { events, acp, last: String::new(), held: Vec::new(), growing })
    }

    /// What was written since the last read, in time order, with when each
    /// was written, as [`Reader::read`] has it.
    ///
    /// One writer writes both files, each in time order, and the events file
    /// is read first: ACP up to the last event read is all there, but ACP
    /// past it may follow an event written since. That ACP is held, and goes
    /// with the next read, once the events file has been read again.
    fn read(&mut self, all: bool) -> Result<Vec<(String, Value)>, String> {
        let events = self.events.read()?;
        if let Some((ts, _)) = events.last() {
            self.last.clone_from(ts);
        }
        let mut acp = take(&mut self.held);
        if let Some(tail) = &mut self.acp {
            let (now, later): (Vec<_>, _) =
                tail.read()?.into_iter().partition(|(ts, _)| all || *ts <= self.last);
            acp.extend(now);
            self.held = later;
        }
        // Both in time order: merged, events first at the same time.
        let mut merged = Vec::with_capacity(events.len() + acp.len());
        let (mut events, mut acp) = (events.into_iter().peekable(), acp.into_iter().peekable());
        loop {
            let next = match (events.peek(), acp.peek()) {
                (Some((e, _)), Some((a, _))) if a < e => acp.next(),
                (Some(_), _) => events.next(),
                (None, _) => acp.next(),
            };
            let Some(one) = next else { return Ok(merged) };
            merged.push(one);
        }
    }

    /// Says which of its files end partway through a record, for files that
    /// won't grow.
    fn cut_short(&self) {
        self.events.cut_short();
        if let Some(acp) = &self.acp {
            acp.cut_short();
        }
    }
}

/// One file of a transcript, read as it grows.
struct Tail {
    path: PathBuf,
    file: BufReader<File>,
    /// A line still being written.
    partial: Vec<u8>,
    /// How many lines were read whole.
    lines: usize,
    /// It was said that an older brnr wrote the file.
    older: bool,
}

impl Tail {
    fn open(path: &Path) -> Result<Tail, String> {
        let file = open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Tail {
            path: path.to_owned(),
            file: BufReader::new(file),
            partial: Vec::new(),
            lines: 0,
            older: false,
        })
    }

    /// The records appended since the last read as events ([`event`]), with
    /// when each was written. A line that isn't one is passed over, and said
    /// (ADR 5.1); an older brnr's records too, said once (ADR 5.9).
    fn read(&mut self) -> Result<Vec<(String, Value)>, String> {
        let mut read = Vec::new();
        loop {
            let n = self.file.read_until(b'\n', &mut self.partial);
            let n = match n {
                Ok(n) => n,
                Err(e) => return Err(e.to_string()),
            };
            if n == 0 || !self.partial.ends_with(b"\n") {
                return Ok(read);
            }
            self.lines += 1;
            let line = take(&mut self.partial);
            let one = record(&line).and_then(|record| {
                if older(&record) {
                    return Some(Err(()));
                }
                let ts = record["ts"].as_str().unwrap_or_default().to_owned();
                Some(Ok((ts, event(record)?)))
            });
            let at = || format!("{}:{}", self.path.display(), self.lines);
            match one {
                Some(Ok(one)) => read.push(one),
                Some(Err(())) if !self.older => {
                    self.older = true;
                    say(&format!(
                        "brnr: {}: an older brnr wrote it, in names this one doesn't read (ADR \
                         5.9); its records are not shown",
                        self.path.display()
                    ));
                }
                Some(Err(())) => {}
                None => match json::within(&line, json::OWN_DEPTH, "it") {
                    Err(why) => say(&format!("brnr: {}: {why}, not shown", at())),
                    Ok(()) => {
                        say(&format!("brnr: {} isn't a record brnr can read, not shown", at()))
                    }
                },
            }
        }
    }

    /// Says so if the file ends partway through a line: once nothing more
    /// will be written, it is a record cut short (ADR 5.1).
    fn cut_short(&self) {
        if !self.partial.is_empty() {
            let (path, n) = (self.path.display(), self.partial.len());
            say(&format!("brnr: {path} ends partway through a record ({n} bytes), not shown"));
        }
    }
}

/// `text` on stderr, as the commands say what they couldn't do.
fn say(text: &str) {
    eprintln!("{}", render::clean(text));
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use serde_json::json;

    use super::*;

    /// A directory of this test's own, empty.
    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("brnr-transcript-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// An array nested `depth` deep.
    fn deep(depth: usize) -> String {
        format!("{}{}", "[".repeat(depth), "]".repeat(depth))
    }

    /// A transcript is read only where the logger could have written it
    /// (ADR 9.3): not in a symlinked project folder, nor through a symlink,
    /// nor what isn't a regular file.
    #[test]
    fn adr_0009_a_transcript_is_read_only_where_it_could_be_written() {
        let (home, outside) = (scratch("walk"), scratch("walk-outside"));
        let projects = home.join("projects");
        let record = r#"{"ts":"t","session":"s","event":{"event":"user_message"}}"#;
        fs::write(outside.join("s.jsonl"), format!("{record}\n")).unwrap();
        for folder in ["-a", "-b"] {
            fs::create_dir_all(projects.join(folder)).unwrap();
        }
        for file in ["-a/s.jsonl", "-a/s.acp.jsonl", "-b/t.jsonl"] {
            fs::write(projects.join(file), format!("{record}\n")).unwrap();
        }
        symlink(&outside, projects.join("-linked")).unwrap();
        let link = projects.join("-b/s.jsonl");
        symlink(outside.join("s.jsonl"), &link).unwrap();
        let fifo = projects.join("-b/u.jsonl");
        assert!(std::process::Command::new("mkfifo").arg(&fifo).status().unwrap().success());

        let mut files = events_files_in(&home);
        files.sort();
        assert_eq!(files, [projects.join("-a/s.jsonl"), projects.join("-b/t.jsonl")]);
        assert_eq!(of_session_in(&home, "s"), [projects.join("-a/s.jsonl")]);
        assert_eq!(of_session_in(&home, "u"), Vec::<PathBuf>::new());
        assert_eq!(open(&link).unwrap_err().to_string(), "is a symlink");
        assert!(first(&link).is_none() && last(&link, |_| true).is_none(), "read through it");
        assert!(tail(&link, CHUNK).is_none() && !has_records(&link), "read through it");
        for what in [&fifo, &projects] {
            assert_eq!(open(what).unwrap_err().to_string(), "is not a regular file");
        }
        assert!(first(&projects.join("-a/s.jsonl")).is_some());
        let _ = fs::remove_dir_all(home);
        let _ = fs::remove_dir_all(outside);
    }

    /// Each part of a transcript's path is held to the logger's rules (ADR
    /// 9.3): from the state directory it is in, a symlinked `projects`,
    /// `hosts` or state directory isn't looked in or read through, nor a
    /// file with another hard link, nor what is another user's.
    #[test]
    fn adr_0009_every_part_of_a_transcripts_path_is_the_users_own() {
        let (home, outside) = (scratch("parts"), scratch("parts-outside"));
        let record = r#"{"ts":"t","session":"s","event":{"event":"user_message"}}"#;
        fs::create_dir_all(outside.join("projects/-w")).unwrap();
        fs::create_dir_all(outside.join("hosts")).unwrap();
        for file in ["projects/-w/s.jsonl", "hosts/h.jsonl"] {
            fs::write(outside.join(file), format!("{record}\n")).unwrap();
        }
        let linked = home.join("linked");
        symlink(&outside, &linked).unwrap();
        for sub in ["projects", "hosts"] {
            symlink(outside.join(sub), home.join(sub)).unwrap();
        }
        assert_eq!(events_files_in(&home), Vec::<PathBuf>::new());
        assert_eq!(of_session_in(&home, "s"), Vec::<PathBuf>::new());
        assert_eq!(events_files_in(&linked), Vec::<PathBuf>::new());
        let refused = |path: &Path| open(path).unwrap_err().to_string();
        for (file, part) in [("projects/-w/s.jsonl", "projects"), ("hosts/h.jsonl", "hosts")] {
            let want = format!("{} is a symlink", home.join(part).display());
            assert_eq!(refused(&home.join(file)), want);
            assert_eq!(refused(&linked.join(file)), format!("{} is a symlink", linked.display()));
            assert!(first(&outside.join(file)).is_some(), "{file}");
        }
        let other = outside.join("hosts/other.jsonl");
        fs::hard_link(outside.join("hosts/h.jsonl"), &other).unwrap();
        assert_eq!(refused(&other), "has 2 hard links");
        if sys::uid() != 0 {
            assert_eq!(refused(Path::new("/usr/bin/env")), "/usr/bin is owned by uid 0");
        }
        let _ = fs::remove_dir_all(home);
        let _ = fs::remove_dir_all(outside);
    }

    /// What `session list`, `session resume` and `doctor` take from a
    /// transcript, whatever its end (ADR 5.1): ending in a newline or not;
    /// cut short; a whole line that isn't a record, last or before the
    /// last; empty; nothing readable.
    #[test]
    fn adr_0005_the_first_and_last_records_are_ones_brnr_can_read() {
        let path = scratch("records").join("s.jsonl");
        let cases: [(&str, Option<i64>, Option<i64>); 9] = [
            ("{\"n\":1}\n{\"n\":2}\n", Some(1), Some(2)),
            ("{\"n\":1}\n{\"n\":2}", Some(1), Some(2)),
            ("{\"n\":1}\n{\"n\":2}\n{\"ts\":\"partial", Some(1), Some(2)),
            ("{\"n\":1}\n{\"n\":2}\nnot a record\n\n", Some(1), Some(2)),
            ("{\"n\":1}\n{\"n\":2}\n{\"ts\":\"part\n{\"n\":3}\n", Some(1), Some(3)),
            ("{\"ts\":\"part\n{\"n\":1}\n", Some(1), Some(1)),
            ("{\"n\":1}\n42\n", Some(1), Some(1)),
            ("", None, None),
            ("{\"ts\":\"partial", None, None),
        ];
        let n = |r: Option<Value>| r.and_then(|r| r["n"].as_i64());
        for (text, first_n, last_n) in cases {
            fs::write(&path, text).unwrap();
            let found = (n(first(&path)), n(last(&path, |_| true)));
            assert_eq!(found, (first_n, last_n), "{text:?}");
        }
        // Past the 64 KiB a read backwards takes at a time.
        let long = format!(r#"{{"n":3,"pad":"{}"}}"#, "x".repeat(100 * 1024));
        let junk = "y".repeat(70_000);
        fs::write(&path, format!("{{\"n\":1}}\n{long}\n{junk}\n{{\"ts\":\"cut")).unwrap();
        assert_eq!(n(last(&path, |_| true)), Some(3));
        assert_eq!(n(last(&path, |r| r["n"] == 1)), Some(1));
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    /// A record nested past serde_json's 128 is one brnr can read, up to
    /// `json::OWN_DEPTH` (ADR 2.5): first, last, or beyond the openings. One
    /// level deeper is none brnr wrote, and is passed over.
    #[test]
    fn adr_0002_a_record_is_read_to_the_limit_and_no_deeper() {
        let path = scratch("deep").join("s.jsonl");
        let opened = r#"{"ts":"1","session":"s","event":{"event":"session_opened","cwd":"/w"}}"#;
        let request = |depth: usize| {
            format!(
                r#"{{"ts":"2","host_id":"h","session":"s","event":{{"event":"permission_request","request":"p1","tool_call":{}}}}}"#,
                deep(depth - 2)
            )
        };
        let at = request(json::OWN_DEPTH);
        assert_eq!(json::depth(at.as_bytes()), json::OWN_DEPTH);
        fs::write(&path, format!("{opened}\n{at}\n")).unwrap();
        let read = last(&path, |_| true).unwrap();
        assert_eq!(read["event"]["tool_call"].to_string(), deep(json::OWN_DEPTH - 2));
        assert!(has_records(&path));
        fs::write(&path, format!("{at}\n{opened}\n")).unwrap();
        assert_eq!(first(&path).unwrap()["event"]["request"], "p1");
        let past = request(json::OWN_DEPTH + 1);
        assert_eq!(record(past.as_bytes()), None);
        fs::write(&path, format!("{opened}\n{past}\n")).unwrap();
        assert_eq!(last(&path, |_| true).unwrap()["event"]["event"], "session_opened");
        assert!(!has_records(&path));
        fs::write(&path, format!("{past}\n{opened}\n")).unwrap();
        assert_eq!(first(&path).unwrap()["event"]["event"], "session_opened");
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    /// `doctor` reads the end of a host log: the lines in its last bytes,
    /// but one they cut, and empty ones; the last one even if cut short.
    #[test]
    fn adr_0005_the_end_of_a_file_is_its_last_lines() {
        let path = scratch("tail").join("h.jsonl");
        fs::write(&path, "a\nbb\n\nccc\ndd").unwrap();
        let lines = |bytes| tail(&path, bytes).unwrap();
        for bytes in [12, 100] {
            assert_eq!(lines(bytes), [&b"a"[..], b"bb", b"ccc", b"dd"]);
        }
        assert_eq!(lines(7), [&b"ccc"[..], b"dd"]);
        assert_eq!(lines(6), [b"dd"]);
        assert_eq!(lines(0), Vec::<Vec<u8>>::new());
        assert!(tail(&path.with_extension("none"), 1).is_none());
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    /// A record is the event `event watch` showed (ADR 5.1): a host event as
    /// it was emitted; an ACP message as an `acp` event, its keys in that
    /// order, with its position only where the record has one (ADR 10.2);
    /// a line that wasn't JSON as its text.
    #[test]
    fn adr_0005_a_record_is_the_event_watch_showed() {
        let host = json!({ "ts": "t", "host_id": "h", "session": "s",
            "event": { "event": "agent_message", "text": "hi", "seq": 3 } });
        assert_eq!(event(host), Some(json!({ "event": "agent_message", "text": "hi", "seq": 3 })));
        let acp = json!({ "ts": "t", "host_id": "h", "pid": 1, "session": "s", "seq": 4,
            "dir": "agent->editor", "msg": { "id": 1 } });
        let want = r#"{"event":"acp","dir":"agent->editor","session":"s","msg":{"id":1},"ts":"t","host_id":"h","pid":1,"seq":4}"#;
        assert_eq!(event(acp).unwrap().to_string(), want);
        // The copy of a `session/new` in a session's raw file.
        let copy = json!({ "ts": "t", "host_id": "h", "session": "s", "dir": "editor->agent",
            "msg": { "method": "session/new" } });
        assert_eq!(event(copy).unwrap().get("seq"), None);
        let raw = json!({ "ts": "t", "host_id": "h", "dir": "agent_stderr", "raw": "oops" });
        let raw = event(raw).unwrap();
        assert_eq!((&raw["msg"], &raw["session"]), (&json!("oops"), &Value::Null));
        assert_eq!(event(json!({ "ts": "t", "host_id": "h", "event": {} })), None);
        assert_eq!(event(json!([1])), None);
    }

    /// `event log` reads a session's two files as they grow, merged in time
    /// order: ACP past the last event read is held for the next read, a
    /// line that isn't a record is passed over, one still being written is
    /// read once it ends, and one nested past 128 is read (ADR 2.5, 5.1).
    #[test]
    fn adr_0005_a_sessions_files_are_read_merged_as_they_grow() {
        let dir = scratch("reader");
        let (events, acp) = (dir.join("s.jsonl"), dir.join("s.acp.jsonl"));
        let host = |ts: &str, name: &str, more: &str| {
            format!(r#"{{"ts":"{ts}","host_id":"h","event":{{"event":"{name}"{more}}}}}"#)
        };
        let msg = |ts: &str| {
            format!(r#"{{"ts":"{ts}","host_id":"h","seq":1,"dir":"agent->editor","msg":"{ts}"}}"#)
        };
        let append = |path: &Path, text: &str| {
            let mut file = OpenOptions::new().append(true).open(path).unwrap();
            io::Write::write_all(&mut file, text.as_bytes()).unwrap();
        };
        let names = |read: Vec<Value>| -> Vec<String> {
            let name = |e: &Value| e["msg"].as_str().or(e["event"].as_str()).unwrap().to_owned();
            read.iter().map(name).collect()
        };
        fs::write(
            &events,
            format!("{}\nnot a record\n{}\n", host("1", "a", ""), host("3", "b", "")),
        )
        .unwrap();
        fs::write(&acp, format!("{}\n{}\n", msg("2"), msg("4"))).unwrap();
        let files = Files { events: events.clone(), acp: Some(acp.clone()), growing: true };
        let mut reader = Reader::open(&[files]).unwrap();
        assert_eq!(names(reader.read(false).unwrap()), ["a", "2", "b"]);
        append(&events, &host("5", "c", ""));
        assert_eq!(names(reader.read(false).unwrap()), ["4"]);
        let request = host("6", "permission_request", &format!(r#","tool_call":{}"#, deep(200)));
        append(&events, &format!("\n{request}\n"));
        append(&acp, &format!("{}\n", msg("7")));
        let read = reader.read(true).unwrap();
        assert_eq!(read[1]["tool_call"].to_string(), deep(200));
        assert_eq!(names(read), ["c", "permission_request", "7"]);
        let none = Files { events: dir.join("none.jsonl"), acp: None, growing: true };
        let missing = Reader::open(&[none]).err().unwrap();
        assert!(
            missing.starts_with(&format!("{}: ", dir.join("none.jsonl").display())),
            "{missing}"
        );
        let _ = fs::remove_dir_all(dir);
    }

    /// One session's files in two project folders are one transcript (ADR
    /// 5.1, #121): found by the id their records carry, oldest first, and
    /// read merged by `ts`, each folder's in the order it has them. A folder
    /// no process writes to any more is read whole, its raw ACP too; the one
    /// still written to, where the newest records are, holds the ACP past
    /// its last event, as one folder's read does.
    #[test]
    fn adr_0005_a_sessions_files_in_every_folder_are_read_as_one() {
        let root = scratch("folders");
        let record = |ts: &str, session: &str, name: &str| {
            format!(
                r#"{{"ts":"{ts}","host_id":"h","pid":1,"session":"{session}","event":{{"event":"{name}"}}}}"#
            )
        };
        let msg = |ts: &str| {
            format!(
                r#"{{"ts":"{ts}","host_id":"h","pid":1,"session":"s","seq":1,"dir":"agent->editor","msg":"{ts}"}}"#
            )
        };
        let folder = |name: &str, lines: &[String]| {
            let dir = root.join("projects").join(name);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("s.jsonl"), lines.join("\n") + "\n").unwrap();
            dir.join("s.jsonl")
        };
        // The newer folder sorts first; the files are taken oldest first.
        let a = folder("b-first", &[record("1", "s", "session_opened"), record("5", "s", "x5")]);
        let b = folder("a-then", &[record("3", "s", "session_opened"), record("4", "s", "x4")]);
        // A file of the name whose records are another session's isn't its.
        folder("c-other", &[record("2", "t", "session_opened")]);
        assert_eq!(files_of_in(&root, "s"), [a.clone(), b.clone()]);
        let acp = paths::acp_log(&a);
        fs::write(&acp, format!("{}\n{}\n", msg("1.5"), msg("5.5"))).unwrap();
        let names = |read: Vec<Value>| -> Vec<String> {
            let name = |e: &Value| e["msg"].as_str().or(e["event"].as_str()).unwrap().to_owned();
            read.iter().map(name).collect()
        };
        let files = |growing: bool| {
            [
                Files { events: a.clone(), acp: Some(acp.clone()), growing },
                Files { events: b.clone(), acp: None, growing: false },
            ]
        };
        let all = ["session_opened", "1.5", "session_opened", "x4", "x5", "5.5"];
        let mut reader = Reader::open(&files(false)).unwrap();
        assert_eq!(names(reader.read(false).unwrap()), all);
        // Written to still, the folder holds what follows its last event.
        let mut reader = Reader::open(&files(true)).unwrap();
        assert_eq!(names(reader.read(false).unwrap()), all[..5]);
        assert_eq!(names(reader.read(true).unwrap()), ["5.5"]);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn adr_0010_a_records_position_is_read_from_its_end_or_head() {
        let event = br#"{"ts":"t","host_id":"h","event":{"event":"agent_message","text":"\"seq\":9","x":{"seq":7},"seq":12}}"#;
        assert_eq!(position(event), Some(12));
        let acp = br#"{"ts":"t","host_id":"h","session":"s","seq":13,"dir":"agent->editor","msg":{"seq":1}}"#;
        assert_eq!(position(acp), Some(13));
        let note = br#"{"ts":"t","host_id":"h","event":{"event":"records_skipped","count":1}}"#;
        assert_eq!(position(note), None);
        let raw = br#"{"ts":"t","host_id":"h","dir":"agent_stderr","msg":"x"}"#;
        assert_eq!(position(raw), None);
    }

    /// An events file holding only openings, which a load that failed
    /// leaves, isn't a transcript of its session; one with any other record
    /// is, whatever is between them that brnr can't read (ADR 5.1, ADR 5.6).
    #[test]
    fn adr_0005_only_openings_are_no_transcript() {
        let path = scratch("openings").join("s.jsonl");
        let opened = r#"{"ts":"t","session":"s","event":{"event":"session_opened"}}"#;
        let other = r#"{"ts":"t","session":"s","event":{"event":"user_message"}}"#;
        let cases = [
            ("", false),
            (&format!("{opened}\n"), false),
            (&format!("{opened}\n{opened}\nnot json\n[1]\n{{\"cut\":"), false),
            (&format!("{opened}\nnot json\n{opened}\n{other}\n"), true),
            (other, true),
        ];
        for (text, has) in cases {
            fs::write(&path, text).unwrap();
            assert_eq!(has_records(&path), has, "{text:?}");
        }
        assert!(!has_records(&path.with_extension("none")));
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }
}
