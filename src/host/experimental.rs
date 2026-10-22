//! Actions on an editor's session through the side channel (ADR 7.1 in
//! docs/adr): each is refused unless the editor's part of the profile
//! enables it by name, and in strict mode whatever it enables (ADR 2.1).
//! Observing is always allowed. What an action leaves the editor behind on
//! is made up for here, in the editor's own channel:
//!
//! - `send` goes only while no prompt runs: the editor controls its turns,
//!   so nothing is held, steered or interrupts. It is echoed
//!   (`echo_message` in acp.rs, ADR 7.2), and so is `context` as it joins
//!   the editor's prompt.
//! - `cancel`, `permission` and `elicitation`: a permission request or an
//!   elicitation the host answers is withdrawn from the editor with
//!   `$/cancel_request`, and for an allow or a reject its tool call is
//!   updated (ADR 4.5 for elicitations). If the editor answers anyway, its
//!   answer is dropped, so the agent never gets two, and the editor is told
//!   in the session.
//! - `config`: the agent answers a change only to whoever asked (ADR 6.2),
//!   so the editor is sent the `current_mode_update` or
//!   `config_option_update` here.
//! - `close`, and `session resume --take-over` of an editor's session: the
//!   editor is told in the session, and its later requests for it are
//!   answered with an error saying where it continues.
//!
//! brnr's notes to the editor take the echo's form, a completed tool call,
//! whatever the profile's `echo_as` says: they are brnr's words, and a
//! `user_message_chunk` would make them the user's (ADR 7.2).

use serde_json::{Map, Value, json};

use super::approvals::{AgentRequest, Answered, Who};
use super::strict::Beyond;
use super::turns::Activity;
use super::{Host, text_block};
use crate::config::Experimental;
use crate::error::{Code, Error};
use crate::lock;

impl Host {
    /// Refuses `action` on an editor's session unless the editor's part of
    /// the profile enables it, and in strict mode whatever it enables. A
    /// headless session has nothing to refuse.
    pub(super) fn check_experimental(&self, action: Experimental) -> Result<(), Error> {
        if !self.editor_attached() {
            return Ok(());
        }
        self.check_strict(Beyond::Experimental(action))?;
        if self.experimental.contains(&action) {
            return Ok(());
        }
        // Without --profile, the default profile is the one that applies.
        let profile = self.info["profile"].as_str().unwrap_or("default");
        let name = action.name();
        let error = format!(
            "`{name}` on an editor's session is experimental; enable it with \
             `experimental = [\"{name}\"]` under `[profiles.{profile}.editor]`"
        );
        Err(Error::new(Code::experimental_disabled, error))
    }

    /// `send` in `mode` to session `i` of an editor's: context, or a prompt
    /// while none runs, the editor's or brnr's. Mid-turn, whether a message
    /// waits, is steered or interrupts is the editor's call.
    pub(super) fn check_editor_send(&self, i: usize, mode: &str) -> Result<(), Error> {
        if !self.editor_attached() {
            return Ok(());
        }
        let editors = |error: String| Error::new(Code::turn_owned_by_editor, error);
        if matches!(mode, "steer" | "interrupt") {
            return Err(editors(format!(
                "the editor controls this session's turns; --{mode} is its call, not brnr's"
            )));
        }
        let context = mode == "context";
        self.check_experimental(if context { Experimental::Context } else { Experimental::Send })?;
        if !context && self.sessions[i].turns.activity() == Activity::Busy {
            return Err(editors(
                "the editor controls this session's turns; one is running, so send once it has \
                 ended (or with --context, for the editor's next prompt)"
                    .into(),
            ));
        }
        Ok(())
    }

    // ---- approvals ------------------------------------------------------

    /// The host answered the agent's request `req`, `how` (`allowed`,
    /// `rejected` or `cancelled`; an elicitation's `accepted`, `declined` or
    /// `cancelled`), which the editor was shown too: it is withdrawn from
    /// the editor, and its late answer dropped (see approvals.rs). An allowed tool call is in progress, a rejected one has
    /// failed: the status the agent's own updates take it on from, so
    /// neither contradicts the other. An allow or a reject, an accept or a
    /// decline, is also told in the session, saying by whom.
    pub(super) fn withdraw(&mut self, req: &AgentRequest, how: &'static str, by: &Who) {
        let title = req.title();
        let params = json!({ "requestId": req.id });
        let msg = json!({ "jsonrpc": "2.0", "method": "$/cancel_request", "params": params });
        self.send_editor(req.session.as_deref(), &msg);
        let (status, note) = match how {
            "allowed" => (Some("in_progress"), "Allowed via brnr"),
            "rejected" => (Some("failed"), "Rejected via brnr"),
            "accepted" => (None, "Accepted via brnr"),
            "declined" => (None, "Declined via brnr"),
            _ => return,
        };
        let Some(session) = &req.session else { return };
        if let (Some(status), Some(id)) = (status, req.params["toolCall"]["toolCallId"].as_str()) {
            let update =
                json!({ "sessionUpdate": "tool_call_update", "toolCallId": id, "status": status });
            self.update_editor(session, update);
        }
        let text = format!("\"{title}\" was {how} {}.", who(by));
        self.echo(session, note, &[text_block(&text)]);
    }

    /// The editor answered `id` (`msg`), a request the host had `answered`
    /// already: dropped, as the agent must not get a second answer. One the
    /// editor chose (a result, not the error that acknowledges a
    /// `$/cancel_request`) is told to it in the session.
    pub(super) fn late_answer(&mut self, id: &Value, answered: Answered, msg: &Map<String, Value>) {
        let Answered { session, title, how, by } = answered;
        let event = json!({ "event": "editor_response_dropped", "rpc_id": id, "answered": how });
        self.sink.note(session.as_deref(), event);
        if msg.contains_key("result")
            && let Some(session) = session.filter(|s| self.find(s).is_some())
        {
            let text = format!(
                "\"{title}\" was already {how} {}; your answer wasn't passed on to the agent.",
                who(&by)
            );
            self.echo(&session, &format!("Already {how} via brnr"), &[text_block(&text)]);
        }
    }

    // ---- config ---------------------------------------------------------

    /// The agent set `session`'s mode at a bridge's request.
    pub(super) fn mode_set(&mut self, session: &str, mode: &str) {
        let update = json!({ "sessionUpdate": "current_mode_update", "currentModeId": mode });
        self.update_editor(session, update);
    }

    /// The agent set one of `session`'s config options at a bridge's request,
    /// answering with `result`.
    pub(super) fn config_set(&mut self, session: &str, result: &Value) {
        if let Some(options) = result.get("configOptions").filter(|c| c.is_array()) {
            let update =
                json!({ "sessionUpdate": "config_option_update", "configOptions": options });
            self.update_editor(session, update);
        }
    }

    // ---- closing --------------------------------------------------------

    /// brnr is closing `session` under the editor: `brnr session close`, or
    /// `--take-over` by process `taken_by`. The editor is told in the
    /// session, and from now on its requests for it are answered here (see
    /// `closed_error`).
    pub(super) fn closing_under_editor(&mut self, session: &str, taken_by: Option<u32>) {
        if !self.editor_attached() {
            return;
        }
        self.sessions.close_under_editor(session, taken_by);
        let (title, text) = match taken_by {
            Some(pid) => (
                format!("Session taken over by brnr (process {pid})"),
                format!(
                    "brnr session resume --take-over closed it here; it continues in brnr \
                     process {pid} (brnr event watch {session})."
                ),
            ),
            None => (
                "Session closed via brnr".to_owned(),
                "Closed from outside the editor, through brnr; load it again to continue it."
                    .to_owned(),
            ),
        };
        self.echo(session, &title, &[text_block(&text)]);
    }

    /// The agent didn't close `session`, with `error`: it stays open, and
    /// the editor's requests for it go through again.
    pub(super) fn close_failed(&mut self, session: &str, error: &str) {
        if self.sessions.back_to_editor(session) {
            let text = format!("The agent didn't close it ({error}); it stays open here.");
            self.echo(session, "Close via brnr failed", &[text_block(&text)]);
        }
    }

    /// The error for the editor's request `method` about `session`, if brnr
    /// closed it under the editor: where it continues, as its lock says now.
    /// A load or resume, which reaches here once its lock is taken, opens it
    /// again.
    pub(super) fn closed_error(&mut self, session: &str, method: &str) -> Option<String> {
        if matches!(method, "session/load" | "session/resume") {
            self.sessions.back_to_editor(session);
            return None;
        }
        let taken_by = self.sessions.closed_under_editor(session)?;
        let mut error = match taken_by {
            Some(pid) => format!("brnr: session {session} was taken over by brnr (process {pid})"),
            None => format!("brnr: session {session} was closed via brnr"),
        };
        match lock::holder(session) {
            Some(pid) if pid != std::process::id() => error.push_str(&format!(
                "; it continues in brnr process {pid} (`brnr event watch {session}`)"
            )),
            Some(_) => {} // Still closing here.
            None => error.push_str("; load it again to continue it here"),
        }
        Some(error)
    }
}

/// Who answered a request outside the editor, as the editor is told it.
fn who(by: &Who) -> String {
    match (&by.peer, by.reason) {
        (Some(peer), _) if peer.starts_with("socket#") => {
            format!("through brnr's control socket ({peer})")
        }
        (Some(peer), _) => format!("by brnr's bridge {peer}"),
        (None, Some("timeout")) => "by brnr, its time up".to_owned(),
        (None, Some("cancel")) => "through brnr, with its turn".to_owned(),
        (None, _) => "through brnr".to_owned(),
    }
}
