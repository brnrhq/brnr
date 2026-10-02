//! brnr: run ACP agents behind a host you can talk to from outside the
//! editor. One binary:
//!
//! - `brnr proxy`: what the editor runs as its agent (see proxy.rs);
//! - `brnr host`: owns the agent; started by `proxy` and `start`, or by hand
//!   for a headless session in the foreground (see host/);
//! - everything else controls running hosts (see ctl.rs).

mod ctl;

use std::env;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = env::args_os().skip(1);
    match args.next() {
        Some(cmd) if cmd == "proxy" => brnr::proxy::main(args),
        Some(cmd) if cmd == "host" => brnr::host::main(args),
        first => {
            let rest: Vec<String> =
                first.into_iter().chain(args).map(|a| a.to_string_lossy().into_owned()).collect();
            ctl::main(rest)
        }
    }
}
