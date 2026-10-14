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

use std::ffi::OsString;
use std::process::ExitCode;

pub mod bug;
pub mod config;
mod ctl;
pub mod frame;
pub mod host;
pub mod json;
pub mod lock;
pub mod log;
pub mod paths;
pub mod proxy;
pub mod render;
pub mod request;
pub mod schema;
pub mod signals;
pub mod spawn;
pub mod sys;

/// `brnr <args>` as a function: the command line, `args` being the words
/// after `brnr`, run to its exit status. `acp` is what an editor runs as its
/// ACP agent (proxy.rs); `host` owns the agent, started by `acp`, `session
/// new` and `session resume` with one request on its stdin, never by hand
/// (host/ and request.rs); everything else controls running hosts (ctl.rs).
/// A panic but the host's prints a link to report it (bug.rs).
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
        // The host's panics are its own to record (ADR 11).
        Some(cmd) if cmd == "host" => host::main(args),
        first => {
            let rest: Vec<String> =
                first.into_iter().chain(args).map(|a| a.to_string_lossy().into_owned()).collect();
            bug::install(&ctl::command(&rest));
            ctl::main(rest)
        }
    }
}
