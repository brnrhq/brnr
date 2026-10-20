//! A session's turns and the messages waiting for them (ADR 4.1, 4.2,
//! 4.3): the prompts running, what is held and in which order, the steers
//! the agent hasn't answered, and the `turn_ended` that waits for them.
//!
//! It does no I/O. What happens to a session's turns goes in as a call (a
//! message sent, a prompt or a steer answered, a cancel, the queue
//! changed), and what the host is to do comes back as [`Do`]s, in order:
//! what to send the agent, the events to emit, the notes for the host log.
//! The rules it keeps:
//!
//! - A second `session/prompt` never goes while one runs: a message sent
//!   meanwhile is held, each its own turn in the order sent, interrupts
//!   ahead of the rest in their own order (ADR 4.2).
//! - Nothing held goes while a steer is unanswered, and a turn that ends
//!   meanwhile has its `turn_ended` held back until the agent has answered
//!   them all, or they are dropped (ADR 4.1).
//! - A cancel drops what is held, and the unanswered steers, unless it
//!   keeps them (ADR 4.3).

use std::collections::VecDeque;
use std::mem::take;
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};

use super::text_block;

/// A message accepted by the host and not yet sent as a prompt.
#[derive(Clone)]
pub(super) struct Held {
    /// `m<n>`: how `send --wait` finds the turn that answers it.
    pub(super) id: String,
    pub(super) text: String,
    /// Content blocks besides the text (`resource_link`, `image`).
    pub(super) blocks: Vec<Value>,
}

impl Held {
    /// As ACP content: the text, then the other blocks.
    fn content(&self) -> Vec<Value> {
        let text = (!self.text.is_empty()).then(|| text_block(&self.text));
        text.into_iter().chain(self.blocks.iter().cloned()).collect()
    }
}

/// A `session/prompt` the agent hasn't answered yet.
pub(super) struct Prompt {
    id: String,
    injected: bool,
    /// The `m<n>` ids of the messages its turn carries: the one the host sent
    /// as the prompt, if it did, and those steered into it (ADR 4.1).
    messages: Vec<String>,
}

/// What a session's turns are doing: the one answer to whether a message
/// sent now goes, waits or steers (ADR 4.2).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Activity {
    /// No turn runs and nothing waits: a message goes as a prompt.
    Idle,
    /// No turn runs, but messages wait: held ones, or steers the agent
    /// hasn't answered, and with them the last turn's `turn_ended`.
    Waiting,
    /// A turn runs.
    Busy,
}

/// How a message is sent (`send`'s `mode`, ADR 4.2); context is
/// [`Turns::add_context`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Mode {
    Prompt,
    Steer,
    Interrupt,
}

/// What became of a message sent: `send`'s `status` (ADR 4.2).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Status {
    Delivered,
    Held,
    Steered,
    Interrupting,
}

impl Status {
    pub(super) fn name(self) -> &'static str {
        match self {
            Status::Delivered => "delivered",
            Status::Held => "held",
            Status::Steered => "steered",
            Status::Interrupting => "interrupting",
        }
    }
}

/// One thing for the host to do for a session's turns.
pub(super) enum Do {
    /// Send it as a `session/prompt`, then tell [`Turns::start`] the id it
    /// went with.
    Prompt(Held),
    /// Send `_session/steering` with `prompt`, for message `message`.
    Steer {
        message: String,
        prompt: Vec<Value>,
    },
    /// `session/cancel` the running turn.
    Cancel,
    /// A message is now in a turn: show the editor `echo`, if it is
    /// brnr's, then emit its `user_message`, `event`.
    Sent {
        event: Value,
        echo: Option<Vec<Value>>,
    },
    Emit(Value),
    /// For the session's host log.
    Note(Value),
}

/// A session's turns and the messages waiting for them.
pub(super) struct Turns {
    /// The session's id, which its events name.
    session: String,
    /// Prompts sent and not yet answered, in the order they were sent.
    /// Non-empty means a turn is running.
    prompts: VecDeque<Prompt>,
    /// When the running turn started.
    started: Option<Instant>,
    /// Interrupts held, and steers the agent turned back, which go ahead of
    /// `held`, in the order they came.
    interrupts: VecDeque<Held>,
    /// Messages held for a turn of their own each.
    held: VecDeque<Held>,
    /// Messages steered into the running turn whose steer the agent hasn't
    /// answered, in the order sent. Nothing held goes while there are any.
    steering: VecDeque<Held>,
    /// The `turn_ended` of a turn that ended while steers into it were
    /// unanswered: which messages it carried isn't known until the agent
    /// has answered them all (or they are dropped), so it waits (ADR 4.1).
    ended: Option<Value>,
    /// Context to append to the next prompt.
    context: Vec<String>,
    /// How the last turn ended: `{stop_reason, error, tokens}`, as
    /// `turn_ended`.
    last_turn: Option<Value>,
}

impl Turns {
    pub(super) fn new(session: &str) -> Turns {
        Turns {
            session: session.to_owned(),
            prompts: VecDeque::new(),
            started: None,
            interrupts: VecDeque::new(),
            held: VecDeque::new(),
            steering: VecDeque::new(),
            ended: None,
            context: Vec::new(),
            last_turn: None,
        }
    }

    /// What its turns are doing (see [`Activity`]).
    pub(super) fn activity(&self) -> Activity {
        if !self.prompts.is_empty() {
            Activity::Busy
        } else if self.interrupts.is_empty() && self.held.is_empty() && self.steering.is_empty() {
            Activity::Idle
        } else {
            Activity::Waiting
        }
    }

    /// What a message sent now in `mode` would become.
    pub(super) fn status(&self, mode: Mode) -> Status {
        match (self.activity(), mode) {
            (Activity::Idle, _) => Status::Delivered,
            (_, Mode::Prompt) => Status::Held,
            (_, Mode::Steer) => Status::Steered,
            (Activity::Busy, Mode::Interrupt) => Status::Interrupting,
            // With no turn running but steers unanswered, the last turn's
            // `turn_ended` waits for them, and so does what is held
            // (ADR 4.1): held, ahead of the rest, with no turn to cancel.
            (Activity::Waiting, Mode::Interrupt) => Status::Held,
        }
    }

    /// `send` (ADR 4.2): a prompt now, held, steered into the running turn,
    /// or an interrupt, which cancels it and goes ahead of what is held.
    pub(super) fn send(&mut self, held: Held, mode: Mode) -> (Status, Vec<Do>) {
        let status = self.status(mode);
        let todo = match status {
            Status::Delivered => vec![Do::Prompt(held)],
            Status::Steered => {
                let steer = Do::Steer { message: held.id.clone(), prompt: held.content() };
                self.steering.push_back(held);
                vec![steer]
            }
            Status::Held if mode == Mode::Prompt => {
                self.held.push_back(held);
                Vec::new()
            }
            Status::Held => {
                self.interrupts.push_back(held);
                Vec::new()
            }
            Status::Interrupting => {
                self.interrupts.push_back(held);
                vec![Do::Cancel]
            }
        };
        (status, todo)
    }

    /// `send --context`: appended to the next prompt, or in place of the
    /// last context with `replace`.
    pub(super) fn add_context(&mut self, text: String, replace: bool) {
        match self.context.last_mut() {
            Some(last) if replace => *last = text,
            _ => self.context.push(text),
        }
    }

    /// The context for the editor's prompt, which carries it from now.
    pub(super) fn take_context(&mut self) -> Vec<String> {
        take(&mut self.context)
    }

    /// Message `held` went as prompt `key` (see [`Do::Prompt`]), the context
    /// held appended: its content, and its `user_message`.
    pub(super) fn start(&mut self, key: String, held: Held) -> (Vec<Value>, Do) {
        let mut blocks = held.content();
        blocks.extend(take(&mut self.context).iter().map(|t| text_block(t)));
        let text = prompt_text(Some(&json!(blocks)));
        let event = self.user_message("control", Some(&held.id), &key, text);
        self.begin(Prompt { id: key, injected: true, messages: vec![held.id] });
        (blocks.clone(), Do::Sent { event, echo: Some(blocks) })
    }

    /// The editor's prompt `key`, with the content `prompt`: a turn of the
    /// editor's, and its `user_message`.
    pub(super) fn editor_prompt(&mut self, key: &str, prompt: Option<&Value>) -> Do {
        let event = self.user_message("editor", None, key, prompt_text(prompt));
        self.begin(Prompt { id: key.to_owned(), injected: false, messages: Vec::new() });
        Do::Sent { event, echo: None }
    }

    fn begin(&mut self, prompt: Prompt) {
        if self.prompts.is_empty() {
            self.started = Some(Instant::now());
        }
        self.prompts.push_back(prompt);
    }

    /// The agent's answer to prompt `key`, which ended as `ended` says
    /// (`{stop_reason, error, tokens}`): its `turn_ended`, now or once the
    /// steers into it are answered, then the next held message.
    pub(super) fn answered(&mut self, key: &str, ended: Value) -> Vec<Do> {
        let pos = self.prompts.iter().position(|p| p.id == key);
        let prompt = pos.and_then(|pos| self.prompts.remove(pos));
        if self.prompts.is_empty() {
            self.started = None;
        }
        self.last_turn = Some(ended.clone());
        let event = turn_ended(&self.session, key, prompt.as_ref(), ended);
        let mut todo = Vec::new();
        if self.steering.is_empty() {
            todo.push(Do::Emit(event));
        } else {
            self.ended = Some(event);
        }
        self.next(&mut todo);
        todo
    }

    /// The agent's answer to steering `message`. `injected`: it is in the
    /// running turn, whose `turn_ended` lists it, even if the turn's own
    /// answer came first (its `turn_ended` waits for the steers').
    /// `promptRequired`: the turn ended first, and it goes next as a prompt
    /// of its own, ahead of what is held as it would have been in the turn
    /// (after interrupts, which end that turn). Anything else, an error
    /// too, drops it.
    pub(super) fn steer_answered(&mut self, message: &str, answer: &Map<String, Value>) -> Vec<Do> {
        // Gone meanwhile (a cancel, a close): it doesn't go out.
        let Some(pos) = self.steering.iter().position(|h| h.id == message) else {
            return Vec::new();
        };
        let held = self.steering.remove(pos).unwrap();
        let mut todo = Vec::new();
        match answer.get("result").and_then(|r| r["outcome"].as_str()) {
            Some("injected") => self.injected(held, &mut todo),
            Some("promptRequired") => self.interrupts.push_back(held),
            _ => {
                let answer = answer.get("error").or(answer.get("result"));
                todo.push(Do::Note(json!({ "event": "steer_refused", "response": answer })));
                todo.extend(self.dropped(vec![held], "steer").1);
            }
        }
        self.turn_answered(&mut todo);
        self.next(&mut todo);
        todo
    }

    /// A steered message the agent took into the turn, which carries it from
    /// now: it is shown as sent, as a prompt is. The turn is the running
    /// one, or the one that ended before the agent answered (whose
    /// `turn_ended` is waiting for it).
    fn injected(&mut self, held: Held, todo: &mut Vec<Do>) {
        let key = if let Some(ended) = &mut self.ended {
            if let Some(messages) = ended["messages"].as_array_mut() {
                messages.push(json!(held.id));
            }
            ended["prompt"].as_str().unwrap_or_default().to_owned()
        } else if let Some(prompt) = self.prompts.front_mut() {
            prompt.messages.push(held.id.clone());
            prompt.id.clone()
        } else {
            // No turn to carry it. A steer goes while a turn runs, or while
            // one's `turn_ended` waits for an earlier steer's answer, so this
            // shouldn't happen; if it does, it isn't left without an end.
            todo.push(Do::Note(json!({ "event": "steer_after_turn", "message": held.id })));
            todo.extend(self.dropped(vec![held], "steer").1);
            return;
        };
        let blocks = held.content();
        let text = prompt_text(Some(&json!(blocks)));
        let event = self.user_message("control", Some(&held.id), &key, text);
        todo.push(Do::Sent { event, echo: Some(blocks) });
    }

    /// The editor's own steer, with `text`, which the agent took into the
    /// running turn: a `user_message` of that turn, by the editor, as brnr's
    /// own steers are. The steer itself went through untouched.
    pub(super) fn editor_steered(&mut self, text: String) -> Do {
        match self.prompts.front() {
            Some(prompt) => {
                let event = self.user_message("editor", None, &prompt.id, text);
                Do::Sent { event, echo: None }
            }
            None => Do::Note(json!({ "event": "steer_after_turn", "by": "editor" })),
        }
    }

    /// `cancel` (ADR 4.3): what is held is dropped, unless `keep_held`, and
    /// the running turn, if one is, cancelled. Its status (`cancelling` or
    /// `idle`), and the messages dropped as `{message, text, reason}`.
    pub(super) fn cancel(&mut self, keep_held: bool) -> (&'static str, Vec<Value>, Vec<Do>) {
        let (dropped, mut todo) =
            if keep_held { (Vec::new(), Vec::new()) } else { self.drop_held("cancel") };
        if self.activity() != Activity::Busy {
            return ("idle", dropped, todo);
        }
        todo.push(Do::Cancel);
        ("cancelling", dropped, todo)
    }

    /// Everything held, messages and context alike, which won't go: the
    /// session is closing, or its process exiting (`reason` `close`, `exit`).
    pub(super) fn drop_all(&mut self, reason: &str) -> Vec<Do> {
        let (_, mut todo) = self.drop_held(reason);
        todo.extend(self.drop_context(reason));
        todo
    }

    /// Drops what is held, and the steers the agent hasn't answered:
    /// whatever it answers, they don't go out. Then the `turn_ended` that
    /// waited for them goes.
    fn drop_held(&mut self, reason: &str) -> (Vec<Value>, Vec<Do>) {
        let held = self.steering.drain(..).chain(self.interrupts.drain(..));
        let held: Vec<Held> = held.chain(self.held.drain(..)).collect();
        let (dropped, mut todo) = self.dropped(held, reason);
        self.turn_answered(&mut todo);
        (dropped, todo)
    }

    /// The `turn_ended` that waited for the steers into its turn, once none
    /// is left unanswered.
    fn turn_answered(&mut self, todo: &mut Vec<Do>) {
        if self.steering.is_empty()
            && let Some(event) = self.ended.take()
        {
            todo.push(Do::Emit(event));
        }
    }

    /// The next held message, as a prompt, once no turn runs and no steer
    /// waits for its answer.
    fn next(&mut self, todo: &mut Vec<Do>) {
        if self.prompts.is_empty()
            && self.steering.is_empty()
            && let Some(held) = self.interrupts.pop_front().or_else(|| self.held.pop_front())
        {
            todo.push(Do::Prompt(held));
        }
    }

    /// Messages taken from those held, never to be sent: each is a
    /// `message_dropped` event, `reason` `cancel`, `queue`, `close` or `exit`
    /// (ADR 5.2), or `steer` for a steer the agent refused. Returns them as
    /// `{message, text, reason}` too, as the events have them, for a
    /// response (ADR 8.2).
    fn dropped(&self, held: Vec<Held>, reason: &str) -> (Vec<Value>, Vec<Do>) {
        let events = held.iter().map(|h| {
            Do::Emit(json!({
                "event": "message_dropped",
                "session": self.session,
                "message": h.id,
                "text": h.text,
                "reason": reason,
            }))
        });
        let events = events.collect();
        let dropped =
            held.into_iter().map(|h| json!({ "message": h.id, "text": h.text, "reason": reason }));
        (dropped.collect(), events)
    }

    /// Context held for the next prompt, which won't come: each is a
    /// `context_dropped` event, `reason` `close`, `exit` or `queue` (ADR 5.2;
    /// context has no message id, ADR 4.1).
    fn drop_context(&mut self, reason: &str) -> Vec<Do> {
        let context = take(&mut self.context).into_iter();
        let event = |text| json!({ "event": "context_dropped", "session": self.session, "text": text, "reason": reason });
        context.map(|text| Do::Emit(event(text))).collect()
    }

    /// `queue` (ADR 4.2): drops held message `drop`, refused if there is
    /// none, then, with `clear`, every held message, then, with
    /// `clear_context`, the context. Returns the messages dropped as
    /// `{message, text, reason}`.
    pub(super) fn queue(
        &mut self,
        drop: Option<&str>,
        clear: bool,
        clear_context: bool,
    ) -> Result<(Vec<Value>, Vec<Do>), String> {
        let mut gone = Vec::new();
        if let Some(id) = drop {
            let interrupt = self.interrupts.iter().position(|h| h.id == id);
            let held = match interrupt {
                Some(pos) => self.interrupts.remove(pos),
                None => self.held.iter().position(|h| h.id == id).and_then(|p| self.held.remove(p)),
            };
            gone.push(held.ok_or(format!("no held message {id}"))?);
        }
        if clear {
            gone.extend(self.interrupts.drain(..).chain(self.held.drain(..)));
        }
        let (dropped, mut todo) = self.dropped(gone, "queue");
        if clear_context {
            todo.extend(self.drop_context("queue"));
        }
        Ok((dropped, todo))
    }

    /// Held message `id`, and whether it is an interrupt.
    pub(super) fn held_message(&self, id: &str) -> Option<(&Held, bool)> {
        let interrupts = self.interrupts.iter().map(|h| (h, true));
        interrupts.chain(self.held.iter().map(|h| (h, false))).find(|(h, _)| h.id == id)
    }

    /// The held messages, in the order they go, for `queue`.
    pub(super) fn queued(&self) -> Vec<Value> {
        let interrupts = self.interrupts.iter().map(|h| (h, true));
        let held = interrupts.chain(self.held.iter().map(|h| (h, false)));
        let entry = |(h, interrupt): (&Held, bool)| json!({ "message": h.id, "text": h.text, "interrupt": interrupt, "attachments": h.blocks.len() });
        held.map(entry).collect()
    }

    /// How many messages are held.
    pub(super) fn held(&self) -> usize {
        self.interrupts.len() + self.held.len()
    }

    pub(super) fn context(&self) -> &[String] {
        &self.context
    }

    /// How many prompts are unanswered.
    pub(super) fn prompts(&self) -> usize {
        self.prompts.len()
    }

    /// How long the running turn has run.
    pub(super) fn running_for(&self) -> Option<Duration> {
        self.started.map(|t| t.elapsed())
    }

    pub(super) fn last_turn(&self) -> Option<&Value> {
        self.last_turn.as_ref()
    }

    /// A `user_message`: message `message`'s (none for the editor's own)
    /// in the turn of prompt `prompt`, sent `by` `control` or `editor`.
    fn user_message(&self, by: &str, message: Option<&str>, prompt: &str, text: String) -> Value {
        let mut event = json!({ "event": "user_message", "session": self.session, "by": by });
        if let Some(message) = message {
            event["message"] = json!(message);
        }
        event["prompt"] = json!(prompt);
        event["text"] = json!(text);
        event
    }
}

/// The `turn_ended` of `session`'s prompt `key`, which ended as `ended` says
/// (`{stop_reason, error, tokens}`). `prompt` is what the session kept of it:
/// none for one it doesn't know, the session having closed meanwhile.
pub(super) fn turn_ended(session: &str, key: &str, prompt: Option<&Prompt>, ended: Value) -> Value {
    let injected = prompt.is_some_and(|p| p.injected);
    let messages = prompt.map(|p| p.messages.clone()).unwrap_or_default();
    let mut event = json!({
        "event": "turn_ended",
        "session": session,
        "by": if injected { "control" } else { "editor" },
        "prompt": key,
        "messages": messages,
    });
    if let (Some(event), Value::Object(ended)) = (event.as_object_mut(), ended) {
        event.extend(ended);
    }
    event
}

/// A prompt's content as text: its text blocks, with other blocks shown as
/// `[image]`, `[resource <uri>]` and so on.
pub(super) fn prompt_text(blocks: Option<&Value>) -> String {
    let blocks = blocks.and_then(Value::as_array).map_or(&[][..], Vec::as_slice);
    let parts: Vec<String> = blocks
        .iter()
        .map(|b| match b["type"].as_str() {
            Some("text") => b["text"].as_str().unwrap_or_default().to_owned(),
            Some(kind) => match b["uri"].as_str().or(b["resource"]["uri"].as_str()) {
                Some(uri) => format!("[{kind} {uri}]"),
                None => format!("[{kind}]"),
            },
            None => "[?]".to_owned(),
        })
        .collect();
    parts.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A session's turns, and the prompt ids it has sent with.
    struct Session {
        turns: Turns,
        prompts: u32,
    }

    fn held(id: &str) -> Held {
        Held { id: id.to_owned(), text: format!("text of {id}"), blocks: Vec::new() }
    }

    impl Session {
        fn new() -> Session {
            Session { turns: Turns::new("s1"), prompts: 0 }
        }

        /// What `todo` says, one line each, as the host would do it: a
        /// prompt goes with the next id, `k<n>`.
        fn run(&mut self, todo: Vec<Do>) -> Vec<String> {
            let mut said = Vec::new();
            for step in todo {
                match step {
                    Do::Prompt(held) => {
                        self.prompts += 1;
                        let key = format!("k{}", self.prompts);
                        said.push(format!("prompt {} as {key}", held.id));
                        let (_, sent) = self.turns.start(key, held);
                        said.extend(self.run(vec![sent]));
                    }
                    Do::Steer { message, .. } => said.push(format!("steer {message}")),
                    Do::Cancel => said.push("cancel".into()),
                    Do::Sent { event, .. } => {
                        let by = event["message"].as_str().unwrap_or("editor");
                        said.push(format!(
                            "user_message {by} in {}",
                            event["prompt"].as_str().unwrap()
                        ));
                    }
                    Do::Emit(e) if e["event"] == "turn_ended" => {
                        let messages: Vec<&str> = e["messages"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .map(|m| m.as_str().unwrap())
                            .collect();
                        said.push(format!(
                            "turn_ended {} [{}]",
                            e["prompt"].as_str().unwrap(),
                            messages.join(" ")
                        ));
                    }
                    Do::Emit(e) if e["event"] == "message_dropped" => {
                        said.push(format!(
                            "dropped {} by {}",
                            e["message"].as_str().unwrap(),
                            e["reason"].as_str().unwrap()
                        ));
                    }
                    Do::Emit(e) => said.push(format!(
                        "{} by {}",
                        e["event"].as_str().unwrap(),
                        e["by"].as_str().or(e["reason"].as_str()).unwrap()
                    )),
                    Do::Note(e) => said.push(format!("note {}", e["event"].as_str().unwrap())),
                }
            }
            said
        }

        fn send(&mut self, id: &str, mode: Mode) -> (&'static str, Vec<String>) {
            let (status, todo) = self.turns.send(held(id), mode);
            (status.name(), self.run(todo))
        }

        fn answered(&mut self, key: &str) -> Vec<String> {
            let todo = self.turns.answered(key, json!({ "stop_reason": "end_turn" }));
            self.run(todo)
        }

        fn steer_answered(&mut self, message: &str, answer: Value) -> Vec<String> {
            let todo = self.turns.steer_answered(message, answer.as_object().unwrap());
            self.run(todo)
        }
    }

    fn injected() -> Value {
        json!({ "result": { "outcome": "injected" } })
    }

    #[test]
    fn adr_0004_interrupts_go_ahead_of_held_messages_in_their_order() {
        let mut s = Session::new();
        assert_eq!(s.turns.activity(), Activity::Idle);
        assert_eq!(
            s.send("m1", Mode::Prompt),
            ("delivered", vec!["prompt m1 as k1".into(), "user_message m1 in k1".into()])
        );
        assert_eq!(s.turns.activity(), Activity::Busy);
        assert_eq!(s.send("m2", Mode::Prompt), ("held", vec![]));
        assert_eq!(s.send("m3", Mode::Interrupt), ("interrupting", vec!["cancel".into()]));
        assert_eq!(s.send("m4", Mode::Interrupt), ("interrupting", vec!["cancel".into()]));
        let queued: Vec<_> = s
            .turns
            .queued()
            .iter()
            .map(|q| (q["message"].clone(), q["interrupt"].clone()))
            .collect();
        assert_eq!(
            queued,
            [(json!("m3"), json!(true)), (json!("m4"), json!(true)), (json!("m2"), json!(false))]
        );
        // A second prompt never goes while one runs.
        assert_eq!(
            s.answered("k1"),
            ["turn_ended k1 [m1]", "prompt m3 as k2", "user_message m3 in k2"]
        );
        assert_eq!(
            s.answered("k2"),
            ["turn_ended k2 [m3]", "prompt m4 as k3", "user_message m4 in k3"]
        );
        assert_eq!(
            s.answered("k3"),
            ["turn_ended k3 [m4]", "prompt m2 as k4", "user_message m2 in k4"]
        );
        assert_eq!(s.answered("k4"), ["turn_ended k4 [m2]"]);
        assert_eq!(s.turns.activity(), Activity::Idle);
        assert_eq!(s.turns.last_turn(), Some(&json!({ "stop_reason": "end_turn" })));
    }

    #[test]
    fn adr_0004_a_turn_ended_waits_for_its_steers_and_so_does_what_is_held() {
        let mut s = Session::new();
        s.send("m1", Mode::Prompt);
        assert_eq!(s.send("m2", Mode::Steer), ("steered", vec!["steer m2".into()]));
        assert_eq!(
            s.answered("k1"),
            Vec::<String>::new(),
            "the turn ended before its steer's answer"
        );
        assert_eq!(s.turns.activity(), Activity::Waiting);
        assert_eq!(s.send("m3", Mode::Prompt), ("held", vec![]));
        // Steered into the turn that waits, not a turn of its own.
        assert_eq!(s.send("m4", Mode::Steer), ("steered", vec!["steer m4".into()]));
        assert_eq!(s.steer_answered("m2", injected()), ["user_message m2 in k1"]);
        assert_eq!(
            s.steer_answered("m4", injected()),
            [
                "user_message m4 in k1",
                "turn_ended k1 [m1 m2 m4]",
                "prompt m3 as k2",
                "user_message m3 in k2"
            ]
        );
        // An answer to a steer no longer waited for changes nothing.
        assert_eq!(s.steer_answered("m4", injected()), Vec::<String>::new());
    }

    #[test]
    fn adr_0004_a_steer_injected_while_its_turn_runs_is_in_that_turn() {
        let mut s = Session::new();
        s.send("m1", Mode::Prompt);
        s.send("m2", Mode::Steer);
        assert_eq!(s.steer_answered("m2", injected()), ["user_message m2 in k1"]);
        assert_eq!(s.answered("k1"), ["turn_ended k1 [m1 m2]"]);
    }

    #[test]
    fn adr_0004_a_steer_turned_back_goes_next_ahead_of_what_is_held() {
        let mut s = Session::new();
        s.send("m1", Mode::Prompt);
        s.send("m2", Mode::Prompt);
        s.send("m3", Mode::Steer);
        assert_eq!(s.answered("k1"), Vec::<String>::new());
        let required = json!({ "result": { "outcome": "promptRequired" } });
        assert_eq!(
            s.steer_answered("m3", required),
            ["turn_ended k1 [m1]", "prompt m3 as k2", "user_message m3 in k2"]
        );
        assert_eq!(
            s.answered("k2"),
            ["turn_ended k2 [m3]", "prompt m2 as k3", "user_message m2 in k3"]
        );
    }

    #[test]
    fn adr_0004_a_refused_steer_is_dropped_before_its_turn_ends() {
        let mut s = Session::new();
        s.send("m1", Mode::Prompt);
        s.send("m2", Mode::Steer);
        s.answered("k1");
        let refused = json!({ "error": { "code": -32603, "message": "no" } });
        assert_eq!(
            s.steer_answered("m2", refused),
            ["note steer_refused", "dropped m2 by steer", "turn_ended k1 [m1]"]
        );
        let other = json!({ "result": { "outcome": "later" } });
        s.send("m3", Mode::Prompt);
        s.send("m4", Mode::Steer);
        assert_eq!(s.steer_answered("m4", other), ["note steer_refused", "dropped m4 by steer"]);
    }

    #[test]
    fn adr_0004_an_interrupt_while_steers_wait_is_held_with_no_turn_to_cancel() {
        let mut s = Session::new();
        s.send("m1", Mode::Prompt);
        s.send("m2", Mode::Prompt);
        s.send("m3", Mode::Steer);
        s.answered("k1");
        assert_eq!(s.turns.status(Mode::Interrupt), Status::Held);
        assert_eq!(s.send("m4", Mode::Interrupt), ("held", vec![]));
        assert_eq!(
            s.steer_answered("m3", injected()),
            [
                "user_message m3 in k1",
                "turn_ended k1 [m1 m3]",
                "prompt m4 as k2",
                "user_message m4 in k2"
            ]
        );
    }

    #[test]
    fn adr_0004_cancel_drops_what_is_held_and_unanswered_steers() {
        let mut s = Session::new();
        s.send("m1", Mode::Prompt);
        s.send("m2", Mode::Prompt);
        s.send("m3", Mode::Interrupt);
        s.send("m4", Mode::Steer);
        let (status, dropped, todo) = s.turns.cancel(true);
        assert_eq!((status, dropped), ("cancelling", vec![]));
        assert_eq!(s.run(todo), ["cancel"]);
        assert_eq!(s.turns.held(), 2);
        let (status, dropped, todo) = s.turns.cancel(false);
        assert_eq!(status, "cancelling");
        let ids: Vec<&Value> = dropped.iter().map(|d| &d["message"]).collect();
        assert_eq!(ids, ["m4", "m3", "m2"]);
        assert_eq!(
            s.run(todo),
            ["dropped m4 by cancel", "dropped m3 by cancel", "dropped m2 by cancel", "cancel"]
        );
        // The steer's late answer doesn't bring it back.
        assert_eq!(s.steer_answered("m4", injected()), Vec::<String>::new());
        assert_eq!(s.answered("k1"), ["turn_ended k1 [m1]"]);
        let (status, dropped, todo) = s.turns.cancel(false);
        assert_eq!((status, dropped.len(), todo.len()), ("idle", 0, 0));
    }

    #[test]
    fn adr_0004_a_dropped_steer_lets_its_turn_end() {
        let mut s = Session::new();
        s.send("m1", Mode::Prompt);
        s.send("m2", Mode::Steer);
        s.answered("k1");
        s.turns.add_context("see the docs".into(), false);
        let todo = s.turns.drop_all("close");
        assert_eq!(
            s.run(todo),
            ["dropped m2 by close", "turn_ended k1 [m1]", "context_dropped by close"]
        );
        assert_eq!(s.turns.activity(), Activity::Idle);
    }

    #[test]
    fn adr_0004_context_goes_with_the_next_prompt() {
        let mut s = Session::new();
        s.turns.add_context("first".into(), false);
        s.turns.add_context("second".into(), true);
        let (_, todo) = s.turns.send(held("m1"), Mode::Prompt);
        let Some(Do::Prompt(m1)) = todo.into_iter().next() else { panic!("not sent") };
        let (blocks, _) = s.turns.start("k1".into(), m1);
        assert_eq!(prompt_text(Some(&json!(blocks))), "text of m1\nsecond");
        assert!(s.turns.context().is_empty());
    }

    #[test]
    fn adr_0004_queue_drops_held_messages_and_interrupts() {
        let mut s = Session::new();
        s.send("m1", Mode::Prompt);
        s.send("m2", Mode::Prompt);
        s.send("m3", Mode::Interrupt);
        s.send("m4", Mode::Prompt);
        assert_eq!(s.turns.held_message("m3").map(|(h, i)| (h.id.as_str(), i)), Some(("m3", true)));
        assert_eq!(
            s.turns.held_message("m2").map(|(h, i)| (h.id.as_str(), i)),
            Some(("m2", false))
        );
        assert_eq!(s.turns.queue(Some("m9"), true, false).err(), Some("no held message m9".into()));
        assert_eq!(s.turns.held(), 3, "a refused drop clears nothing");
        let (dropped, todo) = s.turns.queue(Some("m3"), false, false).unwrap();
        assert_eq!((dropped.len(), s.run(todo)), (1, vec!["dropped m3 by queue".into()]));
        let (_, todo) = s.turns.queue(None, true, false).unwrap();
        assert_eq!(s.run(todo), ["dropped m2 by queue", "dropped m4 by queue"]);
        assert_eq!(s.answered("k1"), ["turn_ended k1 [m1]"]);
    }

    #[test]
    fn adr_0004_the_editors_turns_are_its_own() {
        let mut s = Session::new();
        let sent = s.turns.editor_prompt("e1", Some(&json!([{ "type": "text", "text": "hi" }])));
        assert_eq!(s.run(vec![sent]), ["user_message editor in e1"]);
        let steered = s.turns.editor_steered("more".into());
        assert_eq!(s.run(vec![steered]), ["user_message editor in e1"]);
        let todo = s.turns.answered("e1", json!({ "stop_reason": "end_turn" }));
        let Some(Do::Emit(ended)) = todo.first() else { panic!("no turn_ended") };
        assert_eq!((&ended["by"], &ended["messages"]), (&json!("editor"), &json!([])));
        let late = s.turns.editor_steered("late".into());
        assert_eq!(s.run(vec![late]), ["note steer_after_turn"]);
    }
}
