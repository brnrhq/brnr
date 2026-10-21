//! Where a session is, as its lock and the processes say, and what a
//! command may do with it there (ADR 3.9, ADR 3.12, ADR 5.5).
//!
//! [`locate`] finds session `id` among the processes `discover` found: held
//! by the process its lock names (open there, closing, opening, or in a
//! process that doesn't answer or that brnr doesn't list), served by a
//! process without its lock, ended (a transcript no process serves), or
//! unknown to brnr. A command asks the [`Session`] what it may do: [`Session::running`]
//! for the commands that act on a running session, [`Session::resume`] and
//! [`Session::delete`]; `event log` asks [`transcript`]. A refusal says where
//! the session is in the same words, whichever command it is, with what that
//! command can do about it where it can do something. Decided here, and
//! nowhere else:
//!
//! - The lock says who owns a session, whether its holder answers or not;
//!   a process serving it without the lock only shares it (ADR 3.9).
//! - A holder that has closed the session holds its lock until its records
//!   are written, and its status's `closing` tells it from one opening it:
//!   the session is closing there, free in 2 s at most (ADR 5.5).
//! - A transcript a process writes is read once that process has written
//!   what it recorded (`logged`), and one closing once its process has let
//!   go of it (ADR 5.5).

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::error::{self, Code};
use crate::{lock, paths, transcript};

use super::client::{Host, discover};
use super::inactive_sessions;

/// Where a session is.
enum Place<'a> {
    /// Open in `host`, which holds its lock.
    Open(&'a Host),
    /// Served by `host` without its lock, which no process holds: shared,
    /// its holder gone since, or an editor's it couldn't lock (ADR 3.9).
    Shared(&'a Host),
    /// Held by `host`, which has closed it, until its records are written
    /// (ADR 5.5).
    Closing(&'a Host),
    /// Held by `host`, which answers but doesn't serve it yet.
    Opening(&'a Host),
    /// Held by `host`, which doesn't answer: alive, and serving it (ADR 3.9).
    Silent(&'a Host),
    /// Held by process `pid`, which brnr doesn't list: started with runtime
    /// directory `dir`, when that isn't this one (ADR 3.9).
    Unlisted(u32, Option<PathBuf>),
    /// Held by none and served by none: ended, as `inactive_sessions` has it.
    Inactive(Value),
    /// Held by none, served by none, and with no transcript: only the agent
    /// may know it.
    Unknown,
}

/// What a session's lock says of its holder: its pid, and its runtime
/// directory when that isn't this one (ADR 3.9).
type Holder = (u32, Option<PathBuf>);

/// A session, by its id, and where it is.
pub(super) struct Session<'a> {
    id: String,
    place: Place<'a>,
}

/// What `session resume` resumes.
pub(super) enum Resume<'a> {
    /// A session open in this process, which is to close it first
    /// (`--take-over`), as its status has it: its cwd, the process's agent
    /// and profile.
    TakeOver(&'a Host, Value),
    /// One that has ended, as its transcript has it (ADR 3.11).
    Inactive(Value),
    /// One brnr has no transcript of, which goes to the agent as given.
    Unknown,
}

/// A session's transcript, to read: the files of each folder it has files
/// in, oldest first, and the process writing to one, if one is.
struct Files<'a> {
    files: Vec<transcript::Files>,
    host: Option<&'a Host>,
}

/// Session `id` among `hosts`, as its lock says (ADR 3.9).
pub(super) fn locate<'a>(hosts: &'a [Host], id: &str) -> Session<'a> {
    let holder = lock::holder(id).map(|pid| (pid, lock::holder_dir(id)));
    let past = || inactive_sessions(hosts).into_iter().find(|p| p["session"] == id);
    Session { id: id.to_owned(), place: place(hosts, id, holder, past) }
}

/// Where session `id` is, from the facts: `hosts`, the processes; `holder`,
/// the pid its lock names, if one holds it, with that process's runtime
/// directory when it isn't this one; and `past`, its record as a session
/// that has ended, looked for only if no process holds or serves it.
fn place<'a>(
    hosts: &'a [Host],
    id: &str,
    holder: Option<Holder>,
    past: impl FnOnce() -> Option<Value>,
) -> Place<'a> {
    if let Some((pid, dir)) = holder {
        let Some(host) = hosts.iter().find(|h| h.id() == pid.to_string()) else {
            return Place::Unlisted(pid, dir);
        };
        // A session opened again where it was closing is open there, its
        // lock taken back (ADR 5.5); `closing` lists it no more.
        return match () {
            () if host.session(id).is_some() => Place::Open(host),
            () if host.status.is_none() => Place::Silent(host),
            () if host.closing(id) => Place::Closing(host),
            () => Place::Opening(host),
        };
    }
    if let Some(host) = hosts.iter().find(|h| h.session(id).is_some()) {
        return Place::Shared(host);
    }
    past().map_or(Place::Unknown, Place::Inactive)
}

/// "`id` is running in process `pid`".
fn running_in(id: &str, pid: impl std::fmt::Display) -> String {
    format!("{id} is running in process {pid}")
}

/// What is said of `id`, held by process `pid` from runtime directory `dir`,
/// which the commands run here can't reach: that `BRNR_DIR` set to it does
/// (ADR 3.9).
fn elsewhere(id: &str, pid: u32, dir: &Path) -> String {
    let reaches = paths::brnr_dir_is(dir);
    format!(
        "{} with runtime directory {} ({reaches} reaches it)",
        running_in(id, pid),
        dir.display()
    )
}

/// `delete`'s refusal of `id`, open where `said` says: closed first.
fn close_first(said: &str, id: &str) -> String {
    format!("{said}: close it first (brnr session close {id})")
}

impl<'a> Session<'a> {
    /// Where it is, as each command refusing it says, with the code a
    /// script acts on (ADR 8.7).
    fn said(&self) -> error::Error {
        let code = match &self.place {
            Place::Open(_) | Place::Shared(_) => Code::session_active,
            Place::Silent(_) | Place::Unlisted(..) => Code::process_unreachable,
            Place::Closing(_) => Code::session_closing,
            Place::Opening(_) => Code::session_opening,
            Place::Inactive(_) => Code::session_inactive,
            Place::Unknown => Code::session_not_found,
        };
        error::Error::new(code, self.where_())
    }

    /// Where it is, in words.
    fn where_(&self) -> String {
        let id = &self.id;
        match &self.place {
            Place::Open(host) | Place::Shared(host) => running_in(id, host.id()),
            Place::Silent(host) => format!("{}, which is not answering", running_in(id, host.id())),
            Place::Closing(host) => format!(
                "{id} is closing in process {}; it's free once its records are written (2 s at \
                 most)",
                host.id()
            ),
            Place::Opening(host) => format!("{id} is opening in process {}", host.id()),
            Place::Unlisted(pid, None) => {
                format!("{}, which brnr doesn't list", running_in(id, pid))
            }
            Place::Unlisted(pid, Some(dir)) => elsewhere(id, *pid, dir),
            Place::Inactive(_) => format!("{id} isn't running: brnr session resume {id}"),
            Place::Unknown => format!("no session {id} (see brnr session list)"),
        }
    }

    /// The process serving it, for a command that acts on a running
    /// session: the one holding its lock, or one sharing it (ADR 3.9).
    pub(super) fn running(&self) -> Result<&'a Host, error::Error> {
        match self.place {
            Place::Open(host) | Place::Shared(host) => Ok(host),
            _ => Err(self.said()),
        }
    }

    /// What `session resume` may resume, in a process of its own or, with
    /// `into`, in that running one (`--pid`), taking it from the process
    /// holding it with `take_over` (ADR 3.9, ADR 3.11). `--take-over` is
    /// offered only where it can work: a holder that doesn't answer, is
    /// opening the session or isn't listed here can't be asked to close it.
    pub(super) fn resume(
        self,
        into: Option<&Host>,
        take_over: bool,
    ) -> Result<Resume<'a>, error::Error> {
        let said = self.said();
        let id = self.id;
        match self.place {
            Place::Open(host)
            | Place::Shared(host)
            | Place::Opening(host)
            | Place::Silent(host)
                if into.is_some_and(|to| to.id() == host.id()) =>
            {
                let error = format!("{id} is already open in process {}", host.id());
                Err(error::Error::new(Code::session_active, error))
            }
            Place::Open(host) if take_over => {
                let s = host.session(&id).unwrap_or(&Value::Null);
                let status = host.info();
                let past = json!({
                    "session": id,
                    "cwd": s["cwd"],
                    "agent": status["agent"],
                    "profile": status["profile"],
                });
                Ok(Resume::TakeOver(host, past))
            }
            Place::Open(_) => Err(said.map(|said| match into {
                None => format!("{said} (--take-over closes it there and resumes it here)"),
                Some(to) => format!(
                    "{said} (--take-over closes it there and resumes it in process {})",
                    to.id()
                ),
            })),
            Place::Inactive(past) => Ok(Resume::Inactive(past)),
            // Resumed as an id brnr doesn't know (ADR 3.11), saying which of
            // its files aren't read (ADR 5.10, P3).
            Place::Unknown => {
                for (path, why) in unread_transcripts(&id) {
                    errln!("brnr: {}: {why}, not read (ADR 5.10)", path.display());
                }
                Ok(Resume::Unknown)
            }
            _ => Err(said),
        }
    }

    /// What `session delete` may delete: a session no process has open, as
    /// brnr recorded it, if it did (ADR 8.1).
    pub(super) fn delete(self) -> Result<Option<Value>, error::Error> {
        let said = self.said();
        match self.place {
            Place::Open(_) | Place::Shared(_) => Err(said.map(|said| close_first(&said, &self.id))),
            Place::Inactive(past) => Ok(Some(past)),
            Place::Unknown => Ok(None),
            _ => Err(said),
        }
    }

    /// Its transcript's files, for `event log`: every folder's (ADR 5.1),
    /// with a running session's as its process says, and the process to ask
    /// `logged` first.
    fn files(self) -> Result<Files<'a>, error::Error> {
        let said = self.said();
        let id = &self.id;
        let beside = |events: PathBuf| transcript::Files {
            acp: Some(paths::acp_log(&events)),
            events,
            growing: false,
        };
        let mut files: Vec<transcript::Files> =
            transcript::files_of(id).into_iter().map(beside).collect();
        match self.place {
            Place::Open(host) | Place::Shared(host) => {
                let s = host.session(id);
                let Some(events) = s.and_then(|s| s["events_log"].as_str()).map(PathBuf::from)
                else {
                    if files.is_empty() {
                        let error = "this process keeps no transcript (log = \"off\")";
                        return Err(error::Error::new(Code::transcript_not_kept, error));
                    }
                    errln!(
                        "brnr: process {} keeps no transcript of {id} (log = \"off\"): what it does \
                         is not shown",
                        host.id()
                    );
                    return Ok(Files { files, host: Some(host) });
                };
                let acp = s.and_then(|s| s["acp_log"].as_str()).map(PathBuf::from);
                let serving = transcript::Files { events, acp, growing: true };
                match files.iter_mut().find(|f| same_file(&f.events, &serving.events)) {
                    Some(found) => *found = serving,
                    None => files.push(serving),
                }
                Ok(Files { files, host: Some(host) })
            }
            Place::Inactive(past) => {
                if files.is_empty() {
                    files.extend(past["events_log"].as_str().map(PathBuf::from).map(beside));
                }
                Ok(Files { files, host: None })
            }
            // A file whose newest records are in a format this brnr doesn't
            // read is no session to `session list`, but what it can read is
            // shown, and the rest said (ADR 5.10).
            Place::Unknown if !files.is_empty() => Ok(Files { files, host: None }),
            // Passed over as no transcript, and said (ADR 5.10, P3).
            Place::Unknown => match unread_transcripts(id).first() {
                Some((path, why)) => Err(error::Error::new(
                    Code::transcript_unsupported,
                    format!("{}: {why}, not read (ADR 5.10)", path.display()),
                )),
                None => Err(said),
            },
            _ => Err(said),
        }
    }
}

/// Whether `a` and `b` are the same file, however each is spelled.
fn same_file(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (std::fs::metadata(a), std::fs::metadata(b)) {
        (Ok(a), Ok(b)) => (a.dev(), a.ino()) == (b.dev(), b.ino()),
        _ => a == b,
    }
}

/// The events files of session `id` with records in a format this brnr
/// doesn't read (ADR 5.10), and which.
fn unread_transcripts(id: &str) -> Vec<(PathBuf, String)> {
    let files = transcript::of_session(id).into_iter();
    files.filter_map(|path| transcript::unread_in(&path).map(|why| (path, why))).collect()
}

/// The running session `id` names: its process, and its id.
pub(super) fn running_session<'a>(
    hosts: &'a [Host],
    id: &str,
) -> Result<(&'a Host, String), error::Error> {
    Ok((locate(hosts, id).running()?, id.to_owned()))
}

/// What `session delete` says of `id` when process `pid` has taken its lock
/// since [`Session::delete`] found it free: it is open there.
pub(super) fn taken(id: &str, pid: u32) -> String {
    match lock::holder_dir(id) {
        Some(dir) => elsewhere(id, pid, &dir),
        None => close_first(&running_in(id, pid), id),
    }
}

/// The files of session `id`'s transcript, in every folder it has files in
/// (ADR 5.1), oldest first: each one's events and raw ACP (none if its
/// process records none); and the pid of the process serving it, if one
/// is. What of it is in a format this brnr doesn't read is said, and not
/// read (ADR 5.10). A running session's process is asked to have written what it
/// recorded until now first: a thread of its own writes it, and what was
/// just seen happen (a turn `session new --wait` reported) may not be there
/// yet. One closing in its process is read once that process has written its
/// records, `session_closed` last, and let go of it (ADR 5.5).
pub(super) fn transcript(id: &str) -> Result<(Vec<transcript::Files>, Option<i64>), error::Error> {
    let hosts = discover()?;
    // Answered once what it recorded before is written: the close's records,
    // and the lock gone with them.
    let closing = match locate(&hosts, id).place {
        Place::Closing(host) => {
            written(host);
            true
        }
        _ => false,
    };
    let hosts = if closing { discover()? } else { hosts };
    let files = locate(&hosts, id).files()?;
    if let Some(host) = files.host {
        written(host);
    }
    // A file read says what it doesn't read as it reads it; one not read at
    // all, here (ADR 5.10, P3).
    let read = |path: &Path| files.files.iter().any(|f| same_file(&f.events, path));
    for (path, why) in unread_transcripts(id).into_iter().filter(|(p, _)| !read(p)) {
        errln!("brnr: {}: {why}, not shown (ADR 5.10)", path.display());
    }
    Ok((files.files, files.host.and_then(|h| h.meta["host_pid"].as_i64())))
}

/// Asks `host` to have written what it recorded until now (`logged`), and
/// says so when it doesn't answer: what it shows is as far as it has written
/// (ADR 5.5, P3).
fn written(host: &Host) {
    if let Err(e) = host.call(&json!({ "cmd": "logged" })) {
        errln!("brnr: {e}: what it recorded last may not be written yet (a slow disk?)");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Process `pid`: answering with `status`, or not answering without.
    fn host(pid: u32, status: Option<Value>) -> Host {
        Host { meta: json!({ "id": pid.to_string(), "host_pid": pid }), status }
    }

    /// The status of a process serving `sessions` and closing `closing`.
    fn serving(sessions: &[Value], closing: &[&str]) -> Option<Value> {
        let status =
            json!({ "sessions": sessions, "closing": closing, "agent": ["a"], "profile": "p" });
        Some(status)
    }

    fn open(id: &str) -> Value {
        json!({ "session": id, "cwd": "/w", "events_log": "/l/s.jsonl", "acp_log": "/l/s.acp.jsonl" })
    }

    /// Session `s` among `hosts`, its lock held by `holder`, and as ended
    /// with `past`; whether `past` was looked for.
    fn find<'a>(
        hosts: &'a [Host],
        holder: Option<Holder>,
        past: Option<Value>,
    ) -> (Session<'a>, bool) {
        let mut looked = false;
        let place = place(hosts, "s", holder, || {
            looked = true;
            past
        });
        (Session { id: "s".into(), place }, looked)
    }

    fn ended() -> Option<Value> {
        Some(json!({ "session": "s", "events_log": "/l/s.jsonl", "cwd": "/w" }))
    }

    /// Each place, and what each command says of it, the same words for
    /// the same place: running (most commands), resume without and with
    /// `--take-over`, with `--pid` of another process, delete, event log.
    #[test]
    fn adr_0003_each_place_is_refused_in_the_same_words() {
        let elsewhere = PathBuf::from("/run/other");
        let other = host(9, serving(&[], &[]));
        let cases = [
            ("open", vec![host(7, serving(&[open("s")], &[]))], Some((7, None)), None),
            ("shared", vec![host(8, serving(&[open("s")], &[]))], None, None),
            ("closing", vec![host(7, serving(&[], &["s"]))], Some((7, None)), None),
            ("opening", vec![host(7, serving(&[], &[]))], Some((7, None)), None),
            ("silent", vec![host(7, None)], Some((7, None)), None),
            ("unlisted", vec![], Some((7, None)), None),
            ("elsewhere", vec![], Some((7, Some(elsewhere.clone()))), None),
            ("inactive", vec![], None, ended()),
            ("unknown", vec![], None, None),
        ];
        let running = "s is running in process 7";
        let reaches =
            format!("{running} with runtime directory /run/other (BRNR_DIR=/run/other reaches it)");
        let closing =
            "s is closing in process 7; it's free once its records are written (2 s at most)";
        let silent = format!("{running}, which is not answering");
        let unlisted = format!("{running}, which brnr doesn't list");
        let close_first = ": close it first (brnr session close s)";
        // What each says: running, resume, resume --take-over, resume --pid
        // 9, delete, event log; `Ok` where it goes on.
        let said = |name: &str| -> [Result<&str, String>; 6] {
            let refused = |s: &str| Err(s.to_owned());
            match name {
                "open" => [
                    Ok("7"),
                    refused(&format!(
                        "{running} (--take-over closes it there and resumes it here)"
                    )),
                    Ok("take over 7"),
                    refused(&format!(
                        "{running} (--take-over closes it there and resumes it in process 9)"
                    )),
                    refused(&format!("{running}{close_first}")),
                    Ok("/l/s.jsonl from 7"),
                ],
                "shared" => {
                    let running = "s is running in process 8";
                    let refused = || Err(running.to_owned());
                    [
                        Ok("8"),
                        refused(),
                        refused(),
                        refused(),
                        Err(format!("{running}{close_first}")),
                        Ok("/l/s.jsonl from 8"),
                    ]
                }
                "closing" => [(); 6].map(|()| refused(closing)),
                "opening" => [(); 6].map(|()| refused("s is opening in process 7")),
                "silent" => [(); 6].map(|()| refused(&silent)),
                "unlisted" => [(); 6].map(|()| refused(&unlisted)),
                "elsewhere" => [(); 6].map(|()| refused(&reaches)),
                "inactive" => [
                    refused("s isn't running: brnr session resume s"),
                    Ok("inactive"),
                    Ok("inactive"),
                    Ok("inactive"),
                    Ok("inactive"),
                    Ok("/l/s.jsonl"),
                ],
                _ => [
                    refused("no session s (see brnr session list)"),
                    Ok("unknown"),
                    Ok("unknown"),
                    Ok("unknown"),
                    Ok("unknown"),
                    refused("no session s (see brnr session list)"),
                ],
            }
        };
        let resumed = |r: Result<Resume, error::Error>| match r {
            Ok(Resume::TakeOver(h, past)) => {
                assert_eq!(
                    past,
                    json!({ "session": "s", "cwd": "/w", "agent": ["a"], "profile": "p" })
                );
                Ok(format!("take over {}", h.id()))
            }
            Ok(Resume::Inactive(_)) => Ok("inactive".into()),
            Ok(Resume::Unknown) => Ok("unknown".into()),
            Err(e) => Err(e),
        };
        for (name, mut hosts, holder, past) in cases {
            hosts.push(other.clone());
            let at = || find(&hosts, holder.clone(), past.clone()).0;
            let got = [
                at().running().map(|h| h.id().to_owned()),
                resumed(at().resume(None, false)),
                resumed(at().resume(None, true)),
                resumed(at().resume(Some(&other), false)),
                at().delete().map(|p| if p.is_some() { "inactive" } else { "unknown" }.to_owned()),
                at().files().map(|f| {
                    let events = f.files.iter().map(|f| f.events.display().to_string());
                    let events = events.collect::<Vec<_>>().join(", ");
                    match f.host {
                        Some(h) => format!("{events} from {}", h.id()),
                        None => events,
                    }
                }),
            ];
            let got = got.map(|r| r.map_err(|e| e.message));
            let want = said(name).map(|r| r.map(str::to_owned));
            assert_eq!(got, want, "{name}");
        }
    }

    /// The lock decides: a process serving the session without it only
    /// shares it, and one closing it is closing it, not opening it; a
    /// session that has ended is looked for only when nothing holds or
    /// serves it (it reads every transcript).
    #[test]
    fn adr_0003_the_lock_decides_and_closing_comes_before_opening() {
        let hosts = [host(7, serving(&[], &["s"])), host(8, serving(&[open("s")], &[]))];
        let (s, looked) = find(&hosts, Some((7, None)), ended());
        assert!(matches!(s.place, Place::Closing(h) if h.id() == "7") && !looked);
        let hosts = [host(7, serving(&[], &[])), host(8, serving(&[open("s")], &[]))];
        assert!(matches!(find(&hosts, Some((7, None)), None).0.place, Place::Opening(_)));
        let (s, looked) = find(&hosts, None, ended());
        assert!(matches!(s.place, Place::Shared(h) if h.id() == "8") && !looked);
        // Opened again where it was closing: open, its lock taken back.
        let hosts = [host(7, serving(&[open("s")], &["s"]))];
        assert!(matches!(find(&hosts, Some((7, None)), None).0.place, Place::Open(_)));
        let (s, looked) = find(&[], None, ended());
        assert!(matches!(s.place, Place::Inactive(_)) && looked);
    }

    /// `resume --pid` into the process that has the session open already is
    /// refused, `--take-over` or not, whatever state it is in there; closing
    /// there, it is closing.
    #[test]
    fn adr_0003_a_resume_into_its_own_process_is_refused() {
        for status in [serving(&[open("s")], &[]), serving(&[], &[]), None] {
            let hosts = [host(7, status)];
            for take_over in [false, true] {
                let (s, _) = find(&hosts, Some((7, None)), None);
                let refused = resumed_err(s.resume(Some(&hosts[0]), take_over));
                assert_eq!(refused, "s is already open in process 7");
            }
        }
        let hosts = [host(7, serving(&[], &["s"]))];
        let (s, _) = find(&hosts, Some((7, None)), None);
        assert!(resumed_err(s.resume(Some(&hosts[0]), true)).starts_with("s is closing"));
        // Served shared there, without its lock: open there all the same.
        let hosts = [host(8, serving(&[open("s")], &[]))];
        let (s, _) = find(&hosts, None, None);
        assert_eq!(resumed_err(s.resume(Some(&hosts[0]), false)), "s is already open in process 8");
    }

    fn resumed_err(r: Result<Resume, error::Error>) -> String {
        r.err().expect("refused").message
    }

    /// Each place a command refuses a session in has its code, the same
    /// whichever command refuses it (ADR 8.7).
    #[test]
    fn adr_0008_a_refusal_has_its_places_code() {
        let cases = [
            (
                vec![host(7, serving(&[open("s")], &[]))],
                Some((7, None)),
                None,
                Code::session_active,
            ),
            (vec![host(7, serving(&[], &["s"]))], Some((7, None)), None, Code::session_closing),
            (vec![host(7, serving(&[], &[]))], Some((7, None)), None, Code::session_opening),
            (vec![host(7, None)], Some((7, None)), None, Code::process_unreachable),
            (vec![], Some((7, None)), None, Code::process_unreachable),
            (vec![], None, ended(), Code::session_inactive),
            (vec![], None, None, Code::session_not_found),
        ];
        for (hosts, holder, past, code) in cases {
            let (s, _) = find(&hosts, holder, past);
            assert_eq!(s.said().code, code, "{}", s.said().message);
        }
        let hosts = [host(7, serving(&[open("s")], &[]))];
        let (s, _) = find(&hosts, Some((7, None)), None);
        assert_eq!(s.delete().err().map(|e| e.code), Some(Code::session_active));
    }

    /// A process that keeps no transcript (`log = "off"`) has none to read.
    #[test]
    fn adr_0005_a_session_without_a_transcript_has_none_to_read() {
        let hosts = [host(7, serving(&[json!({ "session": "s", "events_log": null })], &[]))];
        let (s, _) = find(&hosts, Some((7, None)), None);
        let error = s.files().err().expect("refused");
        assert_eq!(error.message, "this process keeps no transcript (log = \"off\")");
        assert_eq!(error.code, Code::transcript_not_kept);
        let (s, _) = find(&[], None, ended());
        let files = s.files().expect("files");
        assert_eq!(files.files[0].acp, Some(PathBuf::from("/l/s.acp.jsonl")));
    }
}
