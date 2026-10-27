//! `remote check <target>`: a destination's installation, through SSH
//! (ADR 12.2). It reads no local config.

use super::{Args, args};
use crate::remote;
use std::process::ExitCode;

pub(super) fn check(a: &Args) -> Result<ExitCode, args::Error> {
    remote::check(&a.positional[0], a.json).map_err(Into::into)
}
