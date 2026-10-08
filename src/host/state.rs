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
//!
//! Updates are read as the schema's `SessionUpdate`s (see schema.rs); one
//! it doesn't take, an adapter's own kind or a newer one, changes nothing
//! here. What brnr passes on as the agent said it stays the agent's JSON:
//! a tool call's locations, plan entries, a cost, config options and
//! commands, and the modes and config options of a result.

use serde::Serialize;
use serde_json::{Map, Value, json};

use super::Host;
use crate::schema::{self, SessionUpdate, ToolCallStatus, ToolKind};

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
    pub(super) commands: Vec<Value>,
    pub(super) plan: Option<Value>,
    pub(super) usage: Option<Value>,
    /// Tool calls that haven't finished.
    pub(super) tools: Vec<Tool>,
    pub(super) last_message: Option<String>,
}

/// A tool call as the tool events and `brnr status` have it.
#[derive(Clone, Serialize)]
pub(super) struct Tool {
    tool_call_id: String,
    title: String,
    kind: ToolKind,
    status: ToolCallStatus,
    /// As the agent gave them.
    locations: Value,
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

    /// The config option of ACP's `category` (`mode`, `model`, …): by
    /// category only, an option's id being the agent's own (ADR 28).
    pub(super) fn option(&self, category: &str) -> Option<&Value> {
        self.config.as_ref()?.as_array()?.iter().find(|o| o["category"] == category)
    }

    pub(super) fn current_model(&self) -> Option<String> {
        self.option("model")?["currentValue"].as_str().map(str::to_owned)
    }

    pub(super) fn report(&self) -> Value {
        json!({
            "title": self.title,
            "mode": self.current_mode(),
            "modes": self.modes.as_ref().map(|m| m["availableModes"].clone()),
            "model": self.current_model(),
            "config": self.config,
            "commands": self.commands,
            "plan": self.plan,
            "usage": self.usage,
            "tools": self.tools,
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

    /// Tracks one `session/update` that isn't agent text, `raw` as the agent
    /// sent it, emitting the events that changes to it make.
    pub(super) fn track_state(&mut self, i: usize, update: &SessionUpdate, raw: &Value) {
        if let Some(event) = self.state_change(i, update, raw) {
            self.emit(event);
        }
    }

    /// What a `session/load` replays: history, which the transcript has
    /// already, but for what says how the session is now (its title, mode,
    /// config options and commands).
    pub(super) fn replayed_state(&mut self, i: usize, raw: &Value) {
        let Some(update) = schema::read::<SessionUpdate>(raw) else { return };
        if matches!(
            update,
            SessionUpdate::SessionInfoUpdate(_)
                | SessionUpdate::CurrentModeUpdate(_)
                | SessionUpdate::ConfigOptionUpdate(_)
                | SessionUpdate::AvailableCommandsUpdate(_)
        ) {
            self.state_change(i, &update, raw);
        }
    }

    /// Takes in one update; returns the event it makes, if any.
    fn state_change(&mut self, i: usize, update: &SessionUpdate, raw: &Value) -> Option<Value> {
        let session = self.sessions[i].id.clone();
        let state = &mut self.sessions[i].state;
        let event = match update {
            SessionUpdate::ToolCall(call) => {
                let tool = Tool {
                    tool_call_id: call.tool_call_id.to_string(),
                    title: call.title.clone(),
                    kind: call.kind,
                    status: call.status,
                    locations: raw.get("locations").cloned().unwrap_or(json!([])),
                };
                if !finished(tool.status) {
                    state.tools.push(tool.clone());
                }
                let mut event = tool_event(&session, &tool, "tool_call");
                event["started"] = json!(true);
                event
            }
            SessionUpdate::ToolCallUpdate(update) => {
                let id = update.tool_call_id.to_string();
                let Some(pos) = state.tools.iter().position(|t| t.tool_call_id == id) else {
                    return None; // A call we never saw start, or already finished.
                };
                let tool = &mut state.tools[pos];
                let fields = &update.fields;
                if let Some(title) = &fields.title {
                    tool.title = title.clone();
                }
                if let Some(kind) = fields.kind {
                    tool.kind = kind;
                }
                if let Some(locations) = raw.get("locations").filter(|v| !v.is_null()) {
                    tool.locations = locations.clone();
                }
                let Some(status) = fields.status.filter(|s| *s != tool.status) else {
                    return None; // Progress, not a change of status.
                };
                tool.status = status;
                // `tool_call` again at the end, `tool_progress` on the way.
                let finished = finished(status);
                let name = if finished { "tool_call" } else { "tool_progress" };
                let event = tool_event(&session, tool, name);
                if finished {
                    state.tools.remove(pos);
                }
                event
            }
            SessionUpdate::Plan(_) => {
                state.plan = Some(raw["entries"].clone());
                json!({ "event": "plan", "session": session, "entries": raw["entries"] })
            }
            SessionUpdate::UsageUpdate(usage) => {
                let usage = json!({ "used": usage.used, "size": usage.size, "cost": raw["cost"] });
                state.usage = Some(usage.clone());
                json!({ "event": "usage", "session": session, "usage": usage })
            }
            SessionUpdate::SessionInfoUpdate(info) => {
                let title = info.title.value()?;
                state.title = Some(title.clone());
                changed(&session, "title", json!(title))
            }
            SessionUpdate::CurrentModeUpdate(update) => {
                let mode = update.current_mode_id.to_string();
                state.set_mode(&mode);
                changed(&session, "mode", json!(mode))
            }
            SessionUpdate::ConfigOptionUpdate(_) => {
                let config = state.set_config(&raw["configOptions"])?;
                changed(&session, "config", config)
            }
            SessionUpdate::AvailableCommandsUpdate(_) => {
                let commands = raw["availableCommands"].as_array().cloned().unwrap_or_default();
                let added = patch(&state.commands, &commands, "name", None);
                state.commands = commands;
                changed(&session, "commands", added?)
            }
            _ => return None,
        };
        Some(event)
    }
}

/// Whether a tool call with `status` is no longer running.
fn finished(status: ToolCallStatus) -> bool {
    matches!(status, ToolCallStatus::Completed | ToolCallStatus::Failed)
}

fn preview(text: &str) -> String {
    match text.char_indices().nth(LAST_MESSAGE_PREVIEW) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.to_owned(),
    }
}

fn tool_event(session: &str, tool: &Tool, name: &str) -> Value {
    let mut event = json!(tool);
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
