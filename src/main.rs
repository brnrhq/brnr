//! brnr, the binary: `brnr <args>` is [`brnr::run`] with the words after
//! `brnr`. Everything brnr does is in the library.

use std::env;
use std::process::ExitCode;

fn main() -> ExitCode {
    brnr::run(env::args_os().skip(1))
}
