//! Actions on an editor's session through the side channel (ADR 4 in
//! docs/adr): each is refused unless the editor's part of the profile
//! enables it by name, and in strict mode whatever it enables (ADR 41).
//! Observing is always allowed. What an action leaves the editor behind on
//! is made up for here, in the editor's own channel:
//!
//! - `send` goes only while no prompt runs: the editor controls its turns,
//!   so nothing is held, steered or interrupts. It is echoed (`echo` in
//!   acp.rs, ADR 5), and so is `context` as it joins the editor's prompt.
//! - `cancel` and `permission`: a permission request the host answers is
//!   withdrawn from the editor with `$/cancel_request`, and for an allow
//!   or a reject its tool call is updated. If the editor answers anyway, its
//!   answer is dropped, so the agent never gets two, and the editor is told
//!   in the session.
//! - `config`: the agent answers a change only to whoever asked (ADR 28),
//!   so the editor is sent the `current_mode_update` or
//!   `config_option_update` here.
//! - `close`, and `session resume --take-over` of an editor's session: the
//!   editor is told in the session, and its later requests for it are
//!   answered with an error saying where it continues.
//!
//! brnr's notes to the editor take the echo's form, a completed tool call.

use serde_json::{Map, Value, json};

use super::acp::AgentRequest;
use super::strict::Beyond;
use super::{Host, text_block};
use crate::config::Experimental;
use crate::lock;

/// An agent request the host answered while the editor may answer it too:
/// what the editor is told if it does.
pub(super) struct Answered {
    session: Option<String>,
    /// The tool call's title, as the request has it.
    title: String,
    /// `allowed`, `rejected` or `cancelled`.
    how: &'static str,
    /// Who answered it: `cancel`, or a peer's label.
    by: String,
}

impl Host {
    /// Refuses `action` on an editor's session unless the editor's part of
    /// the profile enables it, and in strict mode whatever it enables. A
    /// headless session has nothing to refuse.
    pub(super) fn check_experimental(&self, action: Experimental) -> Result<(), String> {
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
        Err(format!(
            "`{name}` on an editor's session is experimental; enable it with \
             `experimental = [\"{name}\"]` under `[profiles.{profile}.editor]`"
        ))
    }

    /// `send` in `mode` to session `i` of an editor's: context, or a prompt
    /// while none runs, the editor's or brnr's. Mid-turn, whether a message
    /// waits, is steered or interrupts is the editor's call.
    pub(super) fn check_editor_send(&self, i: usize, mode: &str) -> Result<(), String> {
        if !self.editor_attached() {
            return Ok(());
        }
        if matches!(mode, "steer" | "interrupt") {
            return Err(format!(
                "the editor controls this session's turns; --{mode} is its call, not brnr's"
            ));
        }
        let context = mode == "context";
        self.check_experimental(if context { Experimental::Context } else { Experimental::Send })?;
        if !context && !self.sessions[i].prompts.is_empty() {
            return Err("the editor controls this session's turns; one is running, so send once \
                        it has ended (or with --context, for the editor's next prompt)"
                .into());
        }
        Ok(())
    }

    // ---- approvals ------------------------------------------------------

    /// The host answered the agent's request `req`, `how` (`allowed`,
    /// `rejected` or `cancelled`), which the editor was shown too: it is
    /// withdrawn from the editor. An allowed tool call is in progress, a
    /// rejected one has failed: the status the agent's own updates take it
    /// on from, so neither contradicts the other. An allow or a reject is
    /// also told in the session, saying by whom.
    pub(super) fn withdraw(&mut self, req: &AgentRequest, how: &'static str, by: &str) {
        if !self.editor_attached() {
            return;
        }
        let tool = &req.params["toolCall"];
        let title = tool["title"].as_str().unwrap_or("the request").to_owned();
        let answered =
            Answered { session: req.session.clone(), title: title.clone(), how, by: by.to_owned() };
        self.answered.insert(req.key.clone(), answered);
        let params = json!({ "requestId": req.id });
        let msg = json!({ "jsonrpc": "2.0", "method": "$/cancel_request", "params": params });
        self.send_editor(req.session.as_deref(), &msg);
        let status = match how {
            "allowed" => "in_progress",
            "rejected" => "failed",
            _ => return,
        };
        let Some(session) = &req.session else { return };
        if let Some(id) = tool["toolCallId"].as_str() {
            let update =
                json!({ "sessionUpdate": "tool_call_update", "toolCallId": id, "status": status });
            self.update_editor(session, update);
        }
        let text = format!("\"{title}\" was {how} {}.", who(by));
        let note = if how == "allowed" { "Allowed via brnr" } else { "Rejected via brnr" };
        self.echo(session, note, &[text_block(&text)]);
    }

    /// The editor answered `key` (`msg`), a request the host answered
    /// already: dropped, as the agent must not get a second answer. One the
    /// editor chose (a result, not the error that acknowledges a
    /// `$/cancel_request`) is told to it in the session. False if `key`
    /// isn't such a request.
    pub(super) fn late_answer(&mut self, key: &str, id: &Value, msg: &Map<String, Value>) -> bool {
        let Some(answered) = self.answered.remove(key) else { return false };
        let Answered { session, title, how, by } = answered;
        let event = json!({ "event": "editor-response-dropped", "id": id, "answered": how });
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
        true
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
        self.closed.insert(session.to_owned(), taken_by);
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
        if self.closed.remove(session).is_some() {
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
            self.closed.remove(session);
            return None;
        }
        let taken_by = *self.closed.get(session)?;
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
fn who(by: &str) -> String {
    match by {
        "cancel" => "through brnr, with its turn".to_owned(),
        _ if by.starts_with("socket#") => format!("through brnr's control socket ({by})"),
        _ => format!("by brnr's bridge {by}"),
    }
}
