//! An ACP line as json.rs reads it (ADR 2.5), to the depth limit, and the
//! parts of it brnr reads as the schema's types (ADR 2.3).

#![no_main]

use brnr::internals::{json, schema};
use libfuzzer_sys::fuzz_target;
use serde_json::Value;

fuzz_target!(|data: &[u8]| {
    let parsed = json::parse::<Value>(data, json::DEPTH);
    // What serde_json reads, json.rs reads the same; past the limit, nothing.
    if let Ok(value) = serde_json::from_slice::<Value>(data) {
        assert_eq!(parsed.as_ref(), Some(&value));
    }
    if json::depth(data) > json::DEPTH {
        assert_eq!(parsed, None);
    }
    let Some(msg) = parsed else { return };
    let (params, result) = (&msg["params"], &msg["result"]);
    let _ = schema::read::<schema::SessionUpdate>(&params["update"]);
    let _ = schema::read::<schema::PermissionOptionKind>(&params["options"][0]["kind"]);
    let _ = schema::read::<schema::AgentCapabilities>(&result["agentCapabilities"]);
    let _ = schema::read::<Vec<schema::AuthMethod>>(&result["authMethods"]);
    let _ = schema::read::<schema::NewSessionResponse>(result);
    let _ = schema::read::<schema::ListSessionsResponse>(result);
    let _ = schema::error_message(&msg["error"]);
    let _ = msg.to_string();
});
