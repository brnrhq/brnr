//! `brnr show <session> <request> [--json]`: one waiting approval in full,
//! so it can be judged before it is answered: the tool, its kind and paths,
//! the command or input, and the diff of an edit.

use std::process::ExitCode;

use serde_json::{Value, json};

use brnr::render;

use super::{USAGE, call, discover, print_json, running_session};

pub(super) fn show(args: &[String]) -> Result<ExitCode, String> {
    let mut positional = Vec::new();
    let mut json_out = false;
    for arg in args {
        match arg.as_str() {
            "--json" => json_out = true,
            flag if flag.starts_with("--") => return Err(format!("unknown option: {flag}")),
            _ => positional.push(arg),
        }
    }
    let [arg, request] = positional[..] else { return Err(USAGE.to_owned()) };
    let hosts = discover();
    let (host, session) = running_session(&hosts, arg)?;
    let response = call(host, &json!({ "cmd": "pending" }))?;
    let p: &Value = response["pending"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|p| p["session"] == session.as_str() && p["request"] == request.as_str())
        .ok_or(format!("no pending request {request} in {arg}"))?;
    if json_out {
        print_json(p)?;
        return Ok(ExitCode::SUCCESS);
    }
    let by = if p["owner"] == "editor" { ", answered in the editor" } else { "" };
    println!("{request}, session {session}{by}");
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
    if p["owner"] != "editor" {
        println!("answer: brnr approve {arg} {request}, brnr deny {arg} {request}, or --option <id>");
    }
    Ok(ExitCode::SUCCESS)
}
