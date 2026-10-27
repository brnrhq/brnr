//! Remote installation checks through SSH destinations or profiles (ADR 12.2).

use super::{Args, args};
use crate::{profiles, remote};
use std::process::ExitCode;

pub(super) fn check(a: &Args) -> Result<ExitCode, args::Error> {
    let settings = match a.value("--profile") {
        Some(name) => profiles::load(Some(name))?
            .0
            .remote
            .ok_or_else(|| format!("profile {name:?} is not a remote routing profile"))?,
        None => profiles::Remote::new(&a.positional[0]),
    };
    remote::check(&settings, a.json).map_err(Into::into)
}
