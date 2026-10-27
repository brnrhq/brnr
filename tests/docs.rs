//! The documentation's examples are tested (#25), so they can't drift from
//! the CLI unnoticed:
//!
//! - every line of the `sh` blocks in [`DOCS`] is in [`EXAMPLES`], and runs
//!   against fake_agent.py in a scratch environment, with a session in the
//!   state the example needs and a check of what it says, or is in
//!   [`SKIPPED`], with why it can't run as written. A line in neither fails
//!   the test, as does an entry no document has any more;
//! - every `toml` block loads as a config, every key in its part, as
//!   `brnr doctor` checks one;
//! - every command (a group's with its verb) and option `brnr --help`
//!   lists is in the book's guide ([`GUIDE`]);
//! - every `json` block in the interface reference ([`REFERENCE`],
//!   `docs/reference/`) has an `<!-- example: … -->` just above it, whose
//!   command runs as an `sh` example does, and what brnr prints has the
//!   block's shape: the same fields at every level (but where the fields
//!   table says "optional"), each of the same JSON type, one its row names,
//!   null only where its row says "or null", in every item of a list; and
//!   every field of the block, at every level, has a row (`dropped[].reason`
//!   for a list's items), but inside a value whose row says "ACP's"; or the
//!   block is in [`JSON_SKIPPED`], with why. A bridge's example is a request and its
//!   reply, which a bridge started from the profile ([`BRIDGE`]) sends and
//!   gets;
//! - the reference's synopsis is `brnr --help`, and its errors page lists
//!   the error codes brnr has, as ADR 8.7 does;
//! - every `rust` block, in [`DOCS`] and [`REFERENCE`], is one of the
//!   library's doc examples (src/lib.rs), which `cargo test` compiles: a
//!   use of the facade (ADR 8.5) that can't drift from it.
//!
//! What the examples name that a test can't have (a real adapter, a model
//! the fake agent doesn't offer) is in [`STAND_INS`], with what stands in for
//! it. Another document joins by going in [`DOCS`]: the README, the book
//! (`docs/`, ADR 11.8), the site and the skill are there.

mod common;

use std::fs::{self, File};
use std::path::Path;
use std::process::{Child, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::sleep;
use std::time::Duration;

use serde_json::{Value, json};

use common::{AGENT, Env, kill, script, wait_for, with_default_interrupt};

/// The documents whose examples are tested, from the repository's root.
const DOCS: &[&str] = &[
    "README.md",
    "docs/introduction.md",
    "docs/install.md",
    "docs/guide/editor.md",
    "docs/guide/outside.md",
    "docs/guide/approvals.md",
    "docs/guide/events.md",
    "docs/guide/headless.md",
    "docs/guide/remote.md",
    "docs/guide/agents.md",
    "docs/guide/profiles.md",
    "docs/guide/native-agents.md",
    "docs/guide/doctor.md",
    "docs/project/releasing.md",
    "site/index.html",
    "skills/brnr/SKILL.md",
    "skills/brnr/references/orchestrate.md",
    "skills/brnr/references/approvals.md",
    "skills/brnr/references/observe.md",
    "skills/brnr/references/setup.md",
];

/// The book's guide, which together must mention every command and option
/// of `--help`.
const GUIDE: &[&str] = &[
    "docs/introduction.md",
    "docs/install.md",
    "docs/guide/editor.md",
    "docs/guide/outside.md",
    "docs/guide/approvals.md",
    "docs/guide/events.md",
    "docs/guide/headless.md",
    "docs/guide/remote.md",
    "docs/guide/agents.md",
    "docs/guide/profiles.md",
    "docs/guide/transcripts.md",
    "docs/guide/adapters.md",
    "docs/guide/native-agents.md",
    "docs/guide/doctor.md",
];

/// The interface reference's pages (ADR 8.4): every `json` block in them is
/// an example the test runs, or is in [`JSON_SKIPPED`].
const REFERENCE: &[&str] = &[
    "docs/reference/overview.md",
    "docs/reference/process.md",
    "docs/reference/session.md",
    "docs/reference/prompt.md",
    "docs/reference/queue.md",
    "docs/reference/permission.md",
    "docs/reference/elicitation.md",
    "docs/reference/config.md",
    "docs/reference/event.md",
    "docs/reference/acp.md",
    "docs/reference/doctor.md",
    "docs/reference/remote.md",
    "docs/reference/skill.md",
    "docs/reference/events.md",
    "docs/reference/bridge.md",
    "docs/reference/exit-codes.md",
    "docs/reference/errors.md",
    "docs/reference/configuration.md",
    "docs/reference/files.md",
];

/// The reference's `json` blocks no test runs, by page and heading, and why.
const JSON_SKIPPED: &[(&str, &str, &str)] = &[
    ("events.md", "`session_reopened`", "needs an editor's load the agent answers after the close"),
    ("events.md", "`peer_cut_off`", "needs a peer 16 MiB behind"),
    ("events.md", "`records_skipped`", "needs a disk too slow for the transcript"),
    ("events.md", "`events_missed`", "needs a command cut off that can't catch up"),
];

/// What runs before an example: the state of its session, `$s`.
#[derive(Clone, Copy)]
enum Before {
    /// Nothing running.
    Nothing,
    /// A session whose first turn (`reply hi`) has ended.
    Idle,
    /// A turn running until it is cancelled.
    Turn,
    /// A turn that ends after a second.
    Slow,
    /// An approval waiting, `p1`.
    Approval,
    /// An elicitation waiting, `e1`: the fake agent's form (ADR 4.5).
    Elicitation,
    /// A session that ran a turn and was closed.
    Ended,
}

/// What an example must do. Each also exits 0.
#[derive(Clone, Copy)]
enum Then {
    Succeeds,
    /// Prints this on stdout (or in the file it is redirected to).
    Says(&'static str),
    /// Runs until `$s` closes, printing this meanwhile (on stdout or
    /// stderr, where `notify` has its command's output): `$s` is sent
    /// `reply readme` until it does, then closed.
    Follows(&'static str),
    /// Runs until Ctrl-C (SIGINT), printing this first.
    RunsUntilInterrupted(&'static str),
}

use Before::*;
use Then::*;

/// Every `brnr` command in the documents' `sh` blocks (its comment left
/// out), how it runs, and what it must do.
const EXAMPLES: &[(&str, Before, Then)] = &[
    // Use it from an editor; its stdin is closed at once, as an editor
    // going away closes it.
    ("brnr acp -- brnr-claude-adapter", Nothing, Succeeds),
    ("brnr acp -- brnr-codex-adapter", Nothing, Succeeds),
    ("brnr acp -- brnr-muse-adapter", Nothing, Succeeds),
    ("brnr acp --profile work", Nothing, Succeeds),
    // Agents with ACP of their own: their arguments go to the fake agent.
    ("brnr acp -- junie --acp true", Nothing, Succeeds),
    ("brnr acp -- cursor-agent acp", Nothing, Succeeds),
    (
        r#"brnr session new --prompt "fix the failing tests" -- junie --acp true"#,
        Nothing,
        Says("started"),
    ),
    // Talk to it from outside
    ("brnr session list", Idle, Says("sess-1")),
    ("brnr session status $s", Idle, Says("Fake session")),
    (r#"brnr prompt send $s --prompt "also update the changelog""#, Turn, Says("held")),
    (r#"brnr prompt send $s --steer --prompt "and the tests too""#, Turn, Says("steered")),
    (
        r#"brnr prompt send $s --interrupt --prompt "stop, wrong branch""#,
        Turn,
        Says("interrupting"),
    ),
    (r#"brnr prompt send $s --context --prompt "the API key is in .env.local""#, Idle, Succeeds),
    (r#"brnr prompt send $s --wait --prompt "what did you change?""#, Idle, Succeeds),
    (
        r#"brnr prompt send $s --file src/api.rs --image screenshot.png --prompt "why does this look wrong?""#,
        Idle,
        Says("delivered"),
    ),
    ("brnr prompt cancel $s", Turn, Succeeds),
    ("brnr queue list $s", Turn, Succeeds),
    ("brnr queue clear $s", Turn, Says("nothing held")),
    ("brnr event watch $s", Idle, Follows("agent: readme")),
    ("brnr event log $s", Idle, Says("agent: hi")),
    // Waiting, for scripts
    ("brnr event wait $s", Idle, Succeeds),
    ("brnr event wait $s --for permission", Approval, Succeeds),
    ("brnr event wait $s --for elicitation", Elicitation, Succeeds),
    ("brnr event wait $s --for turn", Slow, Succeeds),
    (
        r#"brnr session new --wait --stop-when-idle 0 --prompt "fix the failing tests" -- brnr-claude-adapter > answer.md"#,
        Nothing,
        Succeeds,
    ),
    // Approvals
    ("brnr permission requests", Approval, Says("p1")),
    ("brnr permission show $s p1", Approval, Says("Edit src/lib.rs")),
    ("brnr permission allow $s p1", Approval, Says("p1 allow")),
    ("brnr permission reject $s p1", Approval, Says("p1 reject")),
    // Elicitations
    ("brnr elicitation requests", Elicitation, Says("e1")),
    ("brnr elicitation show $s e1", Elicitation, Says("name (string, required)")),
    (
        "brnr elicitation accept $s e1 --field name=Ada --field color=green",
        Elicitation,
        Says("e1 accepted"),
    ),
    ("brnr elicitation decline $s e1", Elicitation, Says("e1 declined")),
    ("brnr elicitation cancel $s e1", Elicitation, Says("e1 cancelled")),
    // Settings: each runs without its optional argument and with it.
    ("brnr config get $s [--model]", Idle, Says("* small")),
    ("brnr config set $s --mode plan", Idle, Says("mode=plan")),
    ("brnr config set $s --model opus", Idle, Says("model=large")),
    ("brnr config set $s --option effort=high", Idle, Says("model=large")),
    ("brnr prompt commands $s", Idle, Says("compact")),
    // Sessions and processes
    ("brnr session fork $s", Idle, Says("forked")),
    (r#"brnr session new --pid 4466 --prompt "review the diff""#, Idle, Says("started sess-2")),
    ("brnr session resume --pid 4466 <id>", Idle, Says("started old-1")),
    ("brnr session close $s", Idle, Says("closed")),
    ("brnr session delete $s", Ended, Says("deleted sess-1; brnr's transcript of it stays")),
    ("brnr session delete $s --purge", Ended, Says("deleted sess-1, and brnr's transcript")),
    (
        "brnr session prune --older-than 2592000 --dry-run",
        Ended,
        Says("nothing to prune was last active before"),
    ),
    ("brnr session list -- brnr-claude-adapter", Nothing, Says("old-1")),
    ("brnr session resume <id> -- brnr-claude-adapter", Nothing, Says("started old-1")),
    ("brnr session resume $s --take-over", Idle, Says("started sess-1")),
    ("brnr process list", Idle, Says("headless")),
    ("brnr process stop 4466", Idle, Succeeds),
    // Notifications: curl is a stand-in, first on PATH, that prints its
    // arguments.
    (
        r#"brnr event notify $s -- sh -c 'curl -s -d "$BRNR_TEXT" "https://ntfy.sh/${NTFY_TOPIC:?set it to a private topic}"'"#,
        Idle,
        Follows("-d turn ended: end_turn (control) https://ntfy.sh/brnr-docs-topic"),
    ),
    // Headless sessions
    (
        r#"brnr session new --cwd ~/work/project --prompt "fix the failing tests" -- brnr-claude-adapter"#,
        Nothing,
        Says("started"),
    ),
    ("brnr session new --mode plan --model opus --prompt - < task.md", Nothing, Says("started")),
    ("brnr session new --option effort=high --prompt - < task.md", Nothing, Says("started")),
    ("brnr session resume $s", Ended, Says("started sess-1")),
    (
        "brnr session new --auth api-key --prompt - -- brnr-codex-adapter < task.md",
        Nothing,
        Succeeds,
    ),
    (
        "brnr session new --foreground --prompt - -- brnr-codex-adapter < task.md",
        Nothing,
        RunsUntilInterrupted("agent: readme"),
    ),
    // Doctor
    ("brnr doctor", Nothing, Succeeds),
    ("brnr doctor --fix", Nothing, Succeeds),
    ("brnr doctor --report", Nothing, Says("brnr doctor --report")),
    // The skill's core (skills/brnr/SKILL.md)
    (
        "brnr session new --json --stop-when-idle 600 --prompt - -- brnr-claude-adapter < task.md",
        Nothing,
        Says(r#""session": "sess-1""#),
    ),
    (
        r#"brnr prompt send $s --json --prompt "now add tests for it""#,
        Idle,
        Says(r#""status": "delivered""#),
    ),
    ("brnr event wait $s --timeout 600 --json", Idle, Says(r#""state": "idle""#)),
    (
        r#"brnr prompt send $s --wait --timeout 600 --json --prompt "what did you change?""#,
        Idle,
        Says(r#""stop_reason": "end_turn""#),
    ),
    ("brnr event log $s --last 1 --json", Idle, Says(r#""event":"turn_ended""#)),
    ("brnr session status $s --json", Idle, Says(r#""state": "idle""#)),
    ("brnr session close $s --json", Idle, Says(r#""session": "sess-1""#)),
    ("brnr session list --include active --json", Idle, Says(r#""session": "sess-1""#)),
    ("brnr permission requests --json", Approval, Says(r#""request": "p1""#)),
    ("brnr permission show $s p1 --json", Approval, Says(r#""oldText""#)),
    ("brnr elicitation requests --json", Elicitation, Says(r#""request": "e1""#)),
    // Its references: orchestrate
    (
        "brnr session new --json --stop-when-idle 600 --cwd ~/work/project --prompt - -- brnr-claude-adapter < task.md",
        Nothing,
        Says(r#""message": "m1""#),
    ),
    (
        r#"brnr prompt send $s --json --prompt "also update the changelog""#,
        Turn,
        Says(r#""status": "held""#),
    ),
    ("brnr event wait $s --timeout 900 --json", Idle, Says(r#""state": "idle""#)),
    ("brnr event wait $s --for turn --timeout 900 --json", Slow, Says(r#""event": "turn_ended""#)),
    (
        r#"brnr prompt send $s --wait --timeout 900 --json --prompt "what did you change, and why?""#,
        Idle,
        Says(r#""dropped": []"#),
    ),
    (
        "brnr session new --wait --timeout 900 --json --stop-when-idle 0 --prompt - -- brnr-claude-adapter < task.md",
        Nothing,
        Says(r#""reply": "readme"#),
    ),
    (
        r#"brnr session new --json --stop-when-idle 600 --cwd ~/work/api --prompt "make the api tests pass" -- brnr-claude-adapter"#,
        Nothing,
        Says(r#""session": "sess-1""#),
    ),
    (
        r#"brnr session new --json --stop-when-idle 600 --cwd ~/work/web --prompt "make the web tests pass" -- brnr-codex-adapter"#,
        Nothing,
        Says(r#""session": "sess-1""#),
    ),
    (
        r#"brnr session new --pid 4466 --json --cwd ~/work/docs --prompt "update the docs to match""#,
        Idle,
        Says(r#""session": "sess-2""#),
    ),
    (
        r#"brnr prompt send $s --steer --json --prompt "use the existing helper in src/util.rs""#,
        Turn,
        Says(r#""status": "steered""#),
    ),
    (
        r#"brnr prompt send $s --interrupt --json --prompt "stop: wrong branch, switch to main first""#,
        Turn,
        Says(r#""status": "interrupting""#),
    ),
    (
        r#"brnr prompt send $s --context --json --prompt "the API key is in .env.local""#,
        Idle,
        Succeeds,
    ),
    ("brnr queue list $s --json", Turn, Says(r#""held": []"#)),
    ("brnr prompt cancel $s --json", Turn, Says(r#""status": "cancelling""#)),
    ("brnr process list --json", Idle, Says(r#""owner": "headless""#)),
    // approvals
    (
        "brnr event wait $s --for permission --timeout 600 --json",
        Approval,
        Says(r#""request": "p1""#),
    ),
    ("brnr permission allow $s p1 --json", Approval, Says(r#""optionId": "allow""#)),
    ("brnr permission reject $s p1 --json", Approval, Says(r#""optionId": "reject""#)),
    (
        "brnr permission allow $s p1 --option-id <option> --json",
        Approval,
        Says(r#""optionId": "allow""#),
    ),
    (
        "brnr event wait $s --for elicitation --timeout 600 --json",
        Elicitation,
        Says(r#""request": "e1""#),
    ),
    (
        "brnr elicitation accept $s e1 --field name=Ada --field color=green --json",
        Elicitation,
        Says(r#""action": "accept""#),
    ),
    ("brnr elicitation decline $s e1 --json", Elicitation, Says(r#""action": "decline""#)),
    // observe
    ("brnr event log $s --json", Idle, Says(r#""event":"agent_message""#)),
    ("brnr event watch $s --json", Idle, Follows(r#""event":"agent_message""#)),
    ("brnr event watch --pid 4466 --json", Idle, Follows(r#""event":"agent_message""#)),
    (
        "brnr event log $s --events default,agent_thought --json",
        Idle,
        Says(r#""event":"turn_ended""#),
    ),
    (
        "brnr event watch $s --events turn_ended,permission_request --json",
        Idle,
        Follows(r#""event":"turn_ended""#),
    ),
    // setup
    ("brnr doctor --json", Nothing, Says(r#""check": "config""#)),
    ("brnr session new --profile work --json --prompt - < task.md", Nothing, Says(r#""session""#)),
    ("brnr skill", Nothing, Says("name: brnr")),
    ("brnr skill orchestrate", Nothing, Says("# Orchestrating workers")),
    ("brnr skill install", Nothing, Says("installed")),
    ("brnr skill install --dir .claude/skills", Nothing, Says("installed .claude/skills/brnr")),
];

/// The lines of `sh` blocks that don't run here, and why.
const SKIPPED: &[(&str, &str)] = &[
    ("ssh agent@linux", "establishes authentication and host trust with an external SSH host"),
    (
        "brnr session list --remote agent@linux",
        "requires an SSH host; tests/remote.rs exercises transport",
    ),
    ("brnr acp --remote agent@linux --profile coding", "requires an SSH host and remote profile"),
    ("brnr remote check linux", "requires an SSH host"),
    ("brnr remote check --profile remote-work", "requires a routing profile and SSH host"),
    ("brnr session list --remote linux --json", "requires an SSH host"),
    ("brnr event watch $s --remote linux", "requires a remote session"),
    ("brnr acp --profile remote-work", "requires a routing profile and SSH host"),
    ("brnr session list --profile remote-work", "requires a routing profile and SSH host"),
    (
        r#"brnr prompt send $s --profile remote-work --prompt "Continue""#,
        "requires a remote session",
    ),
    ("brew install brnrhq/tap/brnr", "installs from the Homebrew tap"),
    ("brew install brnrhq/tap/brnr-claude-adapter", "installs from the Homebrew tap"),
    ("brew install brnrhq/tap/brnr-codex-adapter", "installs from the Homebrew tap"),
    ("brew install brnrhq/tap/brnr-muse-adapter", "installs from the Homebrew tap"),
    ("gh attestation verify brnr-X.tar.gz -R brnrhq/brnr", "verifies a release asset, from GitHub"),
    ("cargo install brnr --locked", "installs from crates.io"),
    ("cargo build --release", "the build these tests run"),
    ("adapters/build.sh", "needs bun; CI's adapters job runs it"),
    ("./release.sh minor", "the maintainer's release, on GitHub"),
    ("./release.sh tag", "the maintainer's release, on GitHub"),
    ("./release.sh notes", "the maintainer's release, on GitHub"),
    ("./release.sh 1.0.0-rc.1", "the maintainer's release, on GitHub"),
];

/// Words in the examples a test can't have as written, and what stands in
/// for each: `{agent}` is fake_agent.py, `{dir}` the scratch directory,
/// `{pid}` the one running process, `{s}` the example's session.
const STAND_INS: &[(&str, &str)] = &[
    ("$s", "{s}"),
    ("brnr-claude-adapter", "{agent}"),
    ("brnr-codex-adapter", "{agent}"),
    ("brnr-muse-adapter", "{agent}"),
    ("junie", "{agent}"),
    ("cursor-agent", "{agent}"),
    ("~/work/project", "{dir}/project"),
    ("~/work/api", "{dir}/project"),
    ("~/work/web", "{dir}/project"),
    ("~/work/docs", "{dir}/project"),
    // The fake agent's models are small and large; its one config option
    // is model, and its login method fake-login.
    ("opus", "large"),
    ("<model>", "large"),
    ("effort=high", "model=large"),
    ("api-key", "fake-login"),
    // The fake agent's allow option.
    ("<option>", "allow"),
    // A session only the agent knows (session/list has it).
    ("<id>", "old-1"),
    ("4466", "{pid}"),
];

/// How long an example may take.
const TIMEOUT: Duration = Duration::from_secs(30);

/// How many examples run at once.
const WORKERS: usize = 6;

#[test]
fn adr_0008_every_example_is_run_or_skipped_and_runs() {
    let (lines, _) = documents();
    let mut problems = Vec::new();
    let mut used = vec![false; EXAMPLES.len()];
    let mut skips = vec![false; SKIPPED.len()];
    for (at, line) in &lines {
        if let Some(i) = EXAMPLES.iter().position(|(cmd, ..)| cmd == line) {
            used[i] = true;
        } else if let Some(i) = SKIPPED.iter().position(|(cmd, _)| cmd == line) {
            skips[i] = true;
        } else {
            problems.push(format!(
                "{at}: `{line}` is in neither EXAMPLES (how it runs) nor SKIPPED (why it can't)"
            ));
        }
    }
    for (i, (cmd, ..)) in EXAMPLES.iter().enumerate().filter(|(i, _)| !used[*i]) {
        problems.push(format!("EXAMPLES[{i}]: `{cmd}` is in no document"));
    }
    for (i, (cmd, _)) in SKIPPED.iter().enumerate().filter(|(i, _)| !skips[*i]) {
        problems.push(format!("SKIPPED[{i}]: `{cmd}` is in no document"));
    }

    let mut jobs = Vec::new();
    for (i, (cmd, before, then)) in EXAMPLES.iter().enumerate().filter(|(i, _)| used[*i]) {
        for (j, words) in variants(&lex(cmd)).into_iter().enumerate() {
            jobs.push((format!("d{i}-{j}"), *cmd, words, *before, *then));
        }
    }
    let next = AtomicUsize::new(0);
    let failed = Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for _ in 0..WORKERS {
            scope.spawn(|| {
                while let Some((name, cmd, words, before, then)) =
                    jobs.get(next.fetch_add(1, Ordering::Relaxed))
                {
                    if let Err(e) = run(name, words, *before, *then) {
                        let as_run: Vec<&str> = words.iter().map(|w| &*w.text).collect();
                        failed.lock().unwrap().push(format!("`{cmd}` ({as_run:?}): {e}"));
                    }
                }
            });
        }
    });
    problems.extend(failed.into_inner().unwrap());
    assert!(problems.is_empty(), "\n{}", problems.join("\n"));
}

/// Every `rust` block in the documents is one of src/lib.rs's doc
/// examples, line for line, which `cargo test --doc` compiles: the facade's
/// uses can't drift from it (ADR 8.5). A block runs nothing, as the doc
/// example is `no_run`: it would start processes.
#[test]
fn adr_0008_every_rust_block_is_a_compiled_doc_example() {
    let lib = read("src/lib.rs");
    let examples: Vec<String> = lib
        .split("//! ```")
        .skip(1)
        .step_by(2)
        .map(|block| {
            let body = block.split_once('\n').map_or("", |(_, body)| body);
            let lines = body.lines().map(|l| l.trim_start().trim_start_matches("//!"));
            let lines = lines.map(|l| l.strip_prefix(' ').unwrap_or(l));
            lines.filter(|l| !l.starts_with("# ")).map(|l| format!("{l}\n")).collect()
        })
        .collect();
    let mut found = 0;
    for doc in DOCS.iter().chain(REFERENCE) {
        for (lang, start, body) in md_blocks(&read(doc)) {
            if lang == "rust" {
                found += 1;
                let at = format!("{doc}:{start}");
                assert!(
                    examples.contains(&body),
                    "{at} isn't a doc example in src/lib.rs:\n{body}"
                );
            }
        }
    }
    assert!(found > 0, "no rust block: the reference's overview has one");
}

#[test]
fn adr_0008_every_toml_block_loads() {
    let (_, blocks) = documents();
    assert!(!blocks.is_empty());
    let mut problems = Vec::new();
    for (i, (at, text)) in blocks.into_iter().enumerate() {
        let env = Env::new(&format!("toml{i}"));
        env.write_config(&text);
        let out = env.run(&["doctor", "--json"]);
        let checks: Value = serde_json::from_slice(&out.stdout).unwrap();
        let config =
            checks["checks"].as_array().unwrap().iter().find(|c| c["check"] == "config").unwrap();
        if config["level"] != "ok" {
            problems.push(format!("{at}: {}", config["message"]));
        }
    }
    assert!(problems.is_empty(), "\n{}", problems.join("\n"));
}

#[test]
fn the_guide_has_every_command_and_option() {
    let help = Env::new("help").ok(&["--help"]);
    let guide = GUIDE.iter().map(|doc| read(doc)).collect::<Vec<_>>().join("\n");
    // A group's commands are its name and verb: `brnr process list`.
    let commands = help.lines().filter_map(|l| {
        let mut words = l.strip_prefix("  brnr ")?.split_whitespace();
        let cmd = words.next().filter(|c| !c.starts_with('-'))?;
        Some(match words.next().filter(|v| v.starts_with(|c: char| c.is_ascii_lowercase())) {
            Some(verb) => format!("brnr {cmd} {verb}"),
            None => format!("brnr {cmd}"),
        })
    });
    let options = help
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
        .filter(|w| w.starts_with("--") && w.len() > 2)
        .map(str::to_owned);
    let mut missing: Vec<String> =
        commands.chain(options).filter(|w| !mentions(&guide, w)).collect();
    missing.dedup();
    assert!(missing.is_empty(), "the guide ({GUIDE:?}) doesn't mention {missing:?}");
}

/// Every `json` block in the reference is an example: what brnr prints for
/// the command in the `<!-- example: … -->` just above it has its shape
/// (`run_json`), or the block is in [`JSON_SKIPPED`], with why. A block in
/// neither fails the test, as does an entry no block has any more.
#[test]
fn adr_0008_every_json_example_has_the_shape_brnr_prints() {
    let blocks = json_blocks();
    let mut problems = Vec::new();
    let mut skips = vec![false; JSON_SKIPPED.len()];
    let mut jobs = Vec::new();
    for (i, block) in blocks.iter().enumerate() {
        let skipped = JSON_SKIPPED.iter().position(|(page, heading, _)| {
            block.page.ends_with(page) && block.heading.contains(heading)
        });
        match (&block.spec, skipped) {
            (Some(_), Some(k)) => {
                problems.push(format!("{}: an example, and JSON_SKIPPED[{k}] too", block.at))
            }
            (Some(_), None) => jobs.push((format!("j{i}"), block)),
            (None, Some(k)) => skips[k] = true,
            (None, None) => problems.push(format!(
                "{}: no <!-- example: … --> above it, and not in JSON_SKIPPED",
                block.at
            )),
        }
    }
    for (k, (page, heading, _)) in JSON_SKIPPED.iter().enumerate().filter(|(k, _)| !skips[*k]) {
        problems.push(format!("JSON_SKIPPED[{k}]: no block under {heading} in {page}"));
    }
    let next = AtomicUsize::new(0);
    let failed = Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for _ in 0..WORKERS {
            scope.spawn(|| {
                while let Some((name, block)) = jobs.get(next.fetch_add(1, Ordering::Relaxed)) {
                    if let Err(e) = run_json(name, block) {
                        let spec = block.spec.as_deref().unwrap_or_default();
                        failed.lock().unwrap().push(format!("{} ({spec}): {e}", block.at));
                    }
                }
            });
        }
    });
    problems.extend(failed.into_inner().unwrap());
    eprintln!("{} json examples checked, {} skipped", jobs.len(), skips.len());
    assert!(problems.is_empty(), "\n{}", problems.join("\n\n"));
}

// ---- the documents --------------------------------------------------------

#[test]
fn the_interface_reference_matches_cli_help() {
    let help = Env::new("reference-help").ok(&["--help"]);
    let reference = read("docs/reference/overview.md");
    let synopsis = reference
        .split_once("```text\n")
        .expect("interface reference needs the CLI synopsis")
        .1
        .split_once("\n```")
        .expect("CLI synopsis must end")
        .0;
    assert_eq!(synopsis, help.trim_end(), "update the reference when CLI syntax changes");
}

/// The error codes a test can't reach, with why (ADR 8.7). Every other
/// code is asserted by a test, through `code_is`: in tests/, or in a unit
/// test in src/.
const UNREACHED: &[(&str, &str)] = &[];

/// The error codes, as brnr has them, are the ones docs/reference/errors.md
/// and ADR 8.7 list, in the same order; and a test asserts that brnr reported
/// each one, but those in [`UNREACHED`], with why (ADR 8.7).
#[test]
fn adr_0008_every_error_code_is_listed_and_reached() {
    let codes = brnr::internals::error::names();
    // The first column of the table that follows `heading`.
    let listed = |doc: &str, heading: &str| -> Vec<String> {
        let text = read(doc);
        let (_, after) = text.split_once(heading).unwrap_or_else(|| panic!("{doc}: {heading}"));
        let table = after.lines().skip_while(|l| !l.starts_with("| `"));
        let rows = table.take_while(|l| l.starts_with("| `"));
        rows.filter_map(|l| Some(l.split('`').nth(1)?.to_owned())).collect()
    };
    let adr = "docs/adr/0008-the-command-line-and-compatibility.md";
    for (doc, heading) in [("docs/reference/errors.md", "## Codes"), (adr, "## 8.7 ")] {
        assert_eq!(listed(doc, heading), codes, "{doc} lists other codes than src/error.rs");
    }
    // The tests: tests/, and the unit tests in src/ (after a module's
    // `#[cfg(test)]`), but error.rs's own, which only reads codes back.
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut tests = String::new();
    let mut files = vec![root.join("src")];
    while let Some(path) = files.pop() {
        if path.is_dir() {
            files.extend(fs::read_dir(&path).unwrap().map(|e| e.unwrap().path()));
        } else if path.extension().is_some_and(|e| e == "rs") && !path.ends_with("src/error.rs") {
            let text = fs::read_to_string(&path).unwrap();
            tests.push_str(text.split_once("#[cfg(test)]").map_or("", |(_, tests)| tests));
        }
    }
    for entry in fs::read_dir(root.join("tests")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "rs") && !path.ends_with("tests/docs.rs") {
            tests.push_str(&fs::read_to_string(&path).unwrap());
        }
    }
    let reached = asserted_codes(&tests);
    for code in &codes {
        let why = UNREACHED.iter().find(|(c, _)| c == code);
        match (reached.iter().any(|r| r == code), why) {
            (true, Some(_)) => panic!("{code} is reached: take it out of UNREACHED"),
            (false, None) => panic!("no test asserts {code}, and UNREACHED doesn't say why"),
            _ => {}
        }
    }
    for (code, _) in UNREACHED {
        assert!(codes.contains(code), "UNREACHED has {code}, which brnr doesn't");
    }
    let unknown: Vec<&String> = reached.iter().filter(|c| !codes.contains(&c.as_str())).collect();
    assert!(unknown.is_empty(), "tests assert codes brnr doesn't have: {unknown:?}");
}

/// The codes `tests` asserts: each `code_is(said, code)` call's, where
/// `code` is written out, `"name"` (tests/common's) or `Code::name`
/// (error.rs's). A code only named (in a comment, a fixture, a request) or
/// passed in a variable isn't one.
fn asserted_codes(tests: &str) -> Vec<String> {
    let code = tests.lines().filter(|l| !l.trim_start().starts_with("//"));
    let code = code.collect::<Vec<_>>().join("\n");
    let mut found: Vec<String> = (code.split("code_is(").skip(1))
        .filter_map(|call| {
            // The call, to the end of its statement.
            let call = call.split(';').next()?.trim_end().strip_suffix(')')?;
            let last = call.trim_end().trim_end_matches(',').rsplit(',').next()?.trim();
            let name = match last.strip_prefix("Code::") {
                Some(name) => name,
                None => last.strip_prefix('"')?.strip_suffix('"')?,
            };
            Some(name.to_owned())
        })
        .collect();
    found.sort_unstable();
    found.dedup();
    found
}

fn read(doc: &str) -> String {
    fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(doc)).unwrap()
}

/// Text from the documents, each with where it is (`README.md:42`).
type Found = Vec<(String, String)>;

/// Every document's `sh` lines, comments left out, and its `toml` blocks.
fn documents() -> (Found, Found) {
    let (mut lines, mut tomls) = (Vec::new(), Vec::new());
    for doc in DOCS {
        let text = read(doc);
        let blocks = if doc.ends_with(".html") { html_blocks(&text) } else { md_blocks(&text) };
        for (lang, start, body) in blocks {
            let at = format!("{doc}:{start}");
            match lang.as_str() {
                "toml" => tomls.push((at, body)),
                "sh" => {
                    for (n, line) in body.lines().enumerate() {
                        let (_, end) = lex(line);
                        let code = line[..end].split_whitespace().collect::<Vec<_>>().join(" ");
                        if !code.is_empty() {
                            lines.push((format!("{doc}:{}", start + n), code));
                        }
                    }
                }
                _ => {}
            }
        }
    }
    (lines, tomls)
}

/// A Markdown document's fenced blocks: language, first line, text.
fn md_blocks(text: &str) -> Vec<(String, usize, String)> {
    let mut blocks = Vec::new();
    let mut open: Option<(String, usize, String)> = None;
    for (n, line) in text.lines().enumerate() {
        match (line.strip_prefix("```"), open.take()) {
            (Some(lang), None) => open = Some((lang.trim().to_owned(), n + 2, String::new())),
            (Some(_), Some(block)) => blocks.push(block),
            (None, Some((lang, start, mut body))) => {
                body.push_str(line);
                body.push('\n');
                open = Some((lang, start, body));
            }
            (None, None) => {}
        }
    }
    blocks
}

/// An HTML document's `<pre>` blocks, markup left out: `sh` if they start
/// with a brnr command, `toml` if they have a profile, and otherwise text
/// (a diagram).
fn html_blocks(text: &str) -> Vec<(String, usize, String)> {
    let mut blocks = Vec::new();
    for (i, _) in text.match_indices("<pre>") {
        let start = i + "<pre>".len();
        let end = start + text[start..].find("</pre>").unwrap();
        let mut body = String::new();
        let mut in_tag = false;
        for c in text[start..end].chars() {
            match c {
                '<' => in_tag = true,
                '>' if in_tag => in_tag = false,
                c if !in_tag => body.push(c),
                _ => {}
            }
        }
        for (entity, c) in [("&lt;", "<"), ("&gt;", ">"), ("&quot;", "\""), ("&#39;", "'")] {
            body = body.replace(entity, c);
        }
        body = body.replace("&amp;", "&");
        let lang = if body.starts_with("brnr ") {
            "sh"
        } else if body.lines().any(|l| l.starts_with("[profiles")) {
            "toml"
        } else {
            "text"
        };
        let line = text[..start].matches('\n').count() + 1;
        blocks.push((lang.to_owned(), line, body + "\n"));
    }
    blocks
}

/// Whether `text` has `word`, not as the start of a longer one.
fn mentions(text: &str, word: &str) -> bool {
    text.match_indices(word).any(|(i, _)| {
        !text[i + word.len()..].starts_with(|c: char| c.is_ascii_alphanumeric() || c == '-')
    })
}

// ---- the examples as commands ------------------------------------------

/// One word of a command line, as sh would split it, and whether any of it
/// was quoted.
#[derive(Clone, Debug)]
struct Word {
    text: String,
    quoted: bool,
}

/// A line's words, as sh splits them (quotes, but no expansion: `$s` stays
/// as it is), and where its comment starts.
fn lex(line: &str) -> (Vec<Word>, usize) {
    let mut words = Vec::new();
    let mut word: Option<Word> = None;
    let mut quote = None;
    for (i, c) in line.char_indices() {
        match (quote, c) {
            (None, '#') if word.is_none() => {
                return (words, i);
            }
            (None, c) if c.is_whitespace() => words.extend(word.take()),
            (None, '\'' | '"') => {
                quote = Some(c);
                word.get_or_insert(Word { text: String::new(), quoted: true }).quoted = true;
            }
            (Some(q), c) if c == q => quote = None,
            (_, c) => word.get_or_insert(Word { text: String::new(), quoted: false }).text.push(c),
        }
    }
    words.extend(word);
    (words, line.len())
}

/// The command lines an example stands for: an optional argument
/// (`[plan]`) once left out, and once given.
fn variants((words, _): &(Vec<Word>, usize)) -> Vec<Vec<Word>> {
    let optional = |w: &Word| !w.quoted && w.text.starts_with('[') && w.text.ends_with(']');
    let mut all = vec![Vec::new()];
    for w in words {
        if optional(w) {
            let given = Word { text: w.text[1..w.text.len() - 1].to_owned(), quoted: false };
            let mut with: Vec<Vec<Word>> = all.clone();
            with.iter_mut().for_each(|v| v.push(given.clone()));
            all.extend(with);
        } else {
            all.iter_mut().for_each(|v| v.push(w.clone()));
        }
    }
    all
}

/// Runs one example in an environment of its own, `name`.
fn run(name: &str, words: &[Word], before: Before, then: Then) -> Result<(), String> {
    let env = scratch(name, before, &[], false);
    let dir = env.dir.clone();
    let s = "sess-1";
    let (mut cmd, stdout) = command(&env, words)?;
    cmd.stderr(File::create(dir.join("stderr")).unwrap());
    let mut child = with_default_interrupt(&mut cmd).spawn().unwrap();
    let printed = || fs::read_to_string(dir.join(&stdout)).unwrap_or_default();
    let stderr = || fs::read_to_string(dir.join("stderr")).unwrap_or_default();
    let never = |what| Err(format!("never printed {what:?}"));

    let shown = match then {
        Succeeds | Says(_) => Ok(()),
        Follows(what) => {
            let ok = wait_for(Duration::from_secs(10), || {
                env.run(&["prompt", "send", s, "--prompt", "reply readme"]);
                sleep(Duration::from_millis(200));
                printed().contains(what) || stderr().contains(what)
            });
            env.run(&["session", "close", s]);
            if ok { Ok(()) } else { never(what) }
        }
        RunsUntilInterrupted(what) => {
            let ok = wait_for(Duration::from_secs(10), || printed().contains(what));
            kill(child.id() as i32, libc::SIGINT);
            if ok { Ok(()) } else { never(what) }
        }
    };
    let status = finish(&mut child);
    let (out, stderr) = (printed(), stderr());
    let report = |e: String| format!("{e}\nstdout:\n{out}stderr:\n{stderr}");
    shown.map_err(report)?;
    match status {
        None => return Err(report(format!("still running after {TIMEOUT:?}"))),
        Some(false) => return Err(report("exited non-zero".into())),
        Some(true) => {}
    }
    if let Says(what) = then
        && !out.contains(what)
    {
        return Err(report(format!("didn't print {what:?}")));
    }
    Ok(())
}

/// Whether `child` exited 0; `None`, killing it, if it runs past
/// [`TIMEOUT`].
fn finish(child: &mut Child) -> Option<bool> {
    if !wait_for(TIMEOUT, || child.try_wait().unwrap().is_some()) {
        let _ = child.kill();
        let _ = child.wait();
        return None;
    }
    Some(child.wait().unwrap().success())
}

/// An environment of its own, `name`, for an example: the config (with
/// [`BRIDGE`] started from the default profile if `bridge`), the files the
/// examples name, curl's stand-in, the fake agent's `agent` variables, and
/// `$s` in the state `before` says.
fn scratch(name: &str, before: Before, agent: &[(String, String)], bridge: bool) -> Env {
    let env = agent.iter().fold(Env::new(name), |env, (k, v)| env.agent(k, v));
    let dir = env.dir.clone();
    let agent = format!("[{AGENT:?}]");
    let mut config =
        format!("[profiles.default]\nagent = {agent}\n\n[profiles.work]\nagent = {agent}\n");
    fs::create_dir_all(dir.join("project")).unwrap();
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::create_dir_all(dir.join("bin")).unwrap();
    if bridge {
        script(&dir.join("bin/bridge"), BRIDGE);
        let (command, args) = (dir.join("bin/bridge"), [dir.to_string_lossy()]);
        config.push_str(&format!(
            "\n[[profiles.default.bridges]]\ncommand = {command:?}\nargs = {args:?}\n"
        ));
    }
    env.write_config(&config);
    fs::write(dir.join("task.md"), "reply readme\n").unwrap();
    fs::write(dir.join("src/api.rs"), "pub fn api() {}\n").unwrap();
    fs::write(dir.join("screenshot.png"), b"\x89PNG\r\n\x1a\n").unwrap();
    script(&dir.join("bin/curl"), "#!/bin/sh\necho curl \"$@\"\n");

    let s = "sess-1";
    match before {
        Nothing => {}
        Idle | Ended => drop(env.start(&["--wait", "--prompt", "reply hi"])),
        Turn => drop(env.start(&["--prompt", "hang"])),
        Slow => drop(env.start(&["--prompt", "slow 1"])),
        Approval => {
            env.start(&["--prompt", "perm edit"]);
            env.ok(&["event", "wait", s, "--for", "permission", "--timeout", "10"]);
        }
        Elicitation => {
            env.start(&["--prompt", "elicit form"]);
            env.ok(&["event", "wait", s, "--for", "elicitation", "--timeout", "10"]);
        }
    }
    if let Ended = before {
        env.ok(&["session", "close", s]);
    }
    env
}

/// The command `words` are, as sh would run it in `env`'s scratch directory,
/// [`STAND_INS`] in place, its stdin from the file after `<`; and the file
/// its stdout goes to, the one after `>` or `stdout`.
fn command(env: &Env, words: &[Word]) -> Result<(std::process::Command, String), String> {
    let (dir, s) = (&env.dir, "sess-1");
    let (mut args, mut stdin, mut stdout) = (Vec::new(), None, "stdout".to_owned());
    let mut it = words.iter();
    while let Some(w) = it.next() {
        let text = STAND_INS.iter().find(|(from, _)| *from == w.text).map_or(&*w.text, |s| s.1);
        let text = match text {
            "{pid}" => env.pid(),
            _ => text.to_owned(),
        };
        let text = text
            .replace("{s}", s)
            .replace("{agent}", AGENT)
            .replace("{dir}", &dir.to_string_lossy());
        match (w.quoted, &*text) {
            (false, "<") => stdin = Some(it.next().ok_or("< and no file")?.text.clone()),
            (false, ">") => stdout = it.next().ok_or("> and no file")?.text.clone(),
            _ => args.push(text),
        }
    }
    if args.first().map(String::as_str) != Some("brnr") {
        return Err("not a brnr command".into());
    }
    let path = format!("{}:{}", dir.join("bin").display(), std::env::var("PATH").unwrap());
    let args: Vec<&str> = args[1..].iter().map(String::as_str).collect();
    let mut cmd = env.brnr(&args);
    // HOME is the scratch directory: `skill install` writes under it. The
    // ntfy example needs its topic set.
    cmd.env("PATH", path)
        .env("HOME", dir)
        .env("NTFY_TOPIC", "brnr-docs-topic")
        .stdin(match &stdin {
            Some(file) => Stdio::from(File::open(dir.join(file)).unwrap()),
            None => Stdio::null(),
        })
        .stdout(File::create(dir.join(&stdout)).unwrap());
    Ok((cmd, stdout))
}

// ---- the reference's JSON ------------------------------------------------

/// A started bridge for the reference's examples of the bridge protocol: once
/// `<dir>/bridge.in` exists, it writes it on its stdout, the requests; it
/// appends every line on its stdin, the replies and events, to
/// `<dir>/bridge.out`; it exits when its stdin ends.
const BRIDGE: &str = r#"#!/usr/bin/env python3
import os, sys, threading, time
d = sys.argv[1]
def ask():
    path = os.path.join(d, "bridge.in")
    while not os.path.exists(path):
        time.sleep(0.02)
    with open(path) as f:
        sys.stdout.write(f.read())
    sys.stdout.flush()
threading.Thread(target=ask, daemon=True).start()
with open(os.path.join(d, "bridge.out"), "a") as out:
    for line in sys.stdin:
        out.write(line)
        out.flush()
"#;

/// A `json` block of the reference, and what the test knows of it.
struct Json {
    /// `docs/reference/session.md:120`.
    at: String,
    page: &'static str,
    /// The heading it is under, `## \`brnr session status\``.
    heading: String,
    /// Its `<!-- example: … -->`, if it has one.
    spec: Option<String>,
    text: String,
    /// The rows of its section's fields tables (`| Field | Type | Meaning |`),
    /// then its page's intro's.
    rows: Vec<Row>,
}

/// A row of a fields table: the field's full name, an object's field after a
/// dot and a list's item as `[]` (`last_turn.stop_reason`,
/// `dropped[].reason`); the JSON types its type names (none for `any`, or
/// a row without a type, which says where it is described);
/// whether it says "or null"; whether it says "optional", a field not
/// always there; and whether it says "ACP's", a value ACP defines, whose
/// fields have no rows.
#[derive(Clone)]
struct Row {
    name: String,
    kinds: Vec<&'static str>,
    null: bool,
    optional: bool,
    acp: bool,
}

/// The row of field `name` whose type is `ty`, as the reference writes types
/// (overview.md, "How to read it"): `string or null`, `list of objects`,
/// `string, number or null`, `number, optional`, `object, ACP's`.
fn row(name: &str, ty: &str) -> Result<Row, String> {
    let mut row =
        Row { name: name.to_owned(), kinds: vec![], null: false, optional: false, acp: false };
    for word in ty.split(", ").flat_map(|t| t.split(" or ")).map(str::trim) {
        match word {
            "null" => row.null = true,
            "optional" => row.optional = true,
            "ACP's" => row.acp = true,
            "any" | "" => {}
            _ if word.starts_with("list") => row.kinds.push("a list"),
            "string" | "`\"all\"`" => row.kinds.push("a string"),
            "number" => row.kinds.push("a number"),
            "boolean" => row.kinds.push("a boolean"),
            "object" => row.kinds.push("an object"),
            _ => return Err(format!("`{name}`: {word}: not a type the test knows")),
        }
    }
    Ok(row)
}

/// Whether each of `values` has a type its `row` names, into `problems`: a
/// `number` is an integer (overview.md), and a null needs "or null".
fn typed<const N: usize>(
    row: &Row,
    values: [(&str, &Value); N],
    at: &str,
    problems: &mut Vec<String>,
) {
    for (side, v) in values {
        if v.is_null() {
            if !row.null && N == 1 {
                problems.push(format!("{at}: {side} null, and its row doesn't say \"or null\""));
            }
        } else if !row.kinds.is_empty() && !row.kinds.contains(&kind(v)) {
            let types = row.kinds.join(" or ");
            problems.push(format!("{at}: {side} {}, and its row says {types}", kind(v)));
        } else if row.kinds.contains(&"a number") && v.is_number() && !(v.is_i64() || v.is_u64()) {
            problems.push(format!("{at}: {side} {v}, and a number is an integer"));
        }
    }
}

/// A JSON value's type, as a problem names it.
fn kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "a list",
        Value::Object(_) => "an object",
    }
}

/// Every `json` block of the reference's pages.
fn json_blocks() -> Vec<Json> {
    let mut blocks = Vec::new();
    for &page in REFERENCE {
        let text = read(page);
        let lines: Vec<&str> = text.lines().collect();
        // The rows of each section, by the line its heading is on.
        let mut sections: Vec<(usize, Vec<Row>)> = vec![(0, Vec::new())];
        let mut in_table = false;
        let mut fence = false;
        for (n, line) in lines.iter().enumerate() {
            if line.starts_with("```") {
                fence = !fence;
            }
            if fence {
                continue;
            }
            // The title's section, the page's intro, is every section's.
            if line.starts_with("##") {
                sections.push((n, Default::default()));
            }
            if line.starts_with("| Field | Type |") {
                in_table = true;
            } else if !line.starts_with('|') {
                in_table = false;
            } else if in_table && line.starts_with("| `") {
                let cells: Vec<&str> = line.split('|').map(str::trim).collect();
                // `mode`, `model`: a row of more than one field.
                for name in cells[1].split('`').skip(1).step_by(2) {
                    let row = row(name, cells.get(2).copied().unwrap_or_default())
                        .unwrap_or_else(|e| panic!("{page}:{}: {e}", n + 1));
                    sections.last_mut().unwrap().1.push(row);
                }
            }
        }
        // A section's rows first: they are its fields where the intro has
        // one of the same name.
        let rows_at = |n: usize| -> Vec<Row> {
            let mine = &sections.iter().rfind(|(at, _)| *at <= n).unwrap().1;
            mine.iter().chain(&sections[0].1).cloned().collect()
        };
        for (lang, start, body) in md_blocks(&text) {
            if lang != "json" {
                continue;
            }
            // The fence is on line `start - 1`, counting from 1.
            let fence = start - 2;
            let above = lines[..fence].iter().rev().find(|l| !l.trim().is_empty());
            let spec = above
                .and_then(|l| l.trim().strip_prefix("<!-- example: "))
                .and_then(|l| l.strip_suffix("-->"))
                .map(|l| l.trim().to_owned());
            let heading = lines[..fence].iter().rev().find(|l| l.starts_with('#'));
            blocks.push(Json {
                at: format!("{page}:{start}"),
                page,
                heading: heading.map_or("", |h| h).to_owned(),
                spec,
                text: body,
                rows: rows_at(fence),
            });
        }
    }
    blocks
}

/// What an `<!-- example: … -->` says: the commands to run, `$s` in the
/// state `before` names; the event to pick of the last one's lines; the exit
/// status it must have; the fake agent's variables. Written
/// `<!-- example: <command>[ && <command>]...; before: <state>[; event:
/// <name>[ <field>=<value>]][; exit: <n>][; agent: <VAR>=<value>]... -->`,
/// `<field>` dotted (`msg.method`); `bridge` in place of the commands for a
/// bridge's request and reply. Each command but the last must exit 0.
struct Spec {
    commands: Vec<Vec<Word>>,
    bridge: bool,
    before: Before,
    event: Option<String>,
    exit: i32,
    agent: Vec<(String, String)>,
}

fn spec(text: &str) -> Result<Spec, String> {
    let mut parts = text.split("; ");
    let commands = parts.next().unwrap_or_default();
    let bridge = commands == "bridge";
    let commands = match bridge {
        true => Vec::new(),
        false => commands.split(" && ").map(|c| lex(c).0).collect(),
    };
    let mut spec = Spec { commands, bridge, before: Nothing, event: None, exit: 0, agent: vec![] };
    for part in parts {
        let (key, value) = part.split_once(": ").ok_or(format!("{part}: not <key>: <value>"))?;
        match key {
            "before" => {
                spec.before = match value {
                    "Nothing" => Nothing,
                    "Idle" => Idle,
                    "Turn" => Turn,
                    "Slow" => Slow,
                    "Approval" => Approval,
                    "Elicitation" => Elicitation,
                    "Ended" => Ended,
                    _ => return Err(format!("before: {value}: no such state")),
                }
            }
            "event" => spec.event = Some(value.to_owned()),
            "exit" => spec.exit = value.parse().map_err(|_| format!("exit: {value}"))?,
            "agent" => {
                let (k, v) = value.split_once('=').ok_or(format!("agent: {value}: not K=V"))?;
                spec.agent.push((k.to_owned(), v.to_owned()));
            }
            _ => return Err(format!("{key}: not before, event, exit or agent")),
        }
    }
    Ok(spec)
}

/// Runs the example `block` is, in an environment of its own, `name`, and
/// checks that what brnr prints has its shape.
fn run_json(name: &str, block: &Json) -> Result<(), String> {
    let spec = spec(block.spec.as_deref().unwrap_or_default())?;
    let env = scratch(name, spec.before, &spec.agent, spec.bridge);
    let (example, real) = match spec.bridge {
        true => bridge_exchange(&env, &block.text)?,
        false => {
            let mut printed = String::new();
            for (i, words) in spec.commands.iter().enumerate() {
                let (mut cmd, stdout) = command(&env, words)?;
                cmd.stderr(File::create(env.dir.join("stderr")).unwrap());
                let mut child = cmd.spawn().unwrap();
                let status = match wait_for(TIMEOUT, || child.try_wait().unwrap().is_some()) {
                    true => child.wait().unwrap().code(),
                    false => {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(format!("still running after {TIMEOUT:?}"));
                    }
                };
                printed = fs::read_to_string(env.dir.join(&stdout)).unwrap_or_default();
                let stderr = fs::read_to_string(env.dir.join("stderr")).unwrap_or_default();
                let wanted = if i + 1 == spec.commands.len() { spec.exit } else { 0 };
                if status != Some(wanted) {
                    let n = i + 1;
                    return Err(format!(
                        "command {n} exited {status:?}, not {wanted}\nstdout:\n{printed}stderr:\n{stderr}"
                    ));
                }
            }
            let real: Value = match &spec.event {
                None => serde_json::from_str(&printed).map_err(|e| format!("{e}: {printed}"))?,
                Some(event) => {
                    // `acp msg.method=session/prompt`: the first `acp` event
                    // whose `msg.method` is `session/prompt`.
                    let (name, field) = event.split_once(' ').unwrap_or((event, ""));
                    let (path, value) = field.split_once('=').unwrap_or_default();
                    let pointer = format!("/{}", path.replace('.', "/"));
                    let picked = |e: &Value| {
                        e["event"] == name
                            && (path.is_empty() || e.pointer(&pointer) == Some(&json!(value)))
                    };
                    printed
                        .lines()
                        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
                        .find(picked)
                        .ok_or(format!("no {event} in what it printed:\n{printed}"))?
                }
            };
            let example: Value =
                serde_json::from_str(&block.text).map_err(|e| format!("the example: {e}"))?;
            (example, real)
        }
    };
    let mut problems = Vec::new();
    unlisted(&example, "", &block.rows, &mut problems);
    shape(&example, &real, ("", ""), false, &block.rows, &mut problems);
    match problems.is_empty() {
        true => Ok(()),
        false => Err(format!(
            "{}\nbrnr printed:\n{}",
            problems.join("\n"),
            serde_json::to_string_pretty(&real).unwrap()
        )),
    }
}

/// The example's request and reply, `text`'s two JSON values, and the reply
/// [`BRIDGE`], started in `env`, got when it sent the request.
fn bridge_exchange(env: &Env, text: &str) -> Result<(Value, Value), String> {
    let values: Vec<Value> = serde_json::Deserializer::from_str(text)
        .into_iter()
        .collect::<Result<_, _>>()
        .map_err(|e| format!("the example: {e}"))?;
    let [request, reply] = &values[..] else {
        return Err("a bridge example is a request and its reply".into());
    };
    let id = request["req_id"].as_str().ok_or("the request has no req_id")?;
    let draft = env.dir.join("bridge.draft");
    fs::write(&draft, format!("{request}\n")).unwrap();
    fs::rename(&draft, env.dir.join("bridge.in")).unwrap();
    let mut answer = None;
    wait_for(TIMEOUT, || {
        let out = fs::read_to_string(env.dir.join("bridge.out")).unwrap_or_default();
        let lines = out.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok());
        answer = lines.into_iter().find(|v| v["req_id"] == id);
        answer.is_some()
    });
    let answer = answer.ok_or(format!("no reply to {id} in {TIMEOUT:?}"))?;
    Ok((reply.clone(), answer))
}

/// Each field of `example`, the value at `path`, at every level, that has no
/// row in `rows`, into `problems`: every item of a list, but not the fields
/// of an ACP's value. A bridge's `ok`, `req_id` and `cmd` are the protocol's,
/// in every message.
fn unlisted(example: &Value, path: &str, rows: &[Row], problems: &mut Vec<String>) {
    match example {
        Value::Object(o) => {
            for (k, v) in o {
                let name = if path.is_empty() { k.clone() } else { format!("{path}.{k}") };
                if path.is_empty() && ["ok", "req_id", "cmd"].contains(&k.as_str()) {
                    continue;
                }
                match rows.iter().find(|r| r.name == name) {
                    None => problems.push(format!("{name}: in no fields table")),
                    Some(row) if row.acp => continue,
                    Some(_) => {}
                }
                unlisted(v, &name, rows, problems);
            }
        }
        Value::Array(items) => {
            for item in items {
                unlisted(item, &format!("{path}[]"), rows, problems);
            }
        }
        _ => {}
    }
}

/// Where `example` and `real` differ in shape, into `problems`: the keys at
/// every level, but where the row of one says "optional"; each value's JSON
/// type, the same on both sides and one its row names; and every item of a
/// list, each with the example's item of the same index, else its first. A
/// null, on either side, only where the row says "or null"; inside an ACP's
/// value (`inside`), which has no rows, where both sides have it. `path` names
/// the field `example` is the value of, as its row does
/// (`options[].choices`); `at`, as a problem does (`options[1].choices`).
fn shape(
    example: &Value,
    real: &Value,
    (path, at): (&str, &str),
    inside: bool,
    rows: &[Row],
    problems: &mut Vec<String>,
) {
    let row = rows.iter().find(|r| !inside && r.name == path);
    let acp = inside || row.is_some_and(|r| r.acp);
    let at_ = if at.is_empty() { "the example" } else { at };
    if let Some(row) = row {
        typed(row, [("the example has", example), ("brnr printed", real)], at_, problems);
    }
    match (example, real) {
        (Value::Null, _) | (_, Value::Null) if !inside || example.is_null() != real.is_null() => {
            if !row.is_some_and(|r| r.null) {
                problems.push(format!(
                    "{at_}: the example has {}, brnr printed {}, and its row doesn't say \"or null\"",
                    kind(example),
                    kind(real)
                ));
            }
        }
        (Value::Object(a), Value::Object(b)) => {
            let dot =
                |p: &str, k: &str| if p.is_empty() { k.to_owned() } else { format!("{p}.{k}") };
            let optional = |k: &str| rows.iter().any(|r| r.name == dot(path, k) && r.optional);
            for k in a.keys().filter(|k| !b.contains_key(*k) && !optional(k)) {
                problems.push(format!("{}: in the example, not in what brnr printed", dot(at, k)));
            }
            for k in b.keys().filter(|k| !a.contains_key(*k) && !optional(k)) {
                problems.push(format!("{}: printed by brnr, not in the example", dot(at, k)));
            }
            // An optional field on one side only: its value still has its row's type.
            let one_side = (a.iter().filter(|(k, _)| !b.contains_key(*k)))
                .map(|(k, v)| (k, "the example has", v))
                .chain(
                    b.iter()
                        .filter(|(k, _)| !a.contains_key(*k))
                        .map(|(k, v)| (k, "brnr printed", v)),
                );
            for (k, side, v) in one_side.filter(|(k, ..)| optional(k)) {
                if let Some(row) = rows.iter().find(|r| !acp && r.name == dot(path, k)) {
                    typed(row, [(side, v)], &dot(at, k), problems);
                }
            }
            for (k, v) in a {
                if let Some(real) = b.get(k) {
                    shape(v, real, (&dot(path, k), &dot(at, k)), acp, rows, problems);
                }
            }
        }
        (Value::Array(a), Value::Array(b)) => {
            if let Some(first) = a.first() {
                for (i, real) in b.iter().enumerate() {
                    let example = a.get(i).unwrap_or(first);
                    let (path, at) = (format!("{path}[]"), format!("{at}[{i}]"));
                    shape(example, real, (&path, &at), acp, rows, problems);
                }
            }
        }
        _ if kind(example) != kind(real) => {
            problems.push(format!(
                "{at_}: the example has {}, brnr printed {}",
                kind(example),
                kind(real)
            ));
        }
        _ => {}
    }
}
