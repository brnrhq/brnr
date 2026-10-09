//! ACP's types, from the official schema crate (ADR 43 in docs/adr).
//!
//! brnr forwards each ACP message as it came and interprets a copy, read as
//! json.rs reads a line (ADR 26). What it acts on in the copy is read with
//! `agent-client-protocol-schema`'s types: [`read`] takes a part of the
//! copy as the type its method gives it, a session update, a capability, a
//! permission option. A part the types don't take stays `serde_json::Value`,
//! read as far as brnr can (P4), and is forwarded untouched all the same:
//!
//! - the JSON-RPC envelope: ids of any JSON type, kept as the peer sent
//!   them (the crate's `RequestId` is an `i64`, a string or null), and each
//!   method's `sessionId`, whatever the method;
//! - the editor's requests, of which brnr reads the session and the cwd
//!   whatever else they carry or leave out;
//! - extension methods and `_meta` (`_session/steering` and how agents
//!   advertise it), and what isn't stable ACP (`session/fork`, the `fork`
//!   session capability);
//! - session update kinds the schema doesn't have (an adapter's own, an
//!   unstable or a newer one), or a known kind without what the schema
//!   requires of it: such an update is passed on unread;
//! - what brnr passes on as it was said, in events, the status and its own
//!   messages to the editor: a prompt's content, a tool call in a permission
//!   request and its options, a tool call's locations, plan entries, config
//!   options, commands, modes, a turn's stop reason, the editor's answers;
//! - what the host rewrites in what it forwards (the editor's
//!   `initialize`, a prompt it appends context to, secrets it redacts in
//!   what it records), so the editor's key order and unknown fields stay.
//!
//! The crate is built without its unstable features, so its types and its
//! methods are stable ACP: what strict mode holds to (ADR 41, see
//! [`stable`]).

use agent_client_protocol_schema::v1::{
    AGENT_METHOD_NAMES, CLIENT_METHOD_NAMES, PROTOCOL_LEVEL_METHOD_NAMES,
};
use serde::de::DeserializeOwned;
use serde_json::Value;

pub use agent_client_protocol_schema::v1::{
    AgentCapabilities, AuthMethod, ContentBlock, Error, ErrorCode, ListSessionsResponse, MessageId,
    NewSessionResponse, PermissionOptionKind, SessionConfigOptionValue, SessionInfo, SessionUpdate,
    SetSessionConfigOptionRequest, ToolCallStatus, ToolKind,
};

/// `value`, a part of a message as json.rs read it, as the schema's `T`, if
/// it is one. Run it on a stack that takes `value` (see json.rs).
pub fn read<T: DeserializeOwned>(value: &Value) -> Option<T> {
    T::deserialize(value).ok()
}

/// Whether `method` is stable ACP: one in the schema's tables of method
/// names, which have an unstable method only with its feature, none of
/// which brnr builds. An extension method (`_…`) isn't stable, nor is one
/// the schema still marks unstable (`session/fork`).
pub fn stable(method: &str) -> bool {
    let names = [
        serde_json::to_value(AGENT_METHOD_NAMES),
        serde_json::to_value(CLIENT_METHOD_NAMES),
        serde_json::to_value(PROTOCOL_LEVEL_METHOD_NAMES),
    ];
    names
        .iter()
        .flatten()
        .filter_map(Value::as_object)
        .flat_map(|m| m.values())
        .any(|m| m == method)
}

/// A JSON-RPC error's message, or the whole error if it isn't one (with a
/// code and a message).
pub fn error_message(error: &Value) -> String {
    read::<Error>(error).map_or_else(|| error.to_string(), |e| e.message)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::json;

    /// Session updates as claude-agent-acp 0.85.1 and codex-acp 2.1.1 send
    /// them, unless the client asks for more (notices, compaction,
    /// subagents).
    fn adapter_updates() -> Vec<Value> {
        let text = json!({ "type": "text", "text": "hi" });
        vec![
            json!({ "sessionUpdate": "agent_message_chunk", "content": text, "messageId": "m1",
                    "_meta": { "claudeCode": { "parentToolUseId": "t0" } } }),
            json!({ "sessionUpdate": "agent_thought_chunk", "content": text }),
            // claude: an image by URL has no data.
            json!({ "sessionUpdate": "user_message_chunk",
                    "content": { "type": "image", "data": "", "mimeType": "", "uri": "https://x/a.png" } }),
            // codex: a user's attachment in the history.
            json!({ "sessionUpdate": "user_message_chunk", "messageId": "item-1",
                    "content": { "type": "resource_link", "name": "a.rs", "uri": "file:///a.rs" } }),
            json!({ "sessionUpdate": "tool_call", "toolCallId": "t1", "name": "Bash", "title": "ls",
                    "kind": "execute", "status": "pending", "rawInput": { "command": "ls" },
                    "content": [{ "type": "terminal", "terminalId": "t1" }],
                    "locations": [{ "path": "/a" }],
                    "_meta": { "claudeCode": { "toolName": "Bash" }, "terminal_info": { "cwd": "/" } } }),
            // codex: a title it hasn't got yet is "".
            json!({ "sessionUpdate": "tool_call", "toolCallId": "t2", "title": "" }),
            // claude: a Write without content leaves its diff without newText.
            json!({ "sessionUpdate": "tool_call_update", "toolCallId": "t1", "status": "completed",
                    "content": [{ "type": "diff", "path": "/a", "oldText": null }],
                    "rawOutput": "ok", "_meta": { "terminal_exit": { "exit_code": 0, "signal": null } } }),
            json!({ "sessionUpdate": "plan",
                    "entries": [{ "content": "Fix it", "status": "in_progress", "priority": "medium" }] }),
            json!({ "sessionUpdate": "available_commands_update",
                    "availableCommands": [{ "name": "review", "description": "", "input": null },
                                          { "name": "$skill", "description": "A skill", "input": { "hint": "what" } }] }),
            json!({ "sessionUpdate": "current_mode_update", "currentModeId": "plan" }),
            // codex: a category of its own.
            json!({ "sessionUpdate": "config_option_update", "configOptions": [
                { "id": "collaboration_mode", "name": "Collaboration", "category": "collaboration_mode",
                  "type": "select", "currentValue": "default",
                  "options": [{ "value": "default", "name": "Default" }, { "value": "plan", "name": "Plan" }] },
                { "id": "fast", "name": "Fast", "category": "model_config", "type": "boolean", "currentValue": false }] }),
            json!({ "sessionUpdate": "session_info_update", "title": "", "updatedAt": "2026-10-07T10:00:00Z" }),
            // codex: a title it cleared, and its thread's status.
            json!({ "sessionUpdate": "session_info_update", "title": null }),
            json!({ "sessionUpdate": "session_info_update", "_meta": { "codex": { "threadStatus": "idle" } } }),
            json!({ "sessionUpdate": "usage_update", "used": 12345, "size": 200000,
                    "cost": { "amount": 0.42, "currency": "USD" }, "_meta": { "_claude/model": "opus" } }),
        ]
    }

    #[test]
    fn what_the_adapters_send_is_read_as_the_schemas_types() {
        for update in adapter_updates() {
            assert!(read::<SessionUpdate>(&update).is_some(), "{update}");
        }
        let info = read::<SessionUpdate>(
            &json!({ "sessionUpdate": "session_info_update", "title": null }),
        );
        assert!(
            matches!(info, Some(SessionUpdate::SessionInfoUpdate(i)) if i.title.value().is_none())
        );
        let options = [
            json!({ "optionId": "allow-once", "name": "Allow", "kind": "allow_once" }),
            json!({ "optionId": "implement_plan", "name": "Implement", "kind": "allow_always" }),
            json!({ "optionId": "reject", "name": "Reject", "kind": "reject_once" }),
            json!({ "optionId": "never", "name": "Never", "kind": "reject_always" }),
        ];
        for option in options {
            assert!(read::<PermissionOptionKind>(&option["kind"]).is_some(), "{option}");
        }
        let list = json!({ "sessions": [
            { "sessionId": "a", "cwd": "/w", "title": "A", "updatedAt": "2026-10-07T10:00:00Z" },
            { "sessionId": "b", "cwd": "/w", "title": null, "updatedAt": "2026-10-07T10:00:00Z",
              "additionalDirectories": ["/x"] }],
            "nextCursor": null });
        let list = read::<ListSessionsResponse>(&list).unwrap();
        assert_eq!((list.sessions.len(), list.next_cursor), (2, None));
        let auth = [
            json!({ "id": "claude-ai-login", "name": "Log in", "type": "terminal",
                    "args": ["--cli", "auth", "login", "--claudeai"] }),
            json!({ "id": "api-key", "name": "API key", "_meta": { "api-key": { "provider": "openai" } } }),
        ];
        for method in auth {
            assert!(read::<AuthMethod>(&method).is_some(), "{method}");
        }
        let error = json!({ "code": -32000, "message": "Authentication required" });
        assert_eq!(read::<Error>(&error).map(|e| e.code), Some(ErrorCode::AuthRequired));
        assert_eq!(error_message(&error), "Authentication required");
        assert_eq!(error_message(&json!("boom")), r#""boom""#);
    }

    #[test]
    fn a_deep_part_reads_on_the_stack_json_rs_gives_it() {
        let n = 20_000;
        let input = format!("{}1{}", r#"{"a":"#.repeat(n), "}".repeat(n));
        let line = format!(
            r#"{{"sessionUpdate":"tool_call","toolCallId":"t","title":"deep","rawInput":{input}}}"#
        );
        let line = line.as_bytes();
        let read = json::on_stack(json::depth(line), || {
            let value: Value = json::parse(line).unwrap();
            matches!(read::<SessionUpdate>(&value), Some(SessionUpdate::ToolCall(c)) if c.title == "deep")
        });
        assert_eq!(read, Some(true));
    }

    #[test]
    fn what_the_types_dont_take_is_left_to_value() {
        // Kinds the adapters send only to a client that asks for them,
        // unstable or their own, and one from a newer schema.
        let unknown = [
            json!({ "sessionUpdate": "notice", "severity": "info", "title": "Note" }),
            json!({ "sessionUpdate": "compaction_update", "compactionId": "c", "status": "completed" }),
            json!({ "sessionUpdate": "plan_update",
                    "plan": { "type": "markdown", "planId": "p", "content": "# Plan" } }),
            json!({ "sessionUpdate": "subagent_spawned", "subagentSessionId": "s2", "name": "n",
                    "task": "t", "capabilities": {} }),
            json!({ "sessionUpdate": "async_task_state_update", "asyncTaskId": "a", "state": "done" }),
            json!({ "sessionUpdate": "something_newer", "text": "hi" }),
        ];
        for update in unknown {
            assert!(read::<SessionUpdate>(&update).is_none(), "{update}");
        }
        assert!(read::<PermissionOptionKind>(&json!("allow_for_session")).is_none());
        // A stop reason the schema doesn't have yet: brnr's events carry it
        // as the agent said it.
        assert!(read::<agent_client_protocol_schema::v1::StopReason>(&json!("paused")).is_none());
    }

    #[test]
    fn stable_is_what_the_schema_has_without_its_unstable_features() {
        // ADR 41: in v1, list, resume, close and delete are stable, fork
        // isn't; `$/cancel_request` is; extension methods never are.
        for method in [
            "initialize",
            "session/new",
            "session/load",
            "session/resume",
            "session/list",
            "session/close",
            "session/delete",
            "session/prompt",
            "session/cancel",
            "session/set_mode",
            "session/set_config_option",
            "session/update",
            "session/request_permission",
            "fs/read_text_file",
            "terminal/create",
            "$/cancel_request",
        ] {
            assert!(stable(method), "{method}");
        }
        for method in ["session/fork", "session/set_model", "_session/steering", "_fake/noise"] {
            assert!(!stable(method), "{method}");
        }
    }
}
