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
//! command line (ADR 8.5 in docs/adr). Today that is [`run`], the command
//! line as a function; a typed facade, one module per command group and one
//! function per verb, follows once `--json`'s shapes are settled (#150).
//! From 1.0.0 the facade is covered by the compatibility promise. Nothing
//! else is: `internals`, which the crate's own tests and fuzz targets reach
//! behind the `internals` feature, is hidden and can change in any release.

// Every `pub` item is reachable: through `run` or, with the feature on,
// through `internals`; what isn't is `pub(crate)`. Checked when the
// feature is on, since the re-exports are what reach most of them.
#![cfg_attr(feature = "internals", warn(unreachable_pub))]

use std::ffi::OsString;
use std::process::ExitCode;

mod bug;
mod config;
mod ctl;
mod error;
mod frame;
mod host;
mod json;
mod lock;
mod log;
mod paths;
mod proxy;
mod render;
mod request;
mod schema;
mod signals;
mod spawn;
mod sys;
mod transcript;

/// `brnr <args>` as a function: the command line, `args` being the words
/// after `brnr`, run to its exit status. `acp` is what an editor runs as its
/// ACP agent (proxy.rs); `host` owns the agent, started by `acp`, `session
/// new` and `session resume` with one request on its stdin, never by hand
/// (host/ and request.rs); everything else controls running hosts (ctl.rs).
/// A panic but the host's prints a link to report it (bug.rs).
///
/// ```
/// brnr::run(["--version".into()]);
/// ```
pub fn run(args: impl IntoIterator<Item = OsString>) -> ExitCode {
    // Each of them starts processes and waits for them, which an inherited
    // SIGCHLD ignored would keep it from (see signals.rs).
    signals::wait_for_children();
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
