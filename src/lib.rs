//! An ACP proxy with a side channel: brnr's other commands reach the
//! agent's sessions while it runs, from an editor or headless.
//!
//! ```text
//! editor ──stdio── brnr acp ──socketpair── brnr host ──pipes── agent
//!                                          │
//!                               control socket ── brnr
//! ```
//!
//! The proxy is the editor's child and only relays bytes. The host, a
//! separate binary the proxy finds next to itself, runs in its own session,
//! reparented to launchd/init, and is the only process that ever touches the
//! agent's stdin, stdout and stderr.

pub mod config;
pub mod frame;
pub mod host;
pub mod log;
pub mod paths;
pub mod proxy;
pub mod render;
pub mod signals;
pub mod spawn;
