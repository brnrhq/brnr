//! The claims of docs/threat-model.md that no other test checks: who can
//! reach a process and its files (P13), what the agent's text can't do
//! (P8), how approvals end unanswered (ADR 27), what is recorded of secrets
//! (ADR 25), and what `brnr acp` passes unchanged (P1). Against the fake
//! agent (fake_agent.py), each test in its own directories.

mod common;

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread::sleep;
use std::time::Duration;

use common::*;
use serde_json::{Value, json};

fn mode(path: &Path) -> u32 {
    fs::symlink_metadata(path).unwrap_or_else(|e| panic!("{}: {e}", path.display())).mode() & 0o777
}

fn chmod(path: &Path, mode: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

/// Every path under `dir`, `dir` first.
fn tree(dir: &Path) -> Vec<PathBuf> {
    let mut paths = vec![dir.to_owned()];
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir()) {
            paths.extend(tree(&path));
        } else {
            paths.push(path);
        }
    }
    paths
}

/// Everything brnr has written under `dir`, as text.
fn everything_in(dir: &Path) -> String {
    let files = tree(dir).into_iter().filter(|p| p.is_file());
    files.map(|p| String::from_utf8_lossy(&fs::read(p).unwrap()).into_owned()).collect()
}

fn private_dir_error(err: &str) {
    assert!(err.contains("not a private directory owned by this user"), "{err}");
}

// ---- the runtime directory and its sockets (P13) ----------------------

/// What brnr makes is private to the user whatever the umask: a umask of 0
/// would otherwise leave the runtime directory, the socket, the session locks
/// and the transcripts open to everyone.
#[test]
fn what_brnr_makes_is_private_whatever_the_umask() {
    let env = Env::new("s-umask");
    let mut start = env.brnr(&start_args(&["--prompt", "hello"]));
    let out = with_umask(&mut start, 0).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(wait_for(Duration::from_secs(5), || !env.prompts().is_empty()));
    let run = env.dir.join("run");
    assert_eq!(mode(&run), 0o700);
    assert_eq!(mode(&run.join(format!("{}.sock", env.pid()))), 0o600);
    assert_eq!(mode(&run.join("sessions")), 0o700);
    assert_eq!(mode(&run.join("sessions/sess-1.lock")), 0o600);
    // The transcripts are the logger thread's to write (ADR 48).
    env.ok(&["event", "log", "sess-1"]);
    let home = tree(&env.dir.join("home"));
    assert!(home.len() >= 7, "{home:?}");
    for path in home {
        let want = if path.is_dir() { 0o700 } else { 0o600 };
        assert_eq!(mode(&path), want, "{}", path.display());
    }
    env.stop();
}

/// A runtime directory anyone else can read, search or write is refused by
/// the process, which then writes nothing there, and by every command that
/// would read it; brnr doesn't change its mode itself (only `doctor --fix`
/// does).
#[test]
fn a_runtime_dir_others_can_use_is_refused() {
    for open in [0o701, 0o705, 0o750, 0o770, 0o777] {
        let env = Env::new(&format!("s-open{open:o}"));
        let run = env.dir.join("run");
        fs::create_dir(&run).unwrap();
        chmod(&run, open);
        private_dir_error(&stderr(&env.run(&start_args(&[]))));
        assert!(fs::read_dir(&run).unwrap().next().is_none(), "{open:o}: wrote into it");
        assert!(env.calls().is_empty(), "{open:o}: an agent was started");
        for args in [
            &["list"][..],
            &["process", "list"],
            &["permission", "requests"],
            &["prompt", "send", "sess-1", "hi"],
        ] {
            private_dir_error(&env.fails(args));
        }
        assert_eq!(mode(&run), open);
    }
}

/// A symlinked runtime directory is refused by every command, not only by
/// the process (`runtime_dir_symlink_is_refused`): it could point at any
/// private directory of the user's, or at one someone else made it point at.
#[test]
fn a_symlinked_runtime_dir_is_refused_by_every_command() {
    let env = Env::new("s-symlink");
    let target = env.dir.join("elsewhere");
    fs::create_dir(&target).unwrap();
    chmod(&target, 0o700);
    symlink(&target, env.dir.join("run")).unwrap();
    for args in [
        &["list"][..],
        &["process", "list"],
        &["permission", "requests"],
        &["approve", "sess-1", "p1"],
    ] {
        private_dir_error(&env.fails(args));
    }
    assert!(fs::read_dir(&target).unwrap().next().is_none());
}

/// A private directory of another user's (root's) is refused for being
/// theirs, mode aside. Skipped where there is none to try, or as root.
#[test]
fn a_runtime_dir_of_another_users_is_refused() {
    let ours = uid();
    let theirs = ["/root", "/var/audit", "/private/var/backups"].into_iter().find(|dir| {
        fs::symlink_metadata(dir)
            .is_ok_and(|m| m.is_dir() && m.uid() != ours && m.mode() & 0o077 == 0)
    });
    let Some(dir) = theirs.filter(|_| ours != 0) else {
        eprintln!("skipped: no private directory of another user's to try");
        return;
    };
    let env = Env::new("s-theirs");
    let out = env.brnr(&start_args(&[])).env("BRNR_DIR", dir).output().unwrap();
    private_dir_error(&stderr(&out));
    let out = env.brnr(&["list"]).env("BRNR_DIR", dir).output().unwrap();
    assert!(!out.status.success());
    private_dir_error(&stderr(&out));
}

/// A runtime directory whose `sessions/` is the given mode, and that.
fn session_locks(env: &Env, mode: u32) -> PathBuf {
    let sessions = env.dir.join("run/sessions");
    fs::create_dir_all(&sessions).unwrap();
    chmod(&env.dir.join("run"), 0o700);
    chmod(&sessions, mode);
    sessions
}

/// Whether the one host log says a session's lock couldn't be taken.
fn lock_failed(env: &Env) -> bool {
    let wait = || everything_in(&env.dir.join("home/hosts")).contains(r#""event":"lock-failed""#);
    wait_for(Duration::from_secs(5), wait)
}

/// The session locks' directory is held to the runtime directory's terms,
/// and a lock that is a symlink is never followed. Both a resume and a new
/// headless session are refused when their lock can't be taken (ADR 50),
/// and the host log says why the new session failed.
#[test]
fn adr_0003_session_locks_are_private_and_never_followed() {
    let env = Env::new("s-locks");
    let sessions = session_locks(&env, 0o777);
    private_dir_error(&stderr(&env.run(&start_args(&["--resume", "old-1"]))));
    assert!(env.calls_of("session/resume").is_empty());
    private_dir_error(&env.fails(&start_args(&[])));
    assert!(lock_failed(&env), "no lock-failed");
    assert!(fs::read_dir(&sessions).unwrap().next().is_none(), "a lock written there");

    let env = Env::new("s-locklink");
    let sessions = session_locks(&env, 0o700);
    let victim = env.dir.join("victim");
    fs::write(&victim, "keep me").unwrap();
    symlink(&victim, sessions.join("old-1.lock")).unwrap();
    symlink(&victim, sessions.join("sess-1.lock")).unwrap();
    assert!(!env.run(&start_args(&["--resume", "old-1"])).status.success());
    let err = env.fails(&start_args(&[]));
    assert!(err.contains("the agent opened sess-1, which can't be locked:"), "{err}");
    assert!(lock_failed(&env), "no lock-failed");
    assert_eq!(fs::read_to_string(&victim).unwrap(), "keep me");
}

/// A command doesn't talk to a process through a runtime directory others
/// can use: an approval isn't sent through one, and the request waits until
/// it is private again.
#[test]
fn no_approval_goes_through_a_dir_others_can_use() {
    let env = Env::new("s-approve");
    env.start(&[]);
    env.ok(&["prompt", "send", "sess-1", "perm edit"]);
    assert!(wait_for(Duration::from_secs(5), || env
        .ok(&["permission", "requests"])
        .contains("p1")));
    let run = env.dir.join("run");
    chmod(&run, 0o770);
    private_dir_error(&env.fails(&["approve", "sess-1", "p1"]));
    chmod(&run, 0o700);
    assert!(env.ok(&["permission", "requests"]).contains("p1"));
    assert!(outcome(&env, "perm-1").is_none());
    env.ok(&["deny", "sess-1", "p1"]);
    assert!(wait_for(Duration::from_secs(5), || outcome(&env, "perm-1").is_some()));
}

/// Another user can't connect to a process's socket: the runtime directory
/// is closed to them, and the socket itself too. Needs root, to be someone
/// else (nobody); skipped otherwise.
#[test]
fn another_user_cant_connect() {
    if uid() != 0 {
        eprintln!("skipped: needs root to connect as another user");
        return;
    }
    let env = Env::new("s-nobody");
    chmod(&env.dir, 0o755);
    env.start(&[]);
    let run = env.dir.join("run");
    let socket = run.join(format!("{}.sock", env.pid()));
    let connect = |socket: &Path| {
        let script = "import socket, sys\ns = socket.socket(socket.AF_UNIX)\ntry:\n    s.connect(sys.argv[1])\nexcept PermissionError:\n    sys.exit(13)\n";
        let out = Command::new("python3")
            .args(["-I", "-c", script])
            .arg(socket)
            .uid(65534)
            .gid(65534)
            .current_dir("/")
            .output()
            .unwrap();
        out.status.code()
    };
    // First, that nobody could connect at all with both open: else what
    // keeps them out is some directory above this one.
    chmod(&run, 0o711);
    chmod(&socket, 0o666);
    let control = connect(&socket);
    chmod(&socket, 0o600);
    if control != Some(0) {
        eprintln!("skipped: nobody can't reach {} at all", env.dir.display());
        chmod(&run, 0o700);
        return env.stop();
    }
    // With the directory open for searching, the socket's own mode refuses.
    assert_eq!(connect(&socket), Some(13), "nobody connected to the socket");
    chmod(&run, 0o700);
    assert_eq!(connect(&socket), Some(13), "nobody connected through the directory");
    env.stop();
}

/// The process listens on no network: its only sockets are Unix sockets.
/// Skipped where `lsof` isn't installed.
#[test]
fn the_process_listens_on_no_network() {
    let env = Env::new("s-network");
    env.start(&["--prompt", "hello"]);
    assert!(wait_for(Duration::from_secs(5), || !env.prompts().is_empty()));
    let Ok(out) = Command::new("lsof").args(["-a", "-n", "-P", "-i", "-p", &env.pid()]).output()
    else {
        eprintln!("skipped: no lsof");
        return env.stop();
    };
    assert_eq!(String::from_utf8_lossy(&out.stdout), "", "internet sockets open");
    // lsof itself works: it sees the control socket.
    let all = Command::new("lsof").args(["-a", "-U", "-p", &env.pid()]).output().unwrap();
    assert!(!all.stdout.is_empty(), "lsof saw no Unix sockets either");
    env.stop();
}

// ---- transcripts (P13, ADR 59) ----------------------------------------

/// Session `old-1`'s project folder, once a process has served it and gone.
fn served_once(env: &Env) -> PathBuf {
    env.start(&["--resume", "old-1", "--wait", "--prompt", "reply hi"]);
    env.stop();
    assert!(wait_for(Duration::from_secs(15), || env.hosts().is_empty()));
    let project = fs::read_dir(env.dir.join("home/projects")).unwrap().next().unwrap().unwrap();
    project.path()
}

/// Each `made-private` event in the host logs: the path, and its mode.
fn made_private(env: &Env) -> Vec<(PathBuf, String)> {
    let text = everything_in(&env.dir.join("home/hosts"));
    let records = text.lines().map(|l| serde_json::from_str::<Value>(l).unwrap());
    let events = records.map(|r| r["event"].clone()).filter(|e| e["event"] == "made-private");
    events
        .map(|e| (e["path"].as_str().unwrap().into(), e["mode"].as_str().unwrap().into()))
        .collect()
}

/// A transcript others can read (restored from a backup, copied, chmod'ed)
/// is made private before the process that opens it again writes to it,
/// beneath private parents or parents anyone can search, and the host log
/// says what was; what was private already is left as it was.
#[test]
fn adr_0059_a_transcript_others_can_read_is_made_private_before_it_is_written() {
    for (name, parents) in [("s-reopen", 0o700), ("s-reopen-open", 0o755)] {
        let env = Env::new(name);
        let project = served_once(&env);
        let home = env.dir.join("home");
        let (events, raw) = (project.join("old-1.jsonl"), project.join("old-1.acp.jsonl"));
        let dirs = [home.clone(), home.join("hosts"), home.join("projects"), project.clone()];
        dirs.iter().for_each(|d| chmod(d, parents));
        chmod(&events, 0o644);
        chmod(&raw, 0o644);
        env.start(&["--resume", "old-1", "--wait", "--prompt", "reply private-token"]);
        env.ok(&["event", "log", "old-1"]); // Once the transcript is written (ADR 48).
        assert!(fs::read_to_string(&events).unwrap().contains("private-token"), "{name}");
        for path in tree(&home) {
            let want = if path.is_dir() { 0o700 } else { 0o600 };
            assert_eq!(mode(&path), want, "{name}: {}", path.display());
        }
        let opened = if parents == 0o755 { &dirs[..] } else { &[] };
        let mut want: Vec<(PathBuf, String)> =
            opened.iter().map(|d| (d.clone(), "755".into())).collect();
        want.extend([(events, "644".into()), (raw, "644".into())]);
        assert_eq!(made_private(&env), want, "{name}");
        env.stop();
    }
}

/// A transcript that is a symlink is never followed: the session is served
/// without it, and the host log says why. A state directory that is one is
/// refused before an agent's session starts.
#[test]
fn adr_0059_a_symlinked_transcript_is_never_followed() {
    let env = Env::new("s-loglink");
    let project = served_once(&env);
    let victim = env.dir.join("victim");
    fs::write(&victim, "keep me\n").unwrap();
    chmod(&victim, 0o644);
    let events = project.join("old-1.jsonl");
    fs::remove_file(&events).unwrap();
    symlink(&victim, &events).unwrap();
    env.start(&["--resume", "old-1", "--wait", "--prompt", "reply private-token"]);
    let failed = || everything_in(&env.dir.join("home/hosts")).contains("session-log-failed");
    assert!(wait_for(Duration::from_secs(5), failed), "no session-log-failed");
    let hosts = everything_in(&env.dir.join("home/hosts"));
    let why = format!("{}: is a symlink, so brnr won't write a transcript there", events.display());
    assert!(hosts.contains(&why), "{hosts}");
    assert_eq!(
        (fs::read_to_string(&victim).unwrap().as_str(), mode(&victim)),
        ("keep me\n", 0o644)
    );
    env.stop();

    let env = Env::new("s-homelink");
    let elsewhere = env.dir.join("elsewhere");
    fs::create_dir(&elsewhere).unwrap();
    symlink(&elsewhere, env.dir.join("home")).unwrap();
    let err = env.fails(&start_args(&["--prompt", "private-token"]));
    assert!(err.contains("home: is a symlink, so brnr won't write a transcript there"), "{err}");
    assert!(env.prompts().is_empty(), "prompted");
    assert!(fs::read_dir(&elsewhere).unwrap().next().is_none(), "wrote through it");
}

// ---- the agent's text (P8) ---------------------------------------------

/// A session id is the agent's text: as a file name it can't leave the
/// project folder or the locks' directory, and it is shown escaped.
#[test]
fn a_session_id_cant_climb_out_of_its_folder() {
    let id = "../../../\x1b[2J/escape";
    let env = Env::new("s-climb").agent("SESSION_ID", id);
    env.start(&["--prompt", "hello"]);
    assert!(wait_for(Duration::from_secs(5), || !env.prompts().is_empty()));
    // As P8 has it (ADR 53): anything but lowercase ASCII letters, digits,
    // `-` and `_` is escaped, byte by byte.
    let file = "%2e%2e%2f%2e%2e%2f%2e%2e%2f%1b%5b2%4a%2fescape";
    // The transcript is the logger thread's to write (ADR 48).
    env.ok(&["event", "log", id]);
    let project = fs::read_dir(env.dir.join("home/projects")).unwrap().next().unwrap().unwrap();
    assert!(project.path().join(format!("{file}.jsonl")).is_file(), "{:?}", tree(&project.path()));
    assert!(env.dir.join(format!("run/sessions/{file}.lock")).is_file());
    let made = tree(&env.dir);
    let escaped: Vec<_> = made.iter().filter(|p| p.ends_with("escape")).collect();
    assert!(escaped.is_empty(), "{escaped:?}");
    let list = env.ok(&["list"]);
    assert!(!list.contains('\x1b') && list.contains("escape"), "{list:?}");
    env.ok(&["prompt", "send", id, "hi"]);
    env.stop();
}

// ---- approvals (ADR 27) ------------------------------------------------

/// What the agent was answered to permission request `request`, if it was.
fn outcome(env: &Env, request: &str) -> Option<Value> {
    env.calls()
        .into_iter()
        .find(|c| c["id"] == request && c.get("method").is_none())
        .map(|c| c["result"]["outcome"].clone())
}

/// Without `permission_timeout` a request waits for an answer for as long as
/// it takes: nothing answers it for the user.
#[test]
fn adr_0027_an_unanswered_request_waits() {
    let env = Env::new("s-waits");
    env.start(&[]);
    env.ok(&["prompt", "send", "sess-1", "perm execute"]);
    assert!(wait_for(Duration::from_secs(5), || env
        .ok(&["permission", "requests"])
        .contains("p1")));
    sleep(Duration::from_secs(3));
    assert!(outcome(&env, "perm-1").is_none(), "answered for the user");
    assert!(env.ok(&["permission", "requests"]).contains("p1"));
    env.ok(&["approve", "sess-1", "p1"]);
    assert!(wait_for(Duration::from_secs(5), || outcome(&env, "perm-1").is_some()));
    assert_eq!(outcome(&env, "perm-1").unwrap()["optionId"], "allow");
}

/// A timeout only ever denies: a request that offers nothing to reject with
/// is cancelled, never allowed.
#[test]
fn adr_0027_a_timeout_never_allows() {
    let env = Env::new("s-allowonly").agent("ALLOW_ONLY", "1");
    env.write_config("[profiles.default.headless]\npermission_timeout = 1\n");
    env.start(&[]);
    env.ok(&["prompt", "send", "sess-1", "perm execute"]);
    assert!(wait_for(Duration::from_secs(5), || outcome(&env, "perm-1").is_some()), "never ended");
    assert_eq!(outcome(&env, "perm-1").unwrap(), json!({ "outcome": "cancelled" }));
    assert!(env.ok(&["event", "log", "sess-1"]).contains("by timeout"));
    env.stop();
}

/// A request is answered only as the request of its own session.
#[test]
fn adr_0027_a_request_is_answered_only_in_its_session() {
    let env = Env::new("s-othersession");
    env.start(&[]);
    env.ok(&["session", "fork", "sess-1"]);
    env.ok(&["prompt", "send", "sess-1", "perm edit"]);
    assert!(wait_for(Duration::from_secs(5), || env
        .ok(&["permission", "requests"])
        .contains("p1")));
    let err = env.fails(&["approve", "sess-2", "p1"]);
    assert!(err.contains("no pending request p1 in session sess-2"), "{err}");
    assert!(outcome(&env, "perm-1").is_none());
    env.stop();
}

// ---- secrets (ADR 25) --------------------------------------------------

/// The MCP servers' secrets of a resumed session, by `session/resume` or
/// `session/load`, reach the agent, and nothing brnr records or shows.
#[test]
fn adr_0025_a_resumed_sessions_secrets_are_redacted() {
    for (name, method, agent) in
        [("s-resume", "session/resume", None), ("s-load", "session/load", Some("NO_RESUME"))]
    {
        let mut env = Env::new(name);
        if let Some(var) = agent {
            env = env.agent(var, "1");
        }
        env.write_config(
            r#"[[profiles.default.headless.mcp_servers]]
name = "github"
command = "true"
env = { GITHUB_TOKEN = "resume-secret" }
"#,
        );
        env.start(&["--resume", "old-1", "--wait", "--prompt", "reply hi"]);
        let sent = &env.calls_of(method)[0]["params"]["mcpServers"][0]["env"][0];
        assert_eq!(sent["value"], "resume-secret", "{name}");
        for args in [
            &["session", "status", "old-1", "--json"][..],
            &["process", "list", "--json"],
            &["list", "--json"],
        ] {
            assert!(!env.ok(args).contains("resume-secret"), "{name}: {args:?}");
        }
        let acp = env.ok(&["event", "log", "old-1", "--events", "all", "--json"]);
        assert!(!acp.contains("resume-secret"), "{name}: {acp}");
        env.stop();
        assert!(wait_for(Duration::from_secs(15), || env.hosts().is_empty()));
        let all = everything_in(&env.dir.join("home"));
        assert!(!all.contains("resume-secret") && all.contains("<redacted>"), "{name}");
    }
}

// ---- bridges (ADR 35) --------------------------------------------------

/// A started bridge gets the events, but not the raw ACP unless it asks for
/// it by name; asked for, the MCP servers' secrets in it are redacted.
#[test]
fn adr_0035_a_bridge_gets_no_raw_acp_unless_it_asks() {
    let env = Env::new("s-bridge");
    let (events, acp) = (env.dir.join("events"), env.dir.join("acp"));
    env.write_config(&format!(
        r#"[[profiles.default.bridges]]
command = ["sh", "-c", {:?}]

[[profiles.default.bridges]]
command = ["sh", "-c", {:?}]
events = ["acp", "turn_ended"]

[[profiles.default.headless.mcp_servers]]
name = "github"
command = "true"
env = {{ GITHUB_TOKEN = "bridge-secret" }}
"#,
        format!("exec cat > '{}'", events.display()),
        format!("exec cat > '{}'", acp.display()),
    ));
    env.start(&["--wait", "--prompt", "reply hi"]);
    let ended = |path: &Path| {
        let read = || fs::read_to_string(path).unwrap_or_default().contains("turn_ended");
        assert!(wait_for(Duration::from_secs(5), read), "{}: no turn_ended", path.display());
        fs::read_to_string(path).unwrap()
    };
    let (events, acp) = (ended(&events), ended(&acp));
    assert!(!events.contains(r#""event":"acp""#), "{events}");
    assert!(acp.contains(r#""method":"session/new""#) && acp.contains("<redacted>"), "{acp}");
    assert!(!acp.contains("bridge-secret") && !events.contains("bridge-secret"));
    env.stop();
}

// ---- brnr acp (P1) -----------------------------------------------------

/// `hex` as a JSON `\u` escape.
fn escaped(hex: &str) -> String {
    format!("\\{}{hex}", 'u')
}

/// Lines from the agent, through `brnr acp`, until the response to `id`.
fn lines_until(from_agent: &mut BufReader<ChildStdout>, id: u64) -> Vec<String> {
    let mut lines = Vec::new();
    loop {
        let mut line = String::new();
        assert!(from_agent.read_line(&mut line).unwrap() > 0, "no answer to {id}");
        let msg: Option<Value> = serde_json::from_str(&line).ok();
        lines.push(line);
        if msg.is_some_and(|m| m["id"] == id && m.get("method").is_none()) {
            return lines;
        }
    }
}

/// An editor's lines reach the agent byte for byte, whatever their spacing,
/// key order and escapes, MCP secrets in them included, and with lines that
/// aren't JSON among them, but for a request's id, which is the host's
/// (ADR 61); the agent's reach the editor the same way, its answers with the
/// editor's ids. Only what brnr records has the secrets redacted.
#[test]
fn adr_0002_acp_passes_bytes_unchanged() {
    let env = Env::new("s-bytes");
    let raw = env.dir.join("raw");
    let mut editor = env
        .brnr(&["acp", "--", AGENT])
        .env("RAW_LOG", &raw)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut to_agent = editor.stdin.take().unwrap();
    let mut from_agent = BufReader::new(editor.stdout.take().unwrap());
    let cwd = serde_json::to_string(&env.dir).unwrap();
    // Each request's MCP values, marked with its name.
    let server = |marker: &str| {
        format!(
            r#"[{{"type":"http","name":"api","url":"https://example.invalid","headers":[{{"name":"Authorization","value":"Bearer {marker}"}}]}}, {{"name":"gh","command":"true","args":[],"env":[{{"name":"TOKEN","value":"{marker}-env"}}]}}]"#
        )
    };
    let lines = [
        (Some(1), r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{},"_meta":{"note":"cafE9 \/ é"}}}"#.replace("E9", &escaped("00e9"))),
        (Some(2), format!("{{ \"id\" : 2,\t\"method\":\"session/new\", \"jsonrpc\":\"2.0\",\"params\":{{\"mcpServers\":{},\"cwd\":{cwd}}}}}\r", server("new-secret"))),
        (None, r#"{"jsonrpc":"2.0","method":"_editor/ping","params":{"n":[1,2.50,1e3]}}"#.to_owned()),
        (None, "not json, from the editor".to_owned()),
        (Some(3), r#"{"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{"sessionId":"sess-1","prompt":[{"type":"text","text":"verbatim"}],"_meta":{"n":2.50}}}"#.to_owned()),
        (Some(4), format!(r#"{{"jsonrpc":"2.0","id":4,"method":"session/load","params":{{"sessionId":"old-1","cwd":{cwd},"mcpServers":{}}}}}"#, server("load-secret"))),
        (Some(5), format!(r#"{{"jsonrpc":"2.0","id":5,"method":"session/resume","params":{{"sessionId":"old-2","cwd":{cwd},"mcpServers":{}}}}}"#, server("resume-secret"))),
        (Some(6), format!(r#"{{"jsonrpc":"2.0","id":6,"method":"session/fork","params":{{"sessionId":"sess-1","cwd":{cwd},"mcpServers":{}}}}}"#, server("fork-secret"))),
    ];
    let mut from = Vec::new();
    for (id, line) in &lines {
        writeln!(to_agent, "{line}").unwrap();
        if let Some(id) = id {
            from.extend(lines_until(&mut from_agent, *id));
        }
    }
    // Each request with the host's id in place of the editor's, in the order
    // sent, and nothing else changed.
    let sent: String = (lines.iter())
        .map(|(id, l)| {
            let Some(id) = id else { return format!("{l}\n") };
            let spans = brnr::json::members(l.as_bytes(), "id");
            let ours = format!("\"brnr-{id}\"");
            let line = brnr::json::replace(l.as_bytes(), &spans, ours.as_bytes());
            format!("{}\n", String::from_utf8(line).unwrap())
        })
        .collect();
    assert!(
        wait_for(Duration::from_secs(5), || fs::read(&raw).is_ok_and(|r| r.len() >= sent.len()))
    );
    assert_eq!(fs::read_to_string(&raw).unwrap(), sent, "the agent got other bytes");
    // VERBATIM in fake_agent.py.
    let verbatim = r#"{ "params" : {"update":{"content":{"text":"cafE9 \/ é SMILE","type":"text"},  "sessionUpdate":"agent_message_chunk"},"sessionId":"sess-1"},"method":"session/update" ,"jsonrpc":"2.0"}"#
        .replace("E9", &escaped("00e9")).replace("SMILE", &format!("{}{}", escaped("d83d"), escaped("de00")));
    assert!(from.contains(&format!("{verbatim}\n")), "{from:#?}");
    assert!(from.contains(&"not json, from the agent\n".to_owned()), "{from:#?}");

    let host = env.host_pid();
    drop(to_agent);
    assert!(wait_exit(&mut editor, Duration::from_secs(15)), "acp didn't exit");
    assert!(wait_for(Duration::from_secs(15), || !alive(host)));
    let all = everything_in(&env.dir.join("home"));
    for (method, marker) in [
        ("session/new", "new-secret"),
        ("session/load", "load-secret"),
        ("session/resume", "resume-secret"),
        ("session/fork", "fork-secret"),
    ] {
        assert!(!all.contains(marker), "the editor's {method} recorded unredacted");
    }
    assert!(all.contains("not json, from the editor"), "the raw ACP is kept");
}

/// The agent's stderr comes out of `brnr acp`'s stderr as it was written,
/// and `brnr acp` exits as the agent did.
#[test]
fn adr_0002_acp_passes_stderr_and_the_exit_status() {
    let text = "agent: \x1b[1mbold\x1b[0m caf\u{e9}";
    let env = Env::new("s-stderr").agent("STDERR", text).agent("EXIT", "3");
    let out = env.run_with_stdin(&["acp", "--", AGENT], b"");
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    assert_eq!(stderr(&out), format!("{text}\n"));
    assert!(out.stdout.is_empty());
}

/// How long a stream gets to bring what a test waits for: far more than it
/// takes, so that only a stream that waits for something else fails.
const WITHIN: Duration = Duration::from_secs(10);

/// An editor running `brnr acp -- <agent>`, its stdin held open, its stdout
/// and stderr read on threads of their own: what each brings as it comes,
/// `None` at EOF. The agent is a Python script that waits for `gate(n)`,
/// a file in the test's directory, at most 30 s (then exits 99).
struct Editor {
    acp: Child,
    _stdin: ChildStdin,
    stdout: Receiver<Option<Vec<u8>>>,
    stderr: Receiver<Option<Vec<u8>>>,
}

impl Editor {
    fn new(env: &Env, agent: &str) -> Editor {
        let path = env.dir.join("agent.py");
        let gate = "def gate(n):\n    \
                    for _ in range(600):\n        \
                    if os.path.exists(f'gate{n}'): return\n        \
                    time.sleep(0.05)\n    \
                    sys.exit(99)\n";
        script(&path, &format!("#!/usr/bin/env python3\nimport os, sys, time\n{gate}{agent}"));
        let mut acp = (env.brnr(&["acp", "--", path.to_str().unwrap()]))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        Editor {
            _stdin: acp.stdin.take().unwrap(),
            stdout: reader(acp.stdout.take().unwrap()),
            stderr: reader(acp.stderr.take().unwrap()),
            acp,
        }
    }

    fn running(&mut self) -> bool {
        self.acp.try_wait().unwrap().is_none()
    }

    /// `brnr acp`'s exit code, within WITHIN.
    fn code(&mut self) -> Option<i32> {
        assert!(wait_exit(&mut self.acp, WITHIN), "acp didn't exit");
        self.acp.wait().unwrap().code()
    }
}

/// What `from` brings, as it comes; `None` at EOF.
fn reader(mut from: impl std::io::Read + Send + 'static) -> Receiver<Option<Vec<u8>>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = [0; 4096];
        while let Ok(n @ 1..) = from.read(&mut buf) {
            let _ = tx.send(Some(buf[..n].to_vec()));
        }
        let _ = tx.send(None);
    });
    rx
}

/// The stream brings `want` next, and no more for now.
fn brings(stream: &Receiver<Option<Vec<u8>>>, want: &[u8]) {
    let mut got = Vec::new();
    while got.len() < want.len() {
        match stream.recv_timeout(WITHIN) {
            Ok(Some(bytes)) => got.extend(bytes),
            Ok(None) => panic!("EOF after {:?}", String::from_utf8_lossy(&got)),
            Err(_) => panic!("only {:?} within {WITHIN:?}", String::from_utf8_lossy(&got)),
        }
    }
    assert_eq!(String::from_utf8_lossy(&got), String::from_utf8_lossy(want));
}

/// The stream ends next.
fn ends(stream: &Receiver<Option<Vec<u8>>>) {
    match stream.recv_timeout(WITHIN) {
        Ok(None) => {}
        Ok(Some(bytes)) => panic!("{:?}, not EOF", String::from_utf8_lossy(&bytes)),
        Err(_) => panic!("no EOF within {WITHIN:?}"),
    }
}

/// An agent that closes its stdout and runs on: the editor sees its stdout
/// end then, not when the agent exits, and the agent's stderr after it.
#[test]
fn adr_0062_the_editors_stdout_ends_with_the_agents() {
    let env = Env::new("s-out-eof");
    let agent = "os.close(1)\ngate(1)\nos.write(2, b'after stdout')\nsys.exit(5)\n";
    let mut editor = Editor::new(&env, agent);
    ends(&editor.stdout);
    assert!(editor.running(), "stdout ended only as acp exited");
    fs::write(env.dir.join("gate1"), "").unwrap();
    brings(&editor.stderr, b"after stdout");
    ends(&editor.stderr);
    assert_eq!(editor.code(), Some(5));
}

/// What the agent writes on stderr comes out of `brnr acp`'s as soon as it
/// is written, a line or not, and its last bytes once, whole, before the
/// exit status.
#[test]
fn adr_0062_stderr_comes_as_it_is_written() {
    let env = Env::new("s-err-part");
    let agent = "os.write(2, b'login required: ')\ngate(1)\nos.write(2, b'ok')\nsys.exit(3)\n";
    let mut editor = Editor::new(&env, agent);
    brings(&editor.stderr, b"login required: ");
    assert!(editor.running());
    fs::write(env.dir.join("gate1"), "").unwrap();
    brings(&editor.stderr, b"ok");
    ends(&editor.stderr);
    ends(&editor.stdout);
    assert_eq!(editor.code(), Some(3));
}

/// The agent's stdout and stderr live on their own: stdout carries on after
/// stderr has closed, and ends while the agent runs on. The editor's stderr,
/// `brnr acp`'s own as well, ends as it exits.
#[test]
fn adr_0062_stdout_and_stderr_end_on_their_own() {
    let env = Env::new("s-lifetimes");
    let agent = "os.write(2, b'a')\nos.close(2)\nos.write(1, b'line\\n')\ngate(1)\n\
                 os.close(1)\ngate(2)\nsys.exit(4)\n";
    let mut editor = Editor::new(&env, agent);
    brings(&editor.stderr, b"a");
    brings(&editor.stdout, b"line\n");
    fs::write(env.dir.join("gate1"), "").unwrap();
    ends(&editor.stdout);
    assert!(editor.running());
    assert_eq!(editor.stderr.try_recv(), Err(TryRecvError::Empty));
    fs::write(env.dir.join("gate2"), "").unwrap();
    ends(&editor.stderr);
    assert_eq!(editor.code(), Some(4));
}
