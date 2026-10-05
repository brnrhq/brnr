//! brnr: run ACP agents behind a host you can talk to from outside the
//! editor. One binary:
//!
//! - `brnr acp`: what the editor runs as its ACP agent (see proxy.rs);
//! - `brnr host`: owns the agent; started by `acp` and `start`, or by hand
//!   for a headless session in the foreground (see host/);
//! - everything else controls running hosts (see ctl.rs).

mod ctl;

use std::env;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = env::args_os().skip(1);
    match args.next() {
        // `proxy` is what 0.2.0 called it: editor configs may still say so.
        Some(cmd) if cmd == "acp" || cmd == "proxy" => brnr::proxy::main(args),
        Some(cmd) if cmd == "host" => brnr::host::main(args),
        first => {
            let rest: Vec<String> =
                first.into_iter().chain(args).map(|a| a.to_string_lossy().into_owned()).collect();
            ctl::main(rest)
        }
    }
}
