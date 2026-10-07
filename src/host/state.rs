//! What the agent has told the host about each session, from its
//! `session/update`s and from the results of the requests that open or
//! change a session: title, mode, config options, plan, usage, running tool
//! calls. `brnr status` shows it, and the host turns changes into events.
//!
//! A `session_changed` says what changed (ADR 22 in docs/adr): the title or
//! the mode, or of the config options and the commands, a JSON merge patch
//! (RFC 7396) over them by id and name: `{"model": "opus"}` for an option's
//! value, `{"review": {…}}` for a command added, `null` for what is gone.
//! `brnr status` has them in full.

use serde_json::{Map, Value, json};

use super::Host;

/// Tool call statuses after which the call is no longer running.
const FINISHED: &[&str] = &["completed", "failed"];

/// Updates that say how the session is (its title, mode, config options
/// and commands) rather than what happened in it.
const STATE_UPDATES: &[&str] = &[
    "session_info_update",
    "current_mode_update",
    "config_option_update",
    "available_commands_update",
];

/// How much of the last agent message the status report has; the events
/// have all of it.
const LAST_MESSAGE_PREVIEW: usize = 4000;

#[derive(Default)]
pub(super) struct SessionState {
    pub(super) title: Option<String>,
    /// `{currentModeId, availableModes}` as the agent gave it.
    pub(super) modes: Option<Value>,
    /// The agent's `configOptions`.
    pub(super) config: Option<Value>,
    /// `{currentModelId, availableModels}` (an older, unstable ACP API).
    pub(super) models: Option<Value>,
    pub(super) commands: Vec<Value>,
    pub(super) plan: Option<Value>,
    pub(super) usage: Option<Value>,
    /// Tool calls that haven't finished: id → `{title, kind, status}`.
    pub(super) tools: Vec<(String, Value)>,
    pub(super) last_message: Option<String>,
}

impl SessionState {
    /// Takes what a result that opens or changes a session says about it
    /// (`session/new`, `load`, `resume`, `fork`, `set_config_option`, …).
    /// Returns what it changed of the config options (see the module docs).
    pub(super) fn result(&mut self, result: &Value) -> Option<Value> {
        if let Some(modes) = result.get("modes").filter(|m| m.is_object()) {
            self.modes = Some(modes.clone());
        }
        let mut changed = None;
        if let Some(config) = result.get("configOptions").filter(|c| c.is_array()) {
            changed = self.set_config(config);
        }
        if let Some(models) = result.get("models").filter(|m| m.is_object()) {
            self.models = Some(models.clone());
        }
        changed
    }

    /// Takes the config options; returns what changed of their values.
    fn set_config(&mut self, config: &Value) -> Option<Value> {
        let old = self.config.as_ref().and_then(Value::as_array).map_or(&[][..], Vec::as_slice);
        let new = config.as_array().map_or(&[][..], Vec::as_slice);
        let changed = patch(old, new, "id", Some("currentValue"));
        self.config = Some(config.clone());
        changed
    }

    pub(super) fn current_mode(&self) -> Option<&str> {
        self.modes.as_ref()?["currentModeId"].as_str()
    }

    pub(super) fn set_mode(&mut self, mode: &str) {
        if let Some(modes) = &mut self.modes {
            modes["currentModeId"] = json!(mode);
        }
    }

    /// The config option for the model: id or category `model`.
    pub(super) fn model_option(&self) -> Option<&Value> {
        self.config
            .as_ref()?
            .as_array()?
            .iter()
            .find(|o| o["id"] == "model" || o["category"] == "model")
    }

    pub(super) fn current_model(&self) -> Option<String> {
        match self.model_option() {
            Some(option) => option["currentValue"].as_str().map(str::to_owned),
            None => self.models.as_ref()?["currentModelId"].as_str().map(str::to_owned),
        }
    }

    pub(super) fn report(&self) -> Value {
        json!({
            "title": self.title,
            "mode": self.current_mode(),
            "modes": self.modes.as_ref().map(|m| m["availableModes"].clone()),
            "model": self.current_model(),
            "config": self.config,
            "models": self.models.as_ref().map(|m| m["availableModels"].clone()),
            "commands": self.commands,
            "plan": self.plan,
            "usage": self.usage,
            "tools": self.tools.iter().map(|(_, t)| t).collect::<Vec<_>>(),
            "last_message": self.last_message.as_deref().map(preview),
        })
    }
}

impl Host {
    /// Takes the result of a request that changes session `i`
    /// (`set_config_option`, `set_mode`): a change to its config options is
    /// a `session_changed`, as an update saying so would be.
    pub(super) fn apply_result(&mut self, i: usize, result: &Value) {
        if let Some(config) = self.sessions[i].state.result(result) {
            let session = self.sessions[i].id.clone();
            self.emit(changed(&session, "config", config));
        }
    }

    /// Tracks one `session/update` that isn't agent text, emitting the
    /// events that changes to it make.
    pub(super) fn track_state(&mut self, i: usize, kind: &str, update: &Value) {
        if let Some(event) = self.state_change(i, kind, update) {
            self.emit(event);
        }
    }

    /// What a `session/load` replays: history, which the transcript has
    /// already, but for what says how the session is now.
    pub(super) fn replayed_state(&mut self, i: usize, update: &Value) {
        let kind = update["sessionUpdate"].as_str().unwrap_or_default();
        if STATE_UPDATES.contains(&kind) {
            self.state_change(i, kind, update);
        }
    }

    /// Takes in one update; returns the event it makes, if any.
    fn state_change(&mut self, i: usize, kind: &str, update: &Value) -> Option<Value> {
        let session = self.sessions[i].id.clone();
        let state = &mut self.sessions[i].state;
        let event = match kind {
            "tool_call" => {
                let id = update["toolCallId"].as_str().unwrap_or_default().to_owned();
                let tool = json!({
                    "tool_call_id": id,
                    "title": update["title"],
                    "kind": update.get("kind").cloned().unwrap_or(json!("other")),
                    "status": update.get("status").cloned().unwrap_or(json!("pending")),
                    "locations": update.get("locations").cloned().unwrap_or(json!([])),
                });
                let finished = FINISHED.contains(&tool["status"].as_str().unwrap_or_default());
                if !finished {
                    state.tools.push((id, tool.clone()));
                }
                let mut event = tool_event(&session, &tool, "tool_call");
                event["started"] = json!(true);
                event
            }
            "tool_call_update" => {
                let id = update["toolCallId"].as_str().unwrap_or_default();
                let Some(pos) = state.tools.iter().position(|(t, _)| t == id) else {
                    return None; // A call we never saw start, or already finished.
                };
                let tool = &mut state.tools[pos].1;
                for (field, key) in
                    [("title", "title"), ("kind", "kind"), ("locations", "locations")]
                {
                    if let Some(value) = update.get(key).filter(|v| !v.is_null()) {
                        tool[field] = value.clone();
                    }
                }
                let Some(status) = update["status"].as_str().filter(|s| *s != tool["status"])
                else {
                    return None; // Progress, not a change of status.
                };
                tool["status"] = json!(status);
                // `tool_call` again at the end, `tool_progress` on the way.
                let finished = FINISHED.contains(&status);
                let name = if finished { "tool_call" } else { "tool_progress" };
                let event = tool_event(&session, tool, name);
                if finished {
                    state.tools.remove(pos);
                }
                event
            }
            "plan" => {
                state.plan = Some(update["entries"].clone());
                json!({ "event": "plan", "session": session, "entries": update["entries"] })
            }
            "usage_update" => {
                let usage = json!({ "used": update["used"], "size": update["size"], "cost": update["cost"] });
                state.usage = Some(usage.clone());
                json!({ "event": "usage", "session": session, "usage": usage })
            }
            "session_info_update" => {
                let title = update["title"].as_str()?;
                state.title = Some(title.to_owned());
                changed(&session, "title", json!(title))
            }
            "current_mode_update" => {
                let mode = update["currentModeId"].as_str()?;
                state.set_mode(mode);
                changed(&session, "mode", json!(mode))
            }
            "config_option_update" => {
                let config = state.set_config(&update["configOptions"])?;
                changed(&session, "config", config)
            }
            "available_commands_update" => {
                let commands = update["availableCommands"].as_array().cloned().unwrap_or_default();
                let added = patch(&state.commands, &commands, "name", None);
                state.commands = commands;
                changed(&session, "commands", added?)
            }
            _ => return None,
        };
        Some(event)
    }
}

fn preview(text: &str) -> String {
    match text.char_indices().nth(LAST_MESSAGE_PREVIEW) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.to_owned(),
    }
}

fn tool_event(session: &str, tool: &Value, name: &str) -> Value {
    let mut event = tool.clone();
    event["event"] = json!(name);
    event["session"] = json!(session);
    event
}

fn changed(session: &str, what: &str, value: Value) -> Value {
    json!({ "event": "session_changed", "session": session, "what": what, "value": value })
}

/// What changed from `old` to `new`, lists of objects named by their `key`,
/// as a merge patch over the map from each name to its `field` (the whole
/// object without one): each name that is new or whose value is different,
/// with its value, then `null` for each name gone. `None` if nothing did.
fn patch(old: &[Value], new: &[Value], key: &str, field: Option<&str>) -> Option<Value> {
    fn value<'a>(o: &'a Value, field: Option<&str>) -> &'a Value {
        field.map_or(o, |f| &o[f])
    }
    let named = |list: &'_ [Value], name: &str| list.iter().position(|o| o[key] == name);
    let mut patch = Map::new();
    for o in new {
        let Some(name) = o[key].as_str() else { continue };
        let before = named(old, name).map(|j| value(&old[j], field));
        if before != Some(value(o, field)) {
            patch.insert(name.to_owned(), value(o, field).clone());
        }
    }
    for o in old {
        if let Some(name) = o[key].as_str()
            && named(new, name).is_none()
        {
            patch.insert(name.to_owned(), Value::Null);
        }
    }
    (!patch.is_empty()).then_some(Value::Object(patch))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_patch_has_what_changed() {
        let option =
            |id: &str, value: Value| json!({ "id": id, "currentValue": value, "options": [] });
        let old = [option("model", json!("small")), option("fast", json!(false))];
        let new = [
            option("model", json!("large")),
            option("fast", json!(false)),
            option("effort", json!("high")),
        ];
        let config = |old: &[Value], new: &[Value]| patch(old, new, "id", Some("currentValue"));
        assert_eq!(config(&old, &new), Some(json!({ "model": "large", "effort": "high" })));
        assert_eq!(config(&new, &old), Some(json!({ "model": "small", "effort": null })));
        assert_eq!(config(&old, &old), None);
        let compact = json!({ "name": "compact", "description": "Compact" });
        let review = json!({ "name": "review", "description": "Review" });
        let reworded = json!({ "name": "review", "description": "Review it" });
        let commands =
            |old, new| patch(std::slice::from_ref(old), std::slice::from_ref(new), "name", None);
        let shown = commands(&compact, &review);
        assert_eq!(shown, Some(json!({ "review": review, "compact": null })));
        assert_eq!(commands(&review, &reworded), Some(json!({ "review": reworded })));
        assert_eq!(commands(&compact, &compact), None);
    }
}
