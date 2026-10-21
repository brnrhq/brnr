//! An error as brnr reports it as data (ADR 8.7): `{"code", "message"}`,
//! the code one of [`Code`]'s, which keeps its meaning through a major
//! version, and the message for people, whose wording can change in any
//! release. An error the agent answered with is `agent_failed`, and has the
//! agent's JSON-RPC error beside it, whole, as `rpc_error` (F1).
//!
//! Every reply that fails, every event or record that reports a failure and
//! every command that fails with `--json` says one of these; a failure no
//! code is given to is [`Code::unclassified`], which promises nothing.

use std::fmt;

use serde_json::{Value, json};

/// Declares [`Code`], its codes' names (each its variant's) and their list.
macro_rules! codes {
    ($($(#[doc = $doc:literal])+ $name:ident,)+) => {
        /// What failed, for a program to act on: one code for each failure a
        /// caller can do something about, not for each place brnr fails.
        #[allow(non_camel_case_types)]
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub(crate) enum Code {
            $($(#[doc = $doc])+ $name,)+
        }

        impl Code {
            /// Every code, in the order docs/interface.md lists them.
            pub(crate) const ALL: &[Code] = &[$(Code::$name,)+];

            /// Its name, as it is in JSON.
            pub(crate) fn name(self) -> &'static str {
                match self {
                    $(Code::$name => stringify!($name),)+
                }
            }
        }
    };
}

codes! {
    /// The agent speaks an ACP version brnr doesn't (ADR 2.2).
    acp_version_unsupported,
    /// The agent exited, and its process with it, before the command was
    /// done: a start, a wait, a request it hadn't answered.
    agent_exited,
    /// The agent answered with a JSON-RPC error: `rpc_error` is the error as
    /// the agent sent it.
    agent_failed,
    /// There is no agent to start: none was given or recorded, or its
    /// program can't be run.
    agent_not_started,
    /// An answer that doesn't fit the permission request or elicitation it
    /// answers: no such option, a kind missing or doubled, an option of the
    /// other side, content that doesn't fit the form, an action that isn't
    /// one.
    answer_invalid,
    /// The agent doesn't advertise what was asked of it: forking, closing,
    /// resuming, listing or deleting sessions, steering, images, an MCP
    /// server's transport.
    capability_missing,
    /// A request on the socket that isn't one: not JSON, no `cmd`, a field
    /// missing or of the wrong type, fields that don't go together.
    command_invalid,
    /// A `cmd` the process doesn't have: it may be an older brnr's.
    command_unknown,
    /// The config file can't be read, isn't private, or doesn't load.
    config_invalid,
    /// An event name the process doesn't have, or a note's, in `subscribe`.
    event_unknown,
    /// An action on an editor's session that its profile doesn't enable
    /// (ADR 7.1).
    experimental_disabled,
    /// What the command reads besides its command line won't do: a prompt
    /// on stdin that is empty, stdin that isn't the form's fields, a file
    /// or image that can't be read or sent, a `--cwd` that isn't a
    /// directory.
    input_invalid,
    /// No held message has that id.
    message_not_found,
    /// No brnr process has that pid.
    process_not_found,
    /// The process is an editor's: sessions aren't opened or forked in it
    /// (ADR 7.1).
    process_owned_by_editor,
    /// brnr itself failed, and its process ended (ADR 3.7): a bug to report.
    process_panicked,
    /// The process hasn't committed its start yet (ADR 3.4).
    process_starting,
    /// The process is stopping, or its agent takes no more input.
    process_stopping,
    /// The process doesn't answer, or can't be reached from here.
    process_unreachable,
    /// The config has no profile of that name.
    profile_not_found,
    /// No permission request or elicitation of that handle is waiting.
    request_not_found,
    /// The session is open in a process, and the command needs it not to
    /// be: closed first, or taken over.
    session_active,
    /// The session closed before the command was done.
    session_closed,
    /// The session is closing; it is free once its records are written
    /// (ADR 5.5).
    session_closing,
    /// The session isn't open in any process; it can be resumed.
    session_inactive,
    /// brnr knows no session of that id (the agent may).
    session_not_found,
    /// The session is opening, and takes no requests until it has.
    session_opening,
    /// A mode, model, thought level, config option or login method the
    /// agent doesn't offer, or a value it doesn't take.
    setting_invalid,
    /// The session didn't open in the time the start had (ADR 3.4).
    start_timed_out,
    /// Strict mode refuses what isn't stable ACP (ADR 2.1).
    strict_mode_refused,
    /// The process keeps no transcript to read (`log`, ADR 6.1).
    transcript_not_kept,
    /// The transcript is in a format this brnr doesn't read.
    transcript_unsupported,
    /// The editor owns the session's turns: steering, interrupting and a
    /// prompt while its turn runs are its call (ADR 7.1).
    turn_owned_by_editor,
    /// Anything else. It promises nothing: scripts must not match it, and a
    /// failure under it may get a code of its own in a minor release.
    unclassified,
}

/// Every code's name, as docs/interface.md lists them: for the tests.
#[cfg(any(test, feature = "internals"))]
pub fn names() -> Vec<&'static str> {
    Code::ALL.iter().map(|c| c.name()).collect()
}

impl Code {
    /// The code named `name`, if this brnr has it.
    pub(crate) fn named(name: &str) -> Option<Code> {
        Code::ALL.iter().copied().find(|c| c.name() == name)
    }
}

impl fmt::Display for Code {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// An error: its code, its message, and the agent's JSON-RPC error when the
/// agent's answer is what failed.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Error {
    pub(crate) code: Code,
    pub(crate) message: String,
    pub(crate) rpc_error: Option<Value>,
}

impl Error {
    pub(crate) fn new(code: Code, message: impl Into<String>) -> Error {
        Error { code, message: message.into(), rpc_error: None }
    }

    /// The agent's answer `rpc_error`, a JSON-RPC error, said as `message`.
    pub(crate) fn agent(message: impl Into<String>, rpc_error: &Value) -> Error {
        Error {
            code: Code::agent_failed,
            message: message.into(),
            rpc_error: Some(rpc_error.clone()),
        }
    }

    /// The same failure, its message changed by `f`.
    pub(crate) fn map(mut self, f: impl FnOnce(String) -> String) -> Error {
        self.message = f(self.message);
        self
    }

    /// The same failure, its message after `what: `.
    pub(crate) fn about(self, what: impl fmt::Display) -> Error {
        self.map(|m| format!("{what}: {m}"))
    }

    /// As JSON: `{code, message}`, and `rpc_error` where the agent's answer
    /// failed.
    pub(crate) fn json(&self) -> Value {
        let mut error = json!({ "code": self.code.name(), "message": self.message });
        if let Some(rpc) = &self.rpc_error {
            error["rpc_error"] = rpc.clone();
        }
        error
    }

    /// A reply that failed with it: `{"ok": false, "error": …}`.
    pub(crate) fn reply(&self) -> Value {
        json!({ "ok": false, "error": self.json() })
    }

    /// The error `value` says, as [`Error::json`] writes it. A code this brnr
    /// doesn't have, or an error that isn't one, is `unclassified`, its
    /// message kept.
    pub(crate) fn of(value: &Value) -> Error {
        let message = match &value["message"] {
            Value::String(m) => m.clone(),
            _ if value.is_string() => value.as_str().unwrap_or_default().to_owned(),
            _ => value.to_string(),
        };
        let code = value["code"].as_str().and_then(Code::named).unwrap_or(Code::unclassified);
        let rpc_error = value.get("rpc_error").filter(|r| !r.is_null()).cloned();
        Error { code, message, rpc_error }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(&self.message)
    }
}

/// What brnr hasn't given a code: `unclassified`.
impl From<String> for Error {
    fn from(message: String) -> Error {
        Error::new(Code::unclassified, message)
    }
}

impl From<&str> for Error {
    fn from(message: &str) -> Error {
        Error::new(Code::unclassified, message)
    }
}

impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Error {
        Error::new(Code::unclassified, err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each code is snake_case, subject first as ADR 5.9 names things, and
    /// one of its kind; JSON has it by its name, and reads it back.
    #[test]
    fn adr_0008_an_error_is_a_code_and_a_message() {
        for (i, code) in Code::ALL.iter().enumerate() {
            let name = code.name();
            let snake =
                name.split('_').all(|w| !w.is_empty() && w.bytes().all(|b| b.is_ascii_lowercase()));
            assert!(snake, "{name}");
            assert_eq!(Code::named(name), Some(*code));
            assert!(!Code::ALL[..i].contains(code), "{name} twice");
        }
        let error = Error::new(Code::session_not_found, "no session s");
        assert_eq!(error.json(), json!({ "code": "session_not_found", "message": "no session s" }));
        assert_eq!(Error::of(&error.json()), error);
        assert_eq!(error.reply(), json!({ "ok": false, "error": error.json() }));
        let rpc = json!({ "code": -32603, "message": "no" });
        let agent = Error::agent("session/new failed: no", &rpc);
        let said = json!({ "code": "agent_failed", "message": "session/new failed: no", "rpc_error": rpc });
        assert_eq!(agent.json(), said);
        assert_eq!(Error::of(&said), agent);
        // A code this brnr doesn't have, or text where an error was due,
        // promises nothing.
        let newer = Error::of(&json!({ "code": "session_frozen", "message": "m" }));
        assert_eq!((newer.code, newer.message.as_str()), (Code::unclassified, "m"));
        assert_eq!(Error::of(&json!("text")), Error::from("text"));
    }
}
