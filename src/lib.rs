//! An ACP proxy with a side channel: brnr's other commands reach the
//! agent's sessions while it runs, from an editor or headless.
//!
//! ```text
//! editor ──stdio── brnr acp ──socketpairs── brnr host ──pipes── agent
//!                                           │
//!                                control socket ── brnr
//! ```
//!
//! The proxy is the editor's child and only relays bytes. The host, the same
//! binary started again as `brnr host` (see spawn.rs), runs in its own
//! session, reparented to launchd/init, and is the only process that ever
//! touches the agent's stdin, stdout and stderr.
//!
//! # The library's public API
//!
//! brnr is a command-line tool, and this crate is its binary's library: its
//! modules are private, and its public API is a facade that mirrors the
//! command line (ADR 8.5 in docs/adr). [`run`] is the command line as a
//! function. The typed facade has a module for each command group,
//! [`process`], [`session`], [`prompt`], [`queue`], [`permission`],
//! [`elicitation`], [`config`] and [`event`], and the tools, [`doctor`] and
//! [`skill`](fn@skill): a function for each verb, taking the command's
//! arguments, and its flags as a struct, and returning what the command
//! prints with `--json`, with the same names and shapes. An error is an
//! [`Error`], with its [`Code`]. A function runs its command in this
//! process, which says on stderr what the command says there.
//!
//! ```no_run
//! let mut flags = brnr::session::ListFlags::default();
//! flags.include = vec!["active".into()];
//! for row in brnr::session::list(&flags)?.sessions {
//!     println!("{} {}", row.session, row.state);
//! }
//! # Ok::<(), brnr::Error>(())
//! ```
//!
//! ```no_run
//! let mut flags = brnr::session::StartFlags::default();
//! flags.prompt = Some("reply hi".into());
//! flags.wait = true;
//! if let Some(brnr::session::Started::Turn(turn)) = brnr::session::new(&flags)?.value() {
//!     println!("{}", turn.reply);
//! }
//! # Ok::<(), brnr::Error>(())
//! ```
//!
//! [`session::new`] and [`session::resume`] start the process from the
//! running program, as `<program> host`, as brnr does from its binary. A
//! program that calls them hands its arguments to [`run`] when the first is
//! `host`, first thing:
//!
//! ```no_run
//! fn main() -> std::process::ExitCode {
//!     if std::env::args_os().nth(1).is_some_and(|a| a == "host") {
//!         return brnr::run(std::env::args_os().skip(1));
//!     }
//!     // …
//! #   std::process::ExitCode::SUCCESS
//! }
//! ```
//!
//! From 1.0.0 the facade is covered by the compatibility promise. Nothing
//! else is: `internals`, which the crate's own tests and fuzz targets reach
//! behind the `internals` feature, is hidden and can change in any release.

// Every `pub` item is reachable: through the facade or, with the feature
// on, through `internals`; what isn't is `pub(crate)`. Checked when the
// feature is on, since the re-exports are what reach most of them.
#![cfg_attr(feature = "internals", warn(unreachable_pub))]

use std::ffi::OsString;
use std::process::ExitCode;

mod bug;
mod ctl;
mod error;
mod facade;
mod frame;
mod host;
mod json;
mod lock;
mod log;
mod paths;
mod profiles;
mod proxy;
mod remote;
mod render;
mod request;
mod schema;
mod signals;
mod spawn;
mod sys;
mod transcript;

pub use facade::{
    Agent, Check, Checks, Code, DoctorFlags, Dropped, Error, Event, Exit, Reply, Target, Turn,
    config, doctor, elicitation, event, permission, process, profile, prompt, queue, session,
    skill,
};

/// `brnr <args>` as a function: the command line, `args` being the words
/// after `brnr`, run to its exit status. `acp` is what an editor runs as its
/// ACP agent (proxy.rs); `host` owns the agent, started by `acp`, `session
/// new` and `session resume` with one request on its stdin, never by hand
/// (host/ and request.rs); everything else controls running hosts (ctl.rs).
/// A panic but the host's prints a link to report it (bug.rs).
/// Remote execution replaces the calling process with system SSH (ADR 12),
/// so it returns only if routing or executing SSH fails.
///
/// ```
/// brnr::run(["--version".into()]);
/// ```
pub fn run(args: impl IntoIterator<Item = OsString>) -> ExitCode {
    // Each of them starts processes and waits for them, which an inherited
    // SIGCHLD ignored would keep it from (see signals.rs).
    signals::wait_for_children();
    let mut args: Vec<OsString> = args.into_iter().collect();
    // Transport entry point, stripped once. It prevents routing loops without
    // exporting a marker into agents or the commands they subsequently run.
    let received = args.first().is_some_and(|arg| arg == "_remote");
    if received {
        args.remove(0);
    }
    if let Some(code) = remote::dispatch(&args, received) {
        return code;
    }
    let mut args = args.into_iter();
    match args.next() {
        Some(cmd) if cmd == "acp" => {
            bug::install("acp");
            proxy::main(args)
        }
        // The host's panics are its own to record (ADR 3.7).
        Some(cmd) if cmd == "host" => host::main(args),
        first => {
            let rest: Vec<String> =
                first.into_iter().chain(args).map(|a| a.to_string_lossy().into_owned()).collect();
            bug::install(&ctl::command(&rest));
            ctl::main(rest)
        }
    }
}

/// What the crate's own integration tests (tests/) and fuzz targets (fuzz/)
/// reach, with the `internals` feature on: re-exports of internals, which
/// are not part of the public API and not covered by the compatibility
/// promise (ADR 8.5). They can change or go in any release.
#[cfg(feature = "internals")]
#[doc(hidden)]
pub mod internals {
    pub mod error {
        pub use crate::error::names;
    }
    pub mod frame {
        pub use crate::frame::{DATA, read, write};
    }
    pub mod host {
        pub mod fuzz {
            pub use crate::host::fuzz::{Harness, Sent};
        }
    }
    pub mod json {
        pub use crate::json::{DEPTH, OWN_DEPTH, depth, members, parse, replace};
    }
    pub mod log {
        pub use crate::log::rfc3339;
    }
    pub mod paths {
        pub use crate::paths::{claude_project_key, project_key};
    }
    pub mod schema {
        pub use crate::schema::{
            AgentCapabilities, AuthMethod, ListSessionsResponse, NewSessionResponse,
            PermissionOptionKind, SessionUpdate, error_message, read,
        };
    }
}
