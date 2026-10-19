//! Elicitations (ADR 4.5 in docs/adr): the agent's `elicitation/create`, a
//! form or a URL for the user, held as a permission request is held and
//! answered with ACP's own actions, `accept`, `decline` or `cancel`.
//!
//! What brnr reads of a request it reads with the schema's types (ADR 2.3),
//! and as far as it can when they don't take it (P4): the mode, the message,
//! the scope (a session, a tool call in it, or a request outside any), a
//! form's `requestedSchema` and a URL mode's `url` and `elicitationId`.
//! An accept's content is checked against the form's schema here, once,
//! before anything is sent (P4, P7): every field named in it, the required
//! ones given, each value of its type (a string from `--field` read as the
//! type asks), within its bounds and one of its choices. `pattern` and
//! `format` are passed on as given: brnr has no regex engine, and the agent
//! checks them itself. The response is written with the schema's types, so
//! the content has ACP's value types.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use super::acp::AgentRequest;
use crate::schema::{
    self, CreateElicitationRequest, CreateElicitationResponse, ElicitationAcceptAction,
    ElicitationAction, ElicitationContentValue, ElicitationMode, ElicitationPropertySchema,
    ElicitationSchema, ElicitationScope, MultiSelectItems,
};

/// What the host keeps of a URL elicitation it holds or has accepted,
/// until the agent says its flow is complete.
pub(super) struct UrlElicitation {
    pub(super) session: Option<String>,
    pub(super) handle: String,
}

/// A request as the events and `pending` describe it: `mode`, `message`,
/// `scope` (`session`, `tool_call` or `request`) with the `tool_call` or
/// `scope_request` it names, a form's `schema`, a URL's `url` and
/// `elicitation_id`. A request the schema doesn't take is described as far
/// as its fields allow.
pub(super) fn describe(req: &AgentRequest) -> Value {
    let params = &req.params;
    let mut d = json!({
        "mode": params["mode"],
        "message": params["message"],
        "scope": if params.get("requestId").is_some() { "request" } else { "session" },
        "tool_call": params["toolCallId"],
        "scope_request": params["requestId"],
        "schema": params["requestedSchema"],
        "url": params["url"],
        "elicitation_id": params["elicitationId"],
    });
    if let Some(read) = schema::read::<CreateElicitationRequest>(params) {
        let (scope, mode) = match &read.mode {
            ElicitationMode::Form(form) => (&form.scope, "form".to_owned()),
            ElicitationMode::Url(url) => (&url.scope, "url".to_owned()),
            ElicitationMode::Other(other) => (&other.scope, other.mode.clone()),
            _ => return d,
        };
        d["mode"] = json!(mode);
        d["message"] = json!(read.message);
        match scope {
            ElicitationScope::Session(s) => {
                d["scope"] = json!(if s.tool_call_id.is_some() { "tool_call" } else { "session" });
                d["tool_call"] = json!(s.tool_call_id);
                d["scope_request"] = Value::Null;
            }
            ElicitationScope::Request(r) => {
                d["scope"] = json!("request");
                d["tool_call"] = Value::Null;
                d["scope_request"] = json!(r.request_id);
            }
            _ => {}
        }
    }
    d
}

/// The URL elicitation's id, if it is one.
pub(super) fn elicitation_id(req: &AgentRequest) -> Option<String> {
    req.params["elicitationId"].as_str().map(str::to_owned)
}

/// `elicitation_request` for `req`, held in a `headless` process or an
/// `editor`'s.
pub(super) fn request_event(req: &AgentRequest, owner: &str) -> Value {
    let mut event = json!({
        "event": "elicitation_request",
        "session": req.session,
        "request": req.handle,
        "owner": owner,
    });
    merge(&mut event, describe(req));
    event
}

/// `elicitation_resolved` for `req`, answered with `action` (ACP's:
/// `accept`, `decline`, `cancel`, or whatever an editor sent) and, for an
/// accept, its `content`, by `by`.
pub(super) fn resolved_event(req: &AgentRequest, result: &Value, by: &str) -> Value {
    json!({
        "event": "elicitation_resolved",
        "session": req.session,
        "request": req.handle,
        "action": result["action"],
        "content": result["content"],
        "by": by,
    })
}

/// The response for `action`: `accept` with `content` checked against the
/// form's schema, `decline` or `cancel`. What can't be sent says why.
pub(super) fn response(req: &AgentRequest, action: &str, content: &Value) -> Result<Value, String> {
    let handle = req.handle.as_deref().unwrap_or("?");
    let action = match action {
        "decline" => ElicitationAction::Decline,
        "cancel" => ElicitationAction::Cancel,
        "accept" => {
            let given = match content {
                Value::Null => serde_json::Map::new(),
                Value::Object(fields) => fields.clone(),
                _ => return Err(format!("{handle}: the content must be an object of fields")),
            };
            let mode = describe(req)["mode"].as_str().unwrap_or("?").to_owned();
            let content = match mode.as_str() {
                "form" => {
                    let schema = schema::read::<ElicitationSchema>(&req.params["requestedSchema"])
                        .ok_or_else(|| {
                            format!(
                                "{handle}: the form's schema can't be read; decline or cancel it"
                            )
                        })?;
                    Some(check(handle, &schema, given)?)
                }
                "url" if given.is_empty() => None,
                "url" => {
                    return Err(format!(
                        "{handle} asks to open a URL, not for fields: accept it without --field"
                    ));
                }
                other => {
                    return Err(format!(
                        "{handle}'s mode {other} isn't one brnr can accept; decline or cancel it"
                    ));
                }
            };
            ElicitationAction::Accept(ElicitationAcceptAction::new().content(content))
        }
        other => return Err(format!("the action is accept, decline or cancel, not {other}")),
    };
    serde_json::to_value(CreateElicitationResponse::new(action)).map_err(|e| e.to_string())
}

/// `given` as the schema's content, or the first thing wrong with it,
/// naming the field.
fn check(
    handle: &str,
    schema: &ElicitationSchema,
    given: serde_json::Map<String, Value>,
) -> Result<BTreeMap<String, ElicitationContentValue>, String> {
    let fields = || {
        let names: Vec<&str> = schema.properties.keys().map(String::as_str).collect();
        if names.is_empty() {
            "no fields".to_owned()
        } else {
            format!("fields: {}", names.join(", "))
        }
    };
    let mut content = BTreeMap::new();
    for (name, value) in given {
        let Some(property) = schema.properties.get(&name) else {
            return Err(format!("{handle}: the form has no field {name} ({})", fields()));
        };
        let value =
            check_value(property, &value).map_err(|why| format!("{handle}: {name} {why}"))?;
        content.insert(name, value);
    }
    for name in schema.required.iter().flatten() {
        if !content.contains_key(name) {
            return Err(format!("{handle}: {name} is required ({})", fields()));
        }
    }
    Ok(content)
}

/// `value` as `property`'s type, read from a string where it came as one,
/// within its bounds and one of its choices; else what is wrong, said of
/// the field (`must be an integer, not "x"`).
fn check_value(
    property: &ElicitationPropertySchema,
    value: &Value,
) -> Result<ElicitationContentValue, String> {
    let shown = |v: &Value| {
        let s = v.to_string();
        if s.chars().count() > 40 {
            format!("{}…", s.chars().take(40).collect::<String>())
        } else {
            s
        }
    };
    let choice = |value: &str, choices: &[String]| {
        if choices.iter().any(|c| c == value) {
            Ok(())
        } else {
            Err(format!("must be one of {}, not {value:?}", choices.join(", ")))
        }
    };
    match property {
        ElicitationPropertySchema::String(p) => {
            let Some(s) = value.as_str() else {
                return Err(format!("must be text, not {}", shown(value)));
            };
            let n = s.chars().count();
            if let Some(min) = p.min_length.filter(|&min| n < min as usize) {
                return Err(format!("is {n} characters; at least {min}"));
            }
            if let Some(max) = p.max_length.filter(|&max| n > max as usize) {
                return Err(format!("is {n} characters; at most {max}"));
            }
            if let Some(values) = &p.enum_values {
                choice(s, values)?;
            }
            if let Some(options) = &p.one_of {
                let values: Vec<String> = options.iter().map(|o| o.value.clone()).collect();
                choice(s, &values)?;
            }
            Ok(ElicitationContentValue::String(s.to_owned()))
        }
        ElicitationPropertySchema::Integer(p) => {
            let i = match value {
                Value::Number(n) => n.as_i64(),
                Value::String(s) => s.trim().parse().ok(),
                _ => None,
            }
            .ok_or_else(|| format!("must be an integer, not {}", shown(value)))?;
            if let Some(min) = p.minimum.filter(|&min| i < min) {
                return Err(format!("is {i}; at least {min}"));
            }
            if let Some(max) = p.maximum.filter(|&max| i > max) {
                return Err(format!("is {i}; at most {max}"));
            }
            Ok(ElicitationContentValue::Integer(i))
        }
        ElicitationPropertySchema::Number(p) => {
            let x = match value {
                Value::Number(n) => n.as_f64(),
                Value::String(s) => s.trim().parse().ok().filter(|x: &f64| x.is_finite()),
                _ => None,
            }
            .ok_or_else(|| format!("must be a number, not {}", shown(value)))?;
            if let Some(min) = p.minimum.filter(|&min| x < min) {
                return Err(format!("is {x}; at least {min}"));
            }
            if let Some(max) = p.maximum.filter(|&max| x > max) {
                return Err(format!("is {x}; at most {max}"));
            }
            Ok(ElicitationContentValue::Number(x))
        }
        ElicitationPropertySchema::Boolean(_) => match value {
            Value::Bool(b) => Ok(ElicitationContentValue::Boolean(*b)),
            Value::String(s) if s == "true" => Ok(ElicitationContentValue::Boolean(true)),
            Value::String(s) if s == "false" => Ok(ElicitationContentValue::Boolean(false)),
            _ => Err(format!("must be true or false, not {}", shown(value))),
        },
        ElicitationPropertySchema::Array(p) => {
            let items: Vec<String> = match value {
                Value::String(s) => vec![s.clone()],
                Value::Array(items) => items
                    .iter()
                    .map(|i| i.as_str().map(str::to_owned))
                    .collect::<Option<_>>()
                    .ok_or_else(|| {
                        format!("must be a list of text values, not {}", shown(value))
                    })?,
                _ => return Err(format!("must be a list of text values, not {}", shown(value))),
            };
            let n = items.len() as u64;
            if let Some(min) = p.min_items.filter(|&min| n < min) {
                return Err(format!("has {n} values; at least {min}"));
            }
            if let Some(max) = p.max_items.filter(|&max| n > max) {
                return Err(format!("has {n} values; at most {max}"));
            }
            let choices: Option<Vec<String>> = match &p.items {
                MultiSelectItems::String(s) => Some(s.values.clone()),
                MultiSelectItems::Titled(t) => {
                    Some(t.options.iter().map(|o| o.value.clone()).collect())
                }
                _ => None,
            };
            if let Some(choices) = choices {
                for item in &items {
                    choice(item, &choices).map_err(|why| format!("has a value that {why}"))?;
                }
            }
            Ok(ElicitationContentValue::StringArray(items))
        }
        // A type brnr doesn't know: passed on as given, if ACP's content
        // takes it (P4: the agent checks what brnr can't).
        _ => schema::read::<ElicitationContentValue>(value).ok_or_else(|| {
            format!(
                "is of a type brnr can't check, and {} isn't a value ACP's content takes",
                shown(value)
            )
        }),
    }
}

fn merge(into: &mut Value, from: Value) {
    if let (Some(into), Value::Object(from)) = (into.as_object_mut(), from) {
        into.extend(from);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema() -> ElicitationSchema {
        schema::read(&json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "minLength": 1, "maxLength": 5 },
                "count": { "type": "integer", "minimum": 1, "maximum": 10 },
                "ratio": { "type": "number", "maximum": 1.5 },
                "ok": { "type": "boolean" },
                "color": { "type": "string", "enum": ["red", "green"] },
                "size": { "type": "string", "oneOf": [
                    { "const": "s", "title": "Small" }, { "const": "l", "title": "Large" } ] },
                "tags": { "type": "array", "items": { "type": "string", "enum": ["a", "b", "c"] },
                          "maxItems": 2 },
                "odd": { "type": "_mine" },
            },
            "required": ["name", "color"],
        }))
        .unwrap()
    }

    fn checked(given: Value) -> Result<Value, String> {
        let Value::Object(given) = given else { panic!() };
        check("e1", &schema(), given).map(|c| serde_json::to_value(c).unwrap())
    }

    #[test]
    fn strings_are_read_as_the_type_asks() {
        let content = checked(json!({
            "name": "ab", "count": "7", "ratio": "0.5", "ok": "true", "color": "red",
            "size": "l", "tags": "a", "odd": "x",
        }))
        .unwrap();
        assert_eq!(
            content,
            json!({ "name": "ab", "count": 7, "ratio": 0.5, "ok": true, "color": "red",
                    "size": "l", "tags": ["a"], "odd": "x" })
        );
        let typed = checked(json!({ "name": "ab", "count": 3, "ok": false, "color": "green",
                                    "tags": ["a", "b"] }))
        .unwrap();
        assert_eq!(typed["count"], 3);
        assert_eq!(typed["tags"], json!(["a", "b"]));
    }

    #[test]
    fn what_doesnt_fit_names_the_field() {
        for (given, says) in [
            (json!({ "color": "red" }), "e1: name is required (fields: color, count, name"),
            (json!({ "name": "a", "color": "red", "nope": 1 }), "e1: the form has no field nope"),
            (
                json!({ "name": "a", "color": "blue" }),
                "e1: color must be one of red, green, not \"blue\"",
            ),
            (json!({ "name": "toolong", "color": "red" }), "e1: name is 7 characters; at most 5"),
            (json!({ "name": "", "color": "red" }), "e1: name is 0 characters; at least 1"),
            (
                json!({ "name": "a", "color": "red", "count": "x" }),
                "e1: count must be an integer, not \"x\"",
            ),
            (json!({ "name": "a", "color": "red", "count": 11 }), "e1: count is 11; at most 10"),
            (
                json!({ "name": "a", "color": "red", "count": 1.5 }),
                "e1: count must be an integer, not 1.5",
            ),
            (json!({ "name": "a", "color": "red", "ratio": 2 }), "e1: ratio is 2; at most 1.5"),
            (
                json!({ "name": "a", "color": "red", "ok": "yes" }),
                "e1: ok must be true or false, not \"yes\"",
            ),
            (
                json!({ "name": "a", "color": "red", "size": "m" }),
                "e1: size must be one of s, l, not \"m\"",
            ),
            (
                json!({ "name": "a", "color": "red", "tags": ["a", "b", "c"] }),
                "e1: tags has 3 values; at most 2",
            ),
            (
                json!({ "name": "a", "color": "red", "tags": ["z"] }),
                "e1: tags has a value that must be one of a, b, c, not \"z\"",
            ),
            (
                json!({ "name": "a", "color": "red", "tags": [1] }),
                "e1: tags must be a list of text values, not [1]",
            ),
            (json!({ "name": 1, "color": "red" }), "e1: name must be text, not 1"),
            (
                json!({ "name": "a", "color": "red", "odd": {} }),
                "e1: odd is of a type brnr can't check",
            ),
        ] {
            let err = checked(given.clone()).unwrap_err();
            assert!(err.starts_with(says), "{given}: {err}");
        }
    }
}
