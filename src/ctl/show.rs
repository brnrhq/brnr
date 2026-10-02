//! `brnr show <target> [<request>]`: one waiting permission request in
//! full, so it can be judged before it is answered: the tool, its kind and
//! paths, the command or input, and the diff of an edit.

use std::process::ExitCode;

use serde_json::{Value, json};

use brnr::render;

use super::{USAGE, call, discover, resolve};

pub(super) fn show(args: &[String]) -> Result<ExitCode, String> {
    let (target, request) = match args {
        [target] => (target, None),
        [target, request] => (target, Some(request.as_str())),
        _ => return Err(USAGE.to_owned()),
    };
    let hosts = discover();
    let (host, session) = resolve(&hosts, target)?;
    let response = call(host, &json!({ "cmd": "pending" }))?;
    let pending: Vec<&Value> = response["pending"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|p| session.as_deref().is_none_or(|s| p["session"] == s))
        .collect();
    let p = match (request, &pending[..]) {
        (Some(id), _) => *pending
            .iter()
            .find(|p| p["request"] == id)
            .ok_or(format!("no pending request {id}"))?,
        (None, [one]) => *one,
        (None, []) => return Err("no permission request is waiting".into()),
        (None, _) => {
            let ids: Vec<&str> = pending.iter().filter_map(|p| p["request"].as_str()).collect();
            return Err(format!("several requests are waiting, pick one: {}", ids.join(", ")));
        }
    };
    let id = p["request"].as_str().unwrap_or("?");
    println!(
        "{id}, session {}, answered by the {}",
        p["session"].as_str().unwrap_or("?"),
        p["owner"].as_str().unwrap_or("?")
    );
    print!("{}", render::tool_call(&p["tool_call"]));
    let options: Vec<String> = p["options"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|o| {
            format!(
                "{} ({})",
                o["optionId"].as_str().unwrap_or("?"),
                o["kind"].as_str().unwrap_or("?")
            )
        })
        .collect();
    println!("options: {}", options.join(", "));
    if let Some(secs) = p["timeout_seconds"].as_u64() {
        println!("denied in {secs}s if nobody answers");
    }
    if p["owner"] == "host" {
        println!("answer: brnr approve {target} {id}, brnr deny {target} {id}, or --option <id>");
    }
    Ok(ExitCode::SUCCESS)
}
