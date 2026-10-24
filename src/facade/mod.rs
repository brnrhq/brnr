//! The typed facade (ADR 8.5 in docs/adr): the command line as functions,
//! one module per command group and one function per verb, each returning
//! the data its `--json` prints, with the same names and shapes.
//!
//! A function builds the command's words from its arguments and runs the
//! command in this process, as `brnr::run` would, with `--json`; what the
//! command prints is read into the function's type (`ctl::call`). So
//! there is one interpretation (P5): the function's data is the command's,
//! and a test for each verb compares the two (tests/facade.rs).

use std::fmt::Display;
use std::ops::ControlFlow;
use std::sync::mpsc;
use std::thread;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::ctl::{self, Ran};
pub use crate::error::{Code, Error};

pub mod config;
pub mod elicitation;
pub mod event;
pub mod permission;
pub mod process;
pub mod prompt;
pub mod queue;
pub mod session;

/// The exit status of `--timeout` (ADR 8.3), and of a command cut off that
/// couldn't catch up (ADR 10.2).
const TIMED_OUT: u8 = 124;
const MISSED: u8 = 3;

// ---- what several commands have ------------------------------------------

/// The agent a process runs, as every `--json` has it (ADR 8.2): the file
/// name of the command brnr ran, and what the agent says it is in
/// `initialize`'s `agentInfo`. Each `None` where brnr doesn't know it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Agent {
    pub program: Option<String>,
    pub name: Option<String>,
    pub version: Option<String>,
}

/// A message dropped before it was sent, as its `message_dropped` event has
/// it: `reason` is `cancel`, `queue`, … (ADR 8.2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Dropped {
    pub message: String,
    pub text: String,
    pub reason: String,
}

/// How a turn ended, as `session status`'s `last_turn` has it: ACP's stop
/// reason, `None` if the turn failed; its failure; and the tokens the agent
/// reported, as `turn_ended` has them (ADR 4.4).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Turn {
    pub stop_reason: Option<String>,
    pub error: Option<Error>,
    pub tokens: Option<Value>,
}

/// A turn waited for to its end, as `--wait --json` prints it (ADR 8.3):
/// the agent's messages in it, joined by a blank line, and how it ended.
/// `pid` is the process, from `session new` and `session resume` only.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Reply {
    pub session: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    pub message: String,
    pub reply: String,
    pub stop_reason: Option<String>,
    pub error: Option<Error>,
    pub tokens: Option<Value>,
    pub dropped: Vec<Dropped>,
}

/// An event, as `event watch --json` prints it (docs/reference/events.md):
/// its name, and its fields, `session`, `ts`, `seq` and the rest, as JSON.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Event {
    pub event: String,
    #[serde(flatten)]
    pub fields: Map<String, Value>,
}

impl Event {
    /// The session it is of, if it is of one.
    pub fn session(&self) -> Option<&str> {
        self.fields.get("session").and_then(Value::as_str)
    }

    /// Its field `name`, if it has it.
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.fields.get(name)
    }
}

/// `<session> | --pid <pid>`: a session, or a process for what is in no
/// session (`event watch`, an elicitation outside any session).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Session(String),
    Pid(u32),
}

/// How a command that waits, or whose exit status says more than that it
/// failed, ended, with what it printed (ADR 8.3, docs/reference/exit-codes.md).
/// A command that fails is an [`Error`] instead.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[non_exhaustive]
pub enum Exit<T> {
    /// 0: done, and a turn waited for ended with `end_turn`.
    Success(T),
    /// 1, with what it printed: a turn that failed, stopped otherwise or
    /// whose message was dropped; a session the agent didn't delete; a
    /// `doctor` check that failed.
    Failure(T),
    /// 124: the wait's `--timeout` passed. It cancels nothing.
    TimedOut,
    /// 3: cut off for falling behind, it couldn't catch up from the
    /// transcript; the `events_missed` note says what it missed (ADR 10.2).
    Missed(Event),
}

impl<T> Exit<T> {
    /// The exit status the command exits with.
    pub fn status(&self) -> u8 {
        match self {
            Exit::Success(_) => 0,
            Exit::Failure(_) => 1,
            Exit::TimedOut => TIMED_OUT,
            Exit::Missed(_) => MISSED,
        }
    }

    /// What it printed, if it printed something.
    pub fn value(&self) -> Option<&T> {
        match self {
            Exit::Success(value) | Exit::Failure(value) => Some(value),
            Exit::TimedOut | Exit::Missed(_) => None,
        }
    }

    /// What it printed, if it printed something.
    pub fn into_value(self) -> Option<T> {
        match self {
            Exit::Success(value) | Exit::Failure(value) => Some(value),
            Exit::TimedOut | Exit::Missed(_) => None,
        }
    }

    /// The same exit, its value made another by `f`.
    fn map<U>(self, f: impl FnOnce(T) -> U) -> Exit<U> {
        match self {
            Exit::Success(value) => Exit::Success(f(value)),
            Exit::Failure(value) => Exit::Failure(f(value)),
            Exit::TimedOut => Exit::TimedOut,
            Exit::Missed(note) => Exit::Missed(note),
        }
    }
}

// ---- the tools -----------------------------------------------------------

/// `doctor`'s flags. Its `--report` isn't here: it is text for an issue,
/// not covered (ADR 8.4).
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct DoctorFlags {
    /// `--fix`: also make the safe repairs.
    pub fix: bool,
}

/// `brnr doctor --json`: one check each, in the order they ran.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Checks {
    pub checks: Vec<Check>,
}

/// A check: `level` is `ok`, `info`, `warn` or `fail`; `message` is for
/// people, and not covered.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Check {
    pub level: String,
    pub check: String,
    pub message: String,
}

/// `brnr doctor [--fix]`: [`Exit::Failure`] if a check failed.
pub fn doctor(flags: &DoctorFlags) -> Result<Exit<Checks>, Error> {
    let mut w = Words::new("doctor", None);
    w.flag("--fix", flags.fix);
    w.exit()
}

/// `brnr skill [<reference>]`: the skill for agents that use brnr, or one of
/// its references, as Markdown. Text, not covered (ADR 8.6).
pub fn skill(reference: Option<&str>) -> Result<String, Error> {
    let mut w = Words::new("skill", None);
    w.args(reference);
    w.text()
}

/// `brnr skill install`.
pub mod skill {
    use super::{Error, Words};

    /// `skill install`'s flags.
    #[derive(Clone, Debug, Default)]
    #[non_exhaustive]
    pub struct InstallFlags {
        /// `--dir`: these directories, in place of `~/.claude/skills` and
        /// `~/.agents/skills`.
        pub dir: Vec<String>,
    }

    /// `brnr skill install [--dir <dir>]...`: writes the skill into each
    /// directory's `brnr/`.
    pub fn install(flags: &InstallFlags) -> Result<(), Error> {
        let mut w = Words::new("skill", Some("install"));
        w.values("--dir", &flags.dir);
        w.text().map(drop)
    }
}

// ---- running a command ---------------------------------------------------

/// A command's words, `brnr <cmd> <verb> …`, built from a function's
/// arguments, as a person would type them.
pub(crate) struct Words {
    words: Vec<String>,
    /// What the command reads as its stdin.
    stdin: Option<String>,
}

impl Words {
    pub(crate) fn new(cmd: &str, verb: Option<&str>) -> Words {
        let words = [Some(cmd), verb].into_iter().flatten().map(str::to_owned).collect();
        Words { words, stdin: None }
    }

    /// Positional arguments, those given.
    pub(crate) fn args<'a>(&mut self, args: impl IntoIterator<Item = &'a str>) {
        self.words.extend(args.into_iter().map(str::to_owned));
    }

    pub(crate) fn arg(&mut self, arg: &str) {
        self.args([arg]);
    }

    /// `<session> | --pid <pid>`.
    pub(crate) fn target(&mut self, target: &Target) {
        match target {
            Target::Session(session) => self.arg(session),
            Target::Pid(pid) => self.value("--pid", Some(pid)),
        }
    }

    /// A switch, if it is on.
    pub(crate) fn flag(&mut self, flag: &str, on: bool) {
        if on {
            self.words.push(flag.to_owned());
        }
    }

    /// A flag and its value, if there is one.
    pub(crate) fn value(&mut self, flag: &str, value: Option<impl Display>) {
        if let Some(value) = value {
            self.words.extend([flag.to_owned(), value.to_string()]);
        }
    }

    /// A flag given once for each value.
    pub(crate) fn values(&mut self, flag: &str, values: &[String]) {
        values.iter().for_each(|v| self.value(flag, Some(v)));
    }

    /// A flag given once for each pair, `<k>=<v>`.
    pub(crate) fn pairs(&mut self, flag: &str, pairs: &[(String, String)]) {
        pairs.iter().for_each(|(k, v)| self.value(flag, Some(format!("{k}={v}"))));
    }

    /// `--prompt <text>`, or `--prompt -` and the prompt on stdin for a
    /// prompt that is `-`, which `--prompt` would read as "on stdin".
    pub(crate) fn prompt(&mut self, prompt: Option<&str>) {
        match prompt {
            Some("-") => self.stdin("--prompt", "-", "-".to_owned()),
            prompt => self.value("--prompt", prompt),
        }
    }

    /// `flag` (and `value`, unless empty), the command reading `stdin`.
    pub(crate) fn stdin(&mut self, flag: &str, value: &str, stdin: String) {
        self.words.push(flag.to_owned());
        if !value.is_empty() {
            self.words.push(value.to_owned());
        }
        self.stdin = Some(stdin);
    }

    /// `-- <words>`, if given.
    pub(crate) fn rest(&mut self, rest: Option<&[String]>) {
        if let Some(rest) = rest {
            self.words.push("--".to_owned());
            self.words.extend(rest.iter().cloned());
        }
    }

    /// Runs the command, with `--json` if `json`; it must exit 0.
    fn ran(mut self, json: bool) -> Result<Ran, Error> {
        if json {
            // Before `--`: what follows it is the agent's.
            let at = self.words.iter().position(|w| w == "--").unwrap_or(self.words.len());
            self.words.insert(at, "--json".to_owned());
        }
        crate::signals::wait_for_children();
        ctl::call(&self.words, self.stdin, None)
    }

    /// Runs the command with `--json`: what it printed, as `T`.
    pub(crate) fn json<T: DeserializeOwned>(self) -> Result<T, Error> {
        match self.exit()? {
            Exit::Success(value) => Ok(value),
            other => Err(unexpected(other.status(), "")),
        }
    }

    /// Runs the command with `--json`: how it exited, and what it printed,
    /// as `T`.
    pub(crate) fn exit<T: DeserializeOwned>(self) -> Result<Exit<T>, Error> {
        let ran = self.ran(true)?;
        match (ran.status, ran.missed) {
            (MISSED, Some(note)) => Ok(Exit::Missed(read(&note.to_string())?)),
            (TIMED_OUT, _) => Ok(Exit::TimedOut),
            (0, _) => Ok(Exit::Success(read(&ran.out)?)),
            (1, _) if !ran.out.trim().is_empty() => Ok(Exit::Failure(read(&ran.out)?)),
            (status, _) => Err(unexpected(status, &ran.out)),
        }
    }

    /// Runs the command, which has no `--json`: the text it printed.
    pub(crate) fn text(self) -> Result<String, Error> {
        let ran = self.ran(false)?;
        match ran.status {
            0 => Ok(ran.out),
            status => Err(unexpected(status, &ran.out)),
        }
    }

    /// Runs a command that prints events, one per line, with `--json`, each
    /// given to `each` as it comes, until the command ends or `each` breaks.
    /// The command runs on a thread of its own: broken, it stops where it
    /// next writes, as it would with its reader gone.
    pub(crate) fn events(
        mut self,
        mut each: impl FnMut(Event) -> ControlFlow<()>,
    ) -> Result<Exit<()>, Error> {
        self.words.push("--json".to_owned());
        crate::signals::wait_for_children();
        // Unbuffered: a caller slow to take an event is a reader slow to
        // read, as the process sees it (ADR 10.1).
        let (lines, events) = mpsc::sync_channel(0);
        let (words, stdin) = (self.words, self.stdin);
        let command = thread::spawn(move || ctl::call(&words, stdin, Some(lines)));
        for line in events {
            if each(read(&line)?).is_break() {
                return Ok(Exit::Success(()));
            }
        }
        let ran = command.join().unwrap_or_else(|panic| std::panic::resume_unwind(panic))?;
        match (ran.status, ran.missed) {
            (0, _) => Ok(Exit::Success(())),
            (MISSED, Some(note)) => Ok(Exit::Missed(read(&note.to_string())?)),
            (status, _) => Err(unexpected(status, "")),
        }
    }
}

/// What the command printed, as `T`; that it isn't one is brnr's bug.
fn read<T: DeserializeOwned>(printed: &str) -> Result<T, Error> {
    serde_json::from_str(printed).map_err(|e| {
        let error = format!("the facade can't read what the command printed ({e}): {printed}");
        Error::new(Code::unclassified, error)
    })
}

/// A command that exited as its function doesn't expect: brnr's bug.
fn unexpected(status: u8, printed: &str) -> Error {
    let error = format!("the command exited {status}, as the facade doesn't expect: {printed}");
    Error::new(Code::unclassified, error)
}
