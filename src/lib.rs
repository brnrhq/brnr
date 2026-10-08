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

pub mod bug;
pub mod config;
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
