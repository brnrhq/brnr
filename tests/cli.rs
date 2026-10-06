//! The headless CLI end to end against the fake agent: waiting for
//! replies, the log, status, settings, sessions and processes, approvals,
//! attachments, notifications and resuming.

mod common;

use std::fs;
use std::io::{BufRead, BufReader};
use std::process::Stdio;
use std::thread::sleep;
use std::time::Duration;

use common::*;
use serde_json::Value;

fn code(out: &std::process::Output) -> i32 {
    out.status.code().unwrap_or(-1)
}

fn stdout(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// The session's transcript events, as `log --json` gives them.
fn events(env: &Env, target: &str) -> Vec<Value> {
    env.ok(&["log", target, "--json"]).lines().map(|l| serde_json::from_str(l).unwrap()).collect()
}

fn idle(env: &Env) {
    assert_eq!(code(&env.run(&["wait", "sess-1", "--timeout", "10"])), 0, "not idle");
}

/// Until no turn is running, whatever the last turn's result (`wait` would
/// report a cancelled one as a failure).
fn settled(env: &Env) {
    let busy = || {
        let status: Value = serde_json::from_str(&env.ok(&["status", "sess-1", "--json"])).unwrap();
        status["state"] == "busy"
    };
    assert!(wait_for(Duration::from_secs(10), || !busy()), "still busy");
}

// ---- replies and waiting -------------------------------------------------

#[test]
fn send_wait_prints_the_reply() {
    let env = Env::new("c-sendwait");
    env.start(&[]);
    let out = env.run(&["send", "sess-1", "--wait", "reply", "hello", "there"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(stdout(&out), "hello there\n");
}

#[test]
fn start_wait_prints_the_reply_and_the_turns_result() {
    let env = Env::new("c-startwait");
    let out = env.run(&start_args(&["--wait", "--prompt", "reply done"]));
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(stdout(&out), "done\n");
    assert!(stderr(&out).contains("started"), "{}", stderr(&out));

    let env = Env::new("c-startfail");
    let out = env.run(&start_args(&["--wait", "--prompt", "fail"]));
    assert_eq!(code(&out), 1);
    assert!(stderr(&out).contains("turn failed: boom"), "{}", stderr(&out));
}

#[test]
fn send_wait_times_out() {
    let env = Env::new("c-sendto");
    env.start(&[]);
    let out = env.run(&["send", "sess-1", "--wait", "--timeout", "1", "hang on"]);
    assert_eq!(code(&out), 124, "{}", stderr(&out));
}

#[test]
fn send_wait_reports_a_permission_request() {
    let env = Env::new("c-sendperm");
    env.start(&[]);
    let mut send =
        env.brnr(&["send", "sess-1", "--wait", "perm edit"]).stderr(Stdio::piped()).spawn().unwrap();
    let mut err = BufReader::new(send.stderr.take().unwrap());
    let mut line = String::new();
    while !line.contains("waiting for approval") {
        line.clear();
        assert!(err.read_line(&mut line).unwrap() > 0, "no approval notice");
    }
    assert!(line.contains("p1: Edit src/lib.rs"), "{line}");
    assert!(line.contains("brnr approve sess-1 p1"), "{line}");
    env.ok(&["approve", "sess-1", "p1"]);
    assert!(wait_exit(&mut send, Duration::from_secs(10)));
    assert!(send.wait().unwrap().success());
}

#[test]
fn wait_returns_when_the_session_goes_idle() {
    let env = Env::new("c-wait");
    env.start(&[]);
    assert_eq!(env.ok(&["wait", "sess-1"]), "idle\n", "already idle");

    env.ok(&["send", "sess-1", "hang on"]);
    let mut wait = env.brnr(&["wait", "sess-1"]).stdout(Stdio::piped()).spawn().unwrap();
    sleep(Duration::from_millis(500));
    assert!(wait.try_wait().unwrap().is_none(), "returned while busy");
    env.ok(&["cancel", "sess-1"]);
    assert!(wait_exit(&mut wait, Duration::from_secs(10)));
    // The turn was cancelled: not a normal end.
    assert_eq!(wait.wait().unwrap().code(), Some(1));

    assert_eq!(code(&env.run(&["wait", "sess-1", "--for", "turn", "--timeout", "1"])), 124);
}

#[test]
fn wait_for_permission() {
    let env = Env::new("c-waitperm");
    env.start(&[]);
    env.ok(&["send", "sess-1", "perm edit"]);
    let out = env.ok(&["wait", "sess-1", "--for", "permission", "--timeout", "10"]);
    assert_eq!(out, "approval p1: Edit src/lib.rs\n");
    let json: Value =
        serde_json::from_str(&env.ok(&["wait", "sess-1", "--for", "permission", "--json"])).unwrap();
    assert_eq!(json["request"], "p1");
}

#[test]
fn wait_for_exit() {
    let env = Env::new("c-waitexit");
    env.start(&[]);
    let mut wait = env.brnr(&["wait", "sess-1", "--for", "exit"]).spawn().unwrap();
    sleep(Duration::from_millis(300));
    env.stop();
    assert!(wait_exit(&mut wait, Duration::from_secs(15)));
    assert!(wait.wait().unwrap().success());
}

// ---- cancel and the queue ------------------------------------------------

#[test]
fn cancel_drops_held_messages_and_says_so() {
    let env = Env::new("c-cancel");
    env.start(&["--prompt", "hang on"]);
    assert!(wait_for(Duration::from_secs(5), || env.prompts().len() == 1));
    env.ok(&["send", "sess-1", "--after-turn", "later"]);
    let out = env.ok(&["cancel", "sess-1"]);
    assert!(out.contains("cancelling"), "{out}");
    assert!(out.contains("dropped m2: later"), "{out}");
    settled(&env);
    assert_eq!(env.prompts(), ["hang on"]);
}

#[test]
fn cancel_can_keep_held_messages() {
    let env = Env::new("c-keep");
    env.start(&["--prompt", "hang on"]);
    assert!(wait_for(Duration::from_secs(5), || env.prompts().len() == 1));
    env.ok(&["send", "sess-1", "--after-turn", "later"]);
    env.ok(&["cancel", "sess-1", "--keep-held"]);
    assert!(wait_for(Duration::from_secs(5), || env.prompts().len() == 2));
    assert_eq!(env.prompts(), ["hang on", "later"]);
}

#[test]
fn queue_lists_and_drops() {
    let env = Env::new("c-queue");
    env.start(&["--prompt", "hang on"]);
    env.ok(&["send", "sess-1", "--after-turn", "first"]);
    env.ok(&["send", "sess-1", "--after-turn", "second"]);
    env.ok(&["send", "sess-1", "--context", "some context"]);
    let out = env.ok(&["queue", "sess-1"]);
    assert_eq!(out, "m2 (after turn): first\nm3 (after turn): second\ncontext: some context\n");
    let out = env.ok(&["queue", "sess-1", "--drop", "m2", "--clear-context"]);
    assert_eq!(out, "dropped m2: first\nm3 (after turn): second\n");
    let json: Value = serde_json::from_str(&env.ok(&["queue", "sess-1", "--json"])).unwrap();
    assert_eq!(json["held"][0]["message"], "m3");
    assert!(env.fails(&["queue", "sess-1", "--drop", "m9"]).contains("no held message m9"));
}

// ---- seeing --------------------------------------------------------------

#[test]
fn log_shows_the_conversation() {
    let env = Env::new("c-log");
    env.start(&["--prompt", "tools"]);
    idle(&env);
    env.ok(&["send", "sess-1", "--wait", "reply second"]);
    let log = env.ok(&["log", "sess-1"]);
    let lines: Vec<&str> = log.lines().map(|l| &l[10..]).collect();
    assert_eq!(
        lines,
        [
            "title: Fake session",
            "user: tools",
            "plan (0/2):",
            "  [>] Run the tests",
            "  [ ] Fix them",
            "tool: Run the tests (execute)",
            "tool done: Run the tests",
            "plan (1/2):",
            "  [x] Run the tests",
            "  [ ] Fix them",
            "agent: did the tools",
            "turn ended: end_turn (control)",
            "user: reply second",
            "agent: second",
            "turn ended: end_turn (control)",
        ],
        "{log}"
    );
    let last = env.ok(&["log", "sess-1", "--last", "1"]);
    assert!(last.lines().next().unwrap().ends_with("user: reply second"), "{last}");
    // `all` adds the ACP messages to the events, as for `watch`.
    let all = env.ok(&["log", "sess-1", "--events", "all"]);
    assert!(all.lines().any(|l| l[10..].starts_with("agent->editor ")), "{all}");
    assert!(all.contains("agent: second"), "{all}");
    let acp: Vec<Value> = env
        .ok(&["log", "sess-1", "--events", "all", "--json"])
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .filter(|e| e["event"] == "acp")
        .collect();
    assert!(acp.iter().any(|e| e["dir"] == "agent->editor" && e["msg"]["jsonrpc"] == "2.0"));
    assert!(acp.iter().all(|e| e["session"] == "sess-1" && e["ts"].is_string()));
    let names: Vec<String> =
        events(&env, "sess-1").iter().map(|e| e["event"].as_str().unwrap().to_owned()).collect();
    assert!(
        names.contains(&"usage".to_owned()) && names.contains(&"tool_call".to_owned()),
        "{names:?}"
    );
    let turn = events(&env, "sess-1").into_iter().find(|e| e["event"] == "turn_ended").unwrap();
    assert_eq!(turn["message"], "m1");
}

#[test]
fn log_shows_only_the_events_asked_for() {
    let env = Env::new("c-log-events");
    env.start(&["--wait", "--prompt", "reply first"]);
    let log = env.ok(&["log", "sess-1", "--events", "user_message,turn_ended"]);
    let lines: Vec<&str> = log.lines().map(|l| &l[10..]).collect();
    assert_eq!(lines, ["user: reply first", "turn ended: end_turn (control)"], "{log}");
    let acp = env.ok(&["log", "sess-1", "--events", "acp", "--json"]);
    assert!(!acp.is_empty() && acp.lines().all(|l| l.contains(r#""event":"acp""#)), "{acp}");
    assert!(env.fails(&["log", "sess-1", "--events", "nope"]).contains(r#"unknown event "nope""#));
    assert!(env.fails(&["log", "sess-1", "--raw"]).contains("unknown option: --raw"));
}

#[test]
fn log_reads_an_inactive_session() {
    let env = Env::new("c-loginactive");
    env.start(&["--wait", "--prompt", "reply bye"]);
    env.stop();
    assert!(wait_for(Duration::from_secs(15), || env.hosts().is_empty()));
    let log = env.ok(&["log", "sess-1"]);
    assert!(log.contains("agent: bye"), "{log}");
    let exited = log.lines().find(|l| l.contains("agent exited")).expect(&log);
    assert!(exited.as_bytes()[2] == b':', "no time on {exited:?}");
}

#[test]
fn log_follows_until_the_host_exits() {
    let env = Env::new("c-follow");
    env.start(&[]);
    let mut follow = env.brnr(&["log", "sess-1", "--follow"]).stdout(Stdio::piped()).spawn().unwrap();
    let mut out = BufReader::new(follow.stdout.take().unwrap());
    env.ok(&["send", "sess-1", "reply live"]);
    let mut line = String::new();
    while !line.contains("agent: live") {
        line.clear();
        assert!(out.read_line(&mut line).unwrap() > 0, "log ended early");
    }
    env.stop();
    assert!(wait_exit(&mut follow, Duration::from_secs(15)), "log --follow didn't end");
}

#[test]
fn thoughts_are_shown_when_asked() {
    let env = Env::new("c-think");
    env.start(&["--wait", "--prompt", "think"]);
    assert!(!env.ok(&["log", "sess-1"]).contains("pondering"));
    assert!(!env.ok(&["log", "sess-1", "--json"]).contains("pondering"));
    assert!(env.ok(&["log", "sess-1", "--events", "agent_thought"]).contains("thinking: pondering"));
    let both = env.ok(&["log", "sess-1", "--events", "default,agent_thought"]);
    assert!(both.contains("thinking: pondering") && both.contains("user: think"), "{both}");
    assert!(env.fails(&["log", "sess-1", "--thoughts"]).contains("unknown option: --thoughts"));
}

#[test]
fn watch_is_readable_by_default() {
    let env = Env::new("c-watch");
    env.start(&[]);
    let mut watch = env.brnr(&["watch", "sess-1"]).stdout(Stdio::piped()).spawn().unwrap();
    let mut out = BufReader::new(watch.stdout.take().unwrap());
    sleep(Duration::from_millis(300));
    env.ok(&["send", "sess-1", "reply hi"]);
    let mut seen = Vec::new();
    let mut line = String::new();
    while !line.contains("turn ended") {
        line.clear();
        assert!(out.read_line(&mut line).unwrap() > 0, "watch ended early");
        seen.push(line.clone());
    }
    assert!(seen.iter().any(|l| l.contains("agent: hi")), "{seen:?}");
    assert!(!seen.iter().any(|l| l.contains("->")), "raw ACP by default: {seen:?}");
    let _ = watch.kill();
    let _ = watch.wait();
}

#[test]
fn status_summarizes_the_session() {
    let env = Env::new("c-status");
    env.start(&["--prompt", "tools"]);
    idle(&env);
    let status = env.ok(&["status", "sess-1"]);
    for want in [
        "session sess-1: Fake session",
        &format!("process {}: fake_agent.py (fake-agent 1.2.3), headless", env.pid()),
        "mode default, model small",
        "idle",
        "plan (1/2):",
        "context window: 12.3k of 200.0k tokens, cost 0.42 USD",
        "last message: did the tools",
    ] {
        assert!(status.contains(want), "missing {want:?} in\n{status}");
    }
    let json: Value = serde_json::from_str(&env.ok(&["status", "sess-1", "--json"])).unwrap();
    assert_eq!(json["mode"], "default");
        assert_eq!(json["state"], "idle");
    assert_eq!(json["usage"]["used"], 12345);
}

// ---- settings ------------------------------------------------------------

#[test]
fn mode_lists_and_switches() {
    let env = Env::new("c-mode");
    env.start(&[]);
    let modes = env.ok(&["mode", "sess-1"]);
    assert!(modes.contains("* default") && modes.contains("  plan"), "{modes}");
    assert_eq!(env.ok(&["mode", "sess-1", "plan"]), "mode plan\n");
    let json: Value = serde_json::from_str(&env.ok(&["mode", "sess-1", "--json"])).unwrap();
    assert_eq!(json["mode"], "plan");
    assert!(env.ok(&["mode", "sess-1"]).contains("* plan"));
    assert!(env.fails(&["mode", "sess-1", "warp"]).contains("no mode warp"));
}

#[test]
fn model_and_config() {
    let env = Env::new("c-model");
    env.start(&[]);
    assert!(env.ok(&["model", "sess-1"]).contains("* small"));
    env.ok(&["model", "sess-1", "large"]);
    assert!(env.ok(&["model", "sess-1"]).contains("* large"));
    assert_eq!(env.calls_of("session/set_config_option")[0]["params"]["value"], "large");
    let config = env.ok(&["config", "sess-1"]);
    assert!(config.contains("model") && config.contains("small large"), "{config}");
    env.ok(&["config", "sess-1", "model=small"]);
    assert!(env.fails(&["config", "sess-1", "model=huge"]).contains("bad option"));
}

#[test]
fn start_applies_mode_and_model_before_the_prompt() {
    let env = Env::new("c-startmode");
    env.start(&["--mode", "plan", "--model", "large", "--wait", "--prompt", "reply ok"]);
    let methods: Vec<String> =
        env.calls().iter().filter_map(|c| c["method"].as_str().map(str::to_owned)).collect();
    assert_eq!(
        methods,
        [
            "initialize",
            "session/new",
            "session/set_mode",
            "session/set_config_option",
            "session/prompt"
        ]
    );
    let err = env.fails(&start_args(&["--mode", "warp"]));
    assert!(err.contains("setting mode warp failed"), "{err}");
}

#[test]
fn commands_lists_the_agents_commands() {
    let env = Env::new("c-commands");
    env.start(&[]);
    assert!(wait_for(Duration::from_secs(5), || env.ok(&["commands", "sess-1"]).contains("/compact")));
}

// ---- sessions ------------------------------------------------------------

#[test]
fn sessions_lists_the_agents_sessions() {
    let env = Env::new("c-sessions");
    env.start(&[]);
    // An agent of its own, started to ask: no process of brnr's needed.
    let out = env.ok(&["sessions", "--", AGENT]);
    assert!(out.contains("old-1") && out.contains("An old session"), "{out}");
    // What brnr knows of each.
    let row = |id: &str| out.lines().find(|l| l.starts_with(id)).unwrap_or_else(|| panic!("{out}"));
    assert!(!row("old-1").contains("running") && !row("old-1").contains("inactive"), "{out}");
    assert!(row("sess-1").contains(&format!("running ({})", env.host_pid())), "{out}");
    let json: Value = serde_json::from_str(&env.ok(&["sessions", "--json", "--", AGENT])).unwrap();
    assert_eq!(json[0]["session"], "old-1");
    assert_eq!(json[0]["brnr"], "-");
    assert_eq!(env.hosts().len(), 1, "an agent was left running");
}

#[test]
fn resume_a_session_only_the_agent_knows() {
    let env = Env::new("c-resume-agent");
    let out = env.run(&start_args(&["--resume", "old-1", "--wait", "--prompt", "reply again"]));
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(stdout(&out), "again\n");
    assert_eq!(env.calls_of("session/resume")[0]["params"]["sessionId"], "old-1");
    assert!(env.ok(&["log", "old-1"]).contains("agent: again"));
}

#[test]
fn fork_and_close() {
    let env = Env::new("c-fork");
    env.start(&[]);
    assert_eq!(env.ok(&["fork", "sess-1"]), "forked sess-1 into sess-2\n");
    env.ok(&["send", "sess-2", "hello"]);
    env.ok(&["close", "sess-2"]);
    assert!(env.fails(&["send", "sess-2", "hi"]).contains("sess-2 isn't running"));
    assert!(env.ok(&["status", "sess-1"]).contains("session sess-1"));
    let host = env.host_pid();
    env.ok(&["close", "sess-1"]);
    assert!(
        wait_for(Duration::from_secs(15), || !alive(host)),
        "the process kept running with no session"
    );
}

#[test]
fn resume_continues_a_session() {
    let env = Env::new("c-resume");
    env.start(&["--wait", "--prompt", "reply first"]);
    env.stop();
    assert!(wait_for(Duration::from_secs(15), || env.hosts().is_empty()));
    // With the same agent as before, without saying so.
    let out = env.run(&["start", "--resume", "sess-1", "--wait", "--prompt", "reply again"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(stdout(&out), "again\n");
    assert_eq!(env.calls_of("session/resume")[0]["params"]["sessionId"], "sess-1");
    let log = env.ok(&["log", "sess-1"]);
    assert!(log.contains("agent: first") && log.contains("agent: again"), "{log}");
    assert!(env.fails(&["start", "--resume", "sess-1"]).contains("already running"));
}

#[test]
fn resume_by_loading_keeps_the_replay_out_of_the_transcript() {
    let env = Env::new("c-load").agent("NO_RESUME", "1");
    env.start(&["--wait", "--prompt", "reply first"]);
    env.stop();
    assert!(wait_for(Duration::from_secs(15), || env.hosts().is_empty()));
    env.ok(&["start", "--resume", "sess-1", "--wait", "--prompt", "reply again"]);
    assert_eq!(env.calls_of("session/load").len(), 1);
    let all = env.ok(&["log", "sess-1", "--events", "all"]);
    assert!(!all.contains("replayed history"), "replay recorded:\n{all}");
    assert!(env.ok(&["log", "sess-1"]).contains("agent: again"));
}

// ---- permissions ---------------------------------------------------------

fn outcome(env: &Env, request: &str) -> Option<Value> {
    env.calls()
        .into_iter()
        .find(|c| c["id"] == request && c.get("method").is_none())
        .map(|c| c["result"]["outcome"].clone())
}

#[test]
fn permission_rules_by_kind() {
    let env = Env::new("c-rules");
    env.write_config(
        "[profiles.default]\npermissions = { default = \"ask\", read = \"auto-allow\" }\n",
    );
    env.start(&[]);
    env.ok(&["send", "sess-1", "perm read"]);
    assert!(wait_for(Duration::from_secs(5), || outcome(&env, "perm-1").is_some()));
    assert_eq!(outcome(&env, "perm-1").unwrap()["optionId"], "allow");
    env.ok(&["send", "sess-1", "perm edit"]);
    assert!(wait_for(Duration::from_secs(5), || env.ok(&["pending", "sess-1"]).contains("p2")));
    assert!(outcome(&env, "perm-2").is_none(), "edit was answered without asking");
}

#[test]
fn permissions_flag_overrides_the_default() {
    let env = Env::new("c-permflag");
    env.start(&["--permissions", "auto-deny"]);
    env.ok(&["send", "sess-1", "perm edit"]);
    assert!(wait_for(Duration::from_secs(5), || outcome(&env, "perm-1").is_some()));
    assert_eq!(outcome(&env, "perm-1").unwrap()["optionId"], "reject");
}

#[test]
fn unanswered_permission_times_out_as_deny() {
    let env = Env::new("c-permtimeout");
    env.write_config("[profiles.default]\npermission_timeout = 1\n");
    env.start(&[]);
    env.ok(&["send", "sess-1", "perm edit"]);
    assert!(wait_for(Duration::from_secs(5), || outcome(&env, "perm-1").is_some()), "never denied");
    assert_eq!(outcome(&env, "perm-1").unwrap()["optionId"], "reject");
    assert!(env.ok(&["log", "sess-1"]).contains("permission p1 -> reject (by timeout)"));
}

#[test]
fn show_explains_a_permission_request() {
    let env = Env::new("c-show");
    env.start(&[]);
    env.ok(&["send", "sess-1", "perm edit"]);
    assert!(wait_for(Duration::from_secs(5), || env.ok(&["pending", "sess-1"]).contains("p1")));
    assert!(env.fails(&["show", "sess-1"]).contains("usage:"), "a request is needed");
    let show = env.ok(&["show", "sess-1", "p1"]);
    for want in [
        "p1, session sess-1\n",
        "Edit src/lib.rs\nkind: edit\npath: src/lib.rs:2",
        "--- src/lib.rs\n+++ src/lib.rs\n@@ -1,3 +1,3 @@\n one\n-old line\n+new line\n three",
        "options: allow (allow_once), reject (reject_once)",
        "brnr approve sess-1 p1",
    ] {
        assert!(show.contains(want), "missing {want:?} in\n{show}");
    }
    let json: Value = serde_json::from_str(&env.ok(&["show", "sess-1", "p1", "--json"])).unwrap();
    assert_eq!(json["tool_call"]["kind"], "edit");
    let pending: Value = serde_json::from_str(&env.ok(&["pending", "--json"])).unwrap();
    assert_eq!(pending[0]["session"], "sess-1");
    assert_eq!(pending[0]["options"][0]["option"], "allow");
    assert!(env.fails(&["approve", "sess-1"]).contains("usage:"), "a request is needed");
    assert!(env.fails(&["approve", "sess-1", "p9"]).contains("no pending request p9"));
    let json: Value = serde_json::from_str(&env.ok(&["approve", "sess-1", "p1", "--json"])).unwrap();
    assert_eq!(json["outcome"]["optionId"], "allow");
}

// ---- lifecycle -----------------------------------------------------------

#[test]
fn stop_when_idle() {
    let env = Env::new("c-idlestop");
    env.start(&["--stop-when-idle", "0", "--prompt", "reply bye"]);
    assert!(wait_for(Duration::from_secs(15), || env.hosts().is_empty()), "kept running");
    assert_eq!(env.prompts(), ["reply bye"]);
    // Idle time counts from the start: no prompt, and it still ends.
    let env = Env::new("c-idlestart");
    env.start(&["--stop-when-idle", "1"]);
    assert!(wait_for(Duration::from_secs(15), || env.hosts().is_empty()), "kept running");
    assert!(env.fails(&start_args(&["--stop-when-idle"])).contains("not a number of seconds"));
}

#[test]
fn foreground_start_shows_the_session() {
    let env = Env::new("c-fg");
    let args = ["start", "--foreground", "--stop-when-idle", "0", "--prompt", "reply hi", "--", AGENT];
    let out = env.brnr(&args).stdin(Stdio::null()).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("user: reply hi") && text.contains("agent: hi"), "{text}");
    let mut json = args.to_vec();
    json.insert(1, "--json");
    let out = env.brnr(&json).stdin(Stdio::null()).output().unwrap();
    let events: Vec<Value> =
        stdout(&out).lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert!(events.iter().any(|e| e["event"] == "agent_message" && e["text"] == "hi"));
    assert!(env.fails(&["start", "--foreground", "--wait", "--prompt", "x"]).contains("don't go"));
}

#[test]
fn mcp_servers_reach_the_agent() {
    let env = Env::new("c-mcp");
    env.write_config(
        r#"[[profiles.default.mcp_servers]]
name = "files"
command = "true"
args = ["--x"]
env = { TOKEN = "t" }

[[profiles.default.mcp_servers]]
name = "web"
url = "https://example.invalid/mcp"
headers = { Authorization = "Bearer x" }
"#,
    );
    env.start(&[]);
    let servers = &env.calls_of("session/new")[0]["params"]["mcpServers"];
    assert_eq!(servers[0]["name"], "files");
    assert_eq!(servers[0]["env"][0], serde_json::json!({ "name": "TOKEN", "value": "t" }));
    assert_eq!(servers[1]["type"], "http");
    assert_eq!(servers[1]["headers"][0]["name"], "Authorization");

    let env = Env::new("c-mcpsse");
    env.write_config(
        "[[profiles.default.mcp_servers]]\nname = \"s\"\nurl = \"https://x\"\ntype = \"sse\"\n",
    );
    assert!(env.fails(&start_args(&[])).contains("doesn't support sse"));
}

#[test]
fn login_needed_is_explained() {
    let env = Env::new("c-auth").agent("AUTH", "1");
    let err = env.fails(&start_args(&[]));
    assert!(err.contains("log in (Log in to the fake)"), "{err}");
    assert!(err.contains("claude"), "{err}");
}

// ---- attachments ---------------------------------------------------------

#[test]
fn files_and_images_go_with_the_prompt() {
    let env = Env::new("c-attach");
    env.start(&[]);
    let image = env.dir.join("dot.png");
    fs::write(&image, b"\x89PNG fake").unwrap();
    let file = env.dir.join("notes file.txt");
    fs::write(&file, "notes").unwrap();
    env.ok(&[
        "send",
        "sess-1",
        "--file",
        file.to_str().unwrap(),
        "--image",
        image.to_str().unwrap(),
        "look",
    ]);
    assert!(wait_for(Duration::from_secs(5), || !env.calls_of("session/prompt").is_empty()));
    let prompt = &env.calls_of("session/prompt")[0]["params"]["prompt"];
    assert_eq!(prompt[0]["text"], "look");
    assert_eq!(prompt[1]["type"], "resource_link");
    assert!(prompt[1]["uri"].as_str().unwrap().ends_with("/notes%20file.txt"), "{prompt}");
    assert_eq!(prompt[2]["type"], "image");
    assert_eq!(prompt[2]["mimeType"], "image/png");
    assert_eq!(prompt[2]["data"], "iVBORyBmYWtl");

    let env = Env::new("c-noimage").agent("NO_IMAGE", "1");
    env.start(&[]);
    let image = env.dir.join("dot.png");
    fs::write(&image, b"x").unwrap();
    assert!(
        env.fails(&["send", "sess-1", "--image", image.to_str().unwrap(), "look"])
            .contains("doesn't take images")
    );
}

// ---- notifications -------------------------------------------------------

#[test]
fn notify_runs_a_command_per_event() {
    let env = Env::new("c-notify");
    env.start(&[]);
    let out = env.dir.join("notified");
    let script = format!("echo \"$BRNR_EVENT|$BRNR_MESSAGE|$BRNR_TITLE\" >> '{}'", out.display());
    let mut notify = env
        .brnr(&["notify", "sess-1", "--events", "turn_ended", "--", "sh", "-c", &script])
        .spawn()
        .unwrap();
    sleep(Duration::from_millis(300));
    env.ok(&["send", "sess-1", "--wait", "reply hi; $(touch pwned)"]);
    assert!(wait_for(Duration::from_secs(5), || out.exists()));
    let text = fs::read_to_string(&out).unwrap();
    assert_eq!(text, "turn_ended|hi; $(touch pwned)|Fake session\n");
    assert!(!env.dir.join("pwned").exists(), "the agent's text ran as shell");
    env.stop();
    assert!(wait_exit(&mut notify, Duration::from_secs(15)), "notify didn't exit with the host");
}

#[test]
fn notify_reads_events_as_watch_does() {
    let env = Env::new("c-notifyevents");
    env.start(&[]);
    let out = env.dir.join("notified");
    let script = format!("echo \"$BRNR_EVENT\" >> '{}'", out.display());
    let notify = |events: &str| {
        env.brnr(&["notify", "sess-1", "--events", events, "--", "sh", "-c", &script]).spawn().unwrap()
    };
    // `default` is notify's own: the turn ending, not the messages in it.
    let mut first = notify("default,user_message");
    sleep(Duration::from_millis(300));
    env.ok(&["send", "sess-1", "--wait", "reply hi"]);
    assert!(wait_for(Duration::from_secs(5), || {
        fs::read_to_string(&out).is_ok_and(|t| t.lines().count() >= 2)
    }));
    assert_eq!(fs::read_to_string(&out).unwrap(), "user_message\nturn_ended\n");
    env.stop();
    assert!(wait_exit(&mut first, Duration::from_secs(15)), "notify didn't exit with the host");
    let err = env.fails(&["notify", "sess-1", "--events", "nope", "--", "true"]);
    assert!(err.contains(r#"unknown event "nope" (events: default, all,"#), "{err}");
    assert!(env.fails(&["notify", "--", "true"]).contains("<session> or --pid"));
}

#[test]
fn notify_works_as_a_bridge() {
    let env = Env::new("c-notifybridge");
    let out = env.dir.join("notified");
    // As a bridge, it is told its process in $BRNR_PID.
    let script = env.dir.join("notify.sh");
    let line = format!(
        "exec {:?} notify --pid \"$BRNR_PID\" -- sh -c 'echo $BRNR_EVENT $BRNR_SESSION_ID >> {:?}'\n",
        env!("CARGO_BIN_EXE_brnr"),
        out.display().to_string()
    );
    fs::write(&script, line).unwrap();
    let config = format!("[[profiles.default.bridges]]\ncommand = [\"sh\", {:?}]\n", script.display().to_string());
    env.write_config(&config);
    env.start(&[]);
    sleep(Duration::from_millis(500));
    env.ok(&["send", "sess-1", "--wait", "reply hi"]);
    assert!(wait_for(Duration::from_secs(5), || out.exists()));
    assert_eq!(fs::read_to_string(&out).unwrap(), "turn_ended sess-1\n");
}

// ---- processes -----------------------------------------------------------

#[test]
fn ps_lists_the_processes() {
    let env = Env::new("c-ps");
    env.start(&[]);
    env.ok(&["fork", "sess-1"]);
    let ps: Value = serde_json::from_str(&env.ok(&["ps", "--json"])).unwrap();
    assert_eq!(ps[0]["pid"].as_i64(), Some(env.host_pid() as i64));
    assert_eq!(ps[0]["owner"], "headless");
    assert_eq!(ps[0]["sessions"], serde_json::json!(["sess-1", "sess-2"]));
    assert!(env.ok(&["ps"]).contains("sess-1, sess-2"));
    let list: Value = serde_json::from_str(&env.ok(&["list", "--json"])).unwrap();
    assert_eq!(list[0]["title"], "Fake session");
    assert_eq!(list[1]["session"], "sess-2");
    assert_eq!(list[1]["pid"], ps[0]["pid"]);
    // A session's watch and the process's.
    assert!(env.fails(&["watch"]).contains("<session> or --pid"));
    assert!(env.fails(&["stop", "sess-1"]).contains("no brnr process sess-1"));
    env.stop();
}
