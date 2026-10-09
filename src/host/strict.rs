//! Strict mode (ADR 41 in docs/adr): stable ACP to the letter, chosen per
//! process (`strict = true` in a profile, `--strict` on `acp` and `start`).
//!
//! By default brnr also speaks the conventions current agents and editors
//! implement alike, ahead of the spec. Strict mode doesn't: what needs one
//! is refused, saying why, and so is every experimental action on an
//! editor's session (ADR 4), whatever the profile enables. The editor's
//! `fs` and `terminal` capabilities pass through (see acp.rs). The only
//! `_meta` brnr acts on is steering's, so in strict mode none is.
//!
//! Which methods are stable is the schema's to say (see schema.rs): the
//! one a convention uses is refused unless the schema crate, built without
//! its unstable features, has it.
//!
//! It is about the protocol only: headless operation, observing a session,
//! the CLI and process management are the same with it and without.

use super::Host;
use crate::config::Experimental;
use crate::schema;

/// What brnr does beyond stable ACP, which strict mode doesn't. A
/// convention is added to brnr by adding it here, and to ADR 41's list.
#[derive(Clone, Copy)]
pub(super) enum Beyond {
    /// `prompt send --steer`: `_session/steering`, an extension, advertised as
    /// `_meta.steering.supported` in `initialize` (ADR 18).
    Steering,
    /// `brnr session fork`: `session/fork`, unstable in ACP v1 (ADR 16).
    Fork,
    /// An action on an editor's session through the side channel (ADR 4).
    Experimental(Experimental),
}

impl Host {
    /// Refuses `what` in strict mode, saying why; allows it otherwise.
    pub(super) fn check_strict(&self, what: Beyond) -> Result<(), String> {
        if !self.strict {
            return Ok(());
        }
        let (what, method) = match what {
            Beyond::Steering => ("--steer", "_session/steering"),
            Beyond::Fork => ("session fork", "session/fork"),
            Beyond::Experimental(action) => {
                return Err(format!(
                    "{} on an editor's session is an experimental action, and strict mode has none",
                    action.name()
                ));
            }
        };
        if schema::stable(method) {
            return Ok(()); // The schema has made it stable since.
        }
        let why = if method.starts_with('_') { "an ACP extension" } else { "unstable in ACP v1" };
        Err(format!("{what} uses {method}, {why}, which strict mode doesn't"))
    }
}
