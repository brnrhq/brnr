//! `brnr event`: a session's events, read back from its transcript, watched
//! live, or waited for (ADR 5, ADR 8.3). `event notify` isn't here: it runs
//! a command for each event, which a program does in [`watch`]'s `each`
//! (ADR 8.5).

use std::ops::ControlFlow;

use serde::{Deserialize, Serialize};

use super::{Error, Event, Exit, Target, Turn, Words, elicitation, permission};

/// `event log`'s flags.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct LogFlags {
    /// `--last <n>`: from the `n`th last message sent to the agent.
    pub last: Option<usize>,
    /// `--follow`: on, as the process writes, until the session closes or
    /// the process exits.
    pub follow: bool,
    /// `--events`: event names, `default` or `all`; the default if empty.
    pub events: Vec<String>,
}

/// `event watch`'s flags.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct WatchFlags {
    /// `--events`: event names, `default` or `all`; the default if empty.
    pub events: Vec<String>,
}

/// `event wait --for`: what to wait for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum For {
    /// No turn running, nothing held, no approval or elicitation waiting.
    #[default]
    Idle,
    /// The next turn that ends.
    Turn,
    /// An approval waiting.
    Permission,
    /// An elicitation waiting.
    Elicitation,
    /// The process exits.
    Exit,
}

/// `event wait`'s flags.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct WaitFlags {
    /// `--for`.
    pub r#for: For,
    /// `--timeout`, in seconds.
    pub timeout: Option<u64>,
}

/// What `event wait --json` prints, one shape for each [`For`] (ADR 8.3).
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(untagged)]
#[non_exhaustive]
// Returned once per call, not kept in bulk: a box would only cost the caller.
#[allow(clippy::large_enum_variant)]
pub enum Waited {
    /// The session's state.
    Idle(Idle),
    /// The turn's `turn_ended`.
    Turn(Event),
    /// The approval waiting, as `permission show` has it.
    Permission(permission::Request),
    /// The elicitation waiting, as `elicitation show` has it.
    Elicitation(elicitation::Request),
    /// The `process_exited` event.
    Exit(Event),
}

/// A session idle, or `inactive` once it has closed, and how its last turn
/// ended.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Idle {
    pub session: String,
    pub state: String,
    pub last_turn: Option<Turn>,
}

/// `brnr event log <session>`: each event, as the transcript has it, given
/// to `each` until the log ends or `each` breaks.
pub fn log(
    session: &str,
    flags: &LogFlags,
    each: impl FnMut(Event) -> ControlFlow<()>,
) -> Result<(), Error> {
    let mut w = Words::new("event", Some("log"));
    w.arg(session);
    w.value("--last", flags.last);
    w.flag("--follow", flags.follow);
    events(&mut w, &flags.events);
    w.events(each).map(drop)
}

/// `brnr event watch (<session> | --pid <pid>)`: each event, live, given to
/// `each` until the session closes, the process exits or `each` breaks.
/// [`Exit::Missed`] once `each` has had the `events_missed` note.
pub fn watch(
    target: &Target,
    flags: &WatchFlags,
    each: impl FnMut(Event) -> ControlFlow<()>,
) -> Result<Exit<()>, Error> {
    let mut w = Words::new("event", Some("watch"));
    w.target(target);
    events(&mut w, &flags.events);
    w.events(each)
}

/// `brnr event wait <session>`.
pub fn wait(session: &str, flags: &WaitFlags) -> Result<Exit<Waited>, Error> {
    let mut w = Words::new("event", Some("wait"));
    w.arg(session);
    let what = match flags.r#for {
        For::Idle => "idle",
        For::Turn => "turn",
        For::Permission => "permission",
        For::Elicitation => "elicitation",
        For::Exit => "exit",
    };
    w.value("--for", Some(what));
    w.value("--timeout", flags.timeout);
    Ok(match flags.r#for {
        For::Idle => w.exit()?.map(Waited::Idle),
        For::Turn => w.exit()?.map(Waited::Turn),
        For::Permission => w.exit()?.map(Waited::Permission),
        For::Elicitation => w.exit()?.map(Waited::Elicitation),
        For::Exit => w.exit()?.map(Waited::Exit),
    })
}

/// `--events`, if any are named.
fn events(w: &mut Words, events: &[String]) {
    let events = events.join(",");
    w.value("--events", (!events.is_empty()).then_some(events));
}
