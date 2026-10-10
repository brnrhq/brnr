//! brnr: run ACP agents behind a host you can talk to from outside the
//! editor. One binary:
//!
//! - `brnr acp`: what the editor runs as its ACP agent (see proxy.rs);
//! - `brnr host`: owns the agent; started by `acp`, `session new` and
//!   `session resume` with one request on its stdin, never by hand (see host/ and request.rs);
//! - everything else controls running hosts (see ctl.rs).
//!
//! A panic but the host's prints a link to report it (see bug.rs).

mod ctl;

use std::env;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = env::args_os().skip(1);
    match args.next() {
        Some(cmd) if cmd == "acp" => {
            brnr::bug::install("acp");
            brnr::proxy::main(args)
        }
        // The host's panics are its own to record (ADR 11).
        Some(cmd) if cmd == "host" => brnr::host::main(args),
        first => {
            let rest: Vec<String> =
                first.into_iter().chain(args).map(|a| a.to_string_lossy().into_owned()).collect();
            brnr::bug::install(&ctl::command(&rest));
            ctl::main(rest)
        }
    }
}
