//! `brnr doctor [--fix] [--json]`: checks what brnr depends on and says
//! what's wrong.
//!
//! - the runtime directory: private to the user, not a symlink, short enough
//!   for a socket path, and free of metadata, sockets and session locks left
//!   by processes that are gone. Gone is what a process's socket says (see
//!   `gone` in ctl.rs): one nobody listens on was left by a process that is
//!   gone, even when its pid is another process's now;
//! - transcripts under `BRNR_HOME`: readable only by the user;
//! - the config file: it parses, every key is in its part of a profile
//!   (ADR 33), and every profile's settings, cwd, agent and bridges are
//!   valid;
//! - the adapters: where brnr's (`brnr-claude-adapter`, `brnr-codex-adapter`)
//!   and the npm packages' (`claude-agent-acp`, `codex-acp`) are found;
//! - running processes: each answers; of one that doesn't, the sessions it
//!   holds the locks of (ADR 3);
//! - host logs: those of processes that died without recording it (no
//!   `exited`, and not running), the cases ADR 11 can't record, with when
//!   each last wrote and the sessions it had open. Info, not a warning:
//!   they are a record, and nothing is to be repaired.
//!
//! `--fix` tightens permissions on directories and files the user owns and
//! removes stale metadata, sockets and locks. It never touches anything it
//! would refuse to use, nor any log. The exit status is non-zero if a check
//! failed. `--json` prints the checks as a list of `{level, check,
//! message}`.

use std::collections::HashMap;
use std::env;
use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use brnr::host::check_bridge;
use brnr::{config, lock, log, paths, spawn};

use super::{Host, Probe, USAGE, gone, last_record, probe, read_meta, when};

/// The longest pid a socket name may need: Linux's pid_max is at most 2^22.
const PID_DIGITS: usize = 7;

#[derive(Clone, Copy)]
enum Level {
    Ok,
    Info,
    Warn,
    Fail,
}

struct Report {
    fix: bool,
    json: bool,
    checks: Vec<Value>,
    failed: usize,
    warned: usize,
}

impl Report {
    fn line(&mut self, level: Level, what: &str, msg: impl AsRef<str>) {
        let tag = match level {
            Level::Ok => "ok",
            Level::Info => "--",
            Level::Warn => {
                self.warned += 1;
                "warn"
            }
            Level::Fail => {
                self.failed += 1;
                "FAIL"
            }
        };
        if self.json {
            let level = match level {
                Level::Ok => "ok",
                Level::Info => "info",
                Level::Warn => "warn",
                Level::Fail => "fail",
            };
            self.checks.push(json!({ "level": level, "check": what, "message": msg.as_ref() }));
        } else {
            outln!("{tag:<5} {what}: {}", msg.as_ref());
        }
    }
}

pub fn main(args: &[String]) -> Result<(), String> {
    let (mut fix, mut json_out) = (false, false);
    for arg in args {
        match arg.as_str() {
            "--fix" => fix = true,
            "--json" => json_out = true,
            _ => return Err(USAGE.to_owned()),
        }
    }
    let mut r = Report { fix, json: json_out, checks: Vec::new(), failed: 0, warned: 0 };
    let hosts = runtime_dir(&mut r);
    transcripts(&mut r);
    config_file(&mut r);
    adapters(&mut r);
    running(&mut r, hosts.as_deref().unwrap_or_default());
    host_logs(&mut r, hosts.as_deref());
    if r.json {
        outln!("{}", serde_json::to_string_pretty(&r.checks).unwrap());
        return match r.failed {
            0 => Ok(()),
            n => Err(format!("{n} check{} failed", plural(n))),
        };
    }
    match (r.failed, r.warned) {
        (0, 0) => Ok(()),
        (0, n) => {
            outln!("\n{n} warning{}", plural(n));
            Ok(())
        }
        (n, _) => Err(format!("{n} check{} failed", plural(n))),
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

// ---- runtime directory ---------------------------------------------------

/// A running process, and why it didn't answer if it didn't.
type Running = (Host, Option<String>);

/// Checks the directory and returns the processes running, or `None` if it
/// can't be trusted to say.
fn runtime_dir(r: &mut Report) -> Option<Vec<Running>> {
    let what = "runtime dir";
    let dir = paths::runtime_dir();
    if dir.is_relative() {
        let msg =
            format!("BRNR_DIR is relative ({}): processes resolve it in their own cwd", dir.display());
        r.line(Level::Fail, what, msg);
    }
    let room = sun_path_len() - 1;
    let longest = dir.join(format!("{}.sock", "9".repeat(PID_DIGITS))).as_os_str().len();
    if longest > room {
        let msg = format!(
            "{}: socket paths would be {longest} bytes, the limit is {room}",
            dir.display()
        );
        r.line(Level::Fail, what, msg);
    }
    match fs::symlink_metadata(&dir) {
        Err(_) => {
            r.line(Level::Ok, what, format!("{} (created on first use)", dir.display()));
            return Some(Vec::new());
        }
        Ok(meta) => match private_problem(&meta, true) {
            None => r.line(Level::Ok, what, format!("{}: private", dir.display())),
            Some(problem) => {
                if r.fix && fixable(&meta, true) && chmod(&dir, 0o700) {
                    r.line(Level::Ok, what, format!("{}: {problem}; fixed", dir.display()));
                } else {
                    let hint = if fixable(&meta, true) { " (brnr doctor --fix)" } else { "" };
                    let msg = format!("{}: {problem}; brnr won't use it{hint}", dir.display());
                    r.line(Level::Fail, what, msg);
                    return None;
                }
            }
        },
    }

    // Metadata of processes that are gone, and sockets without metadata. A
    // process binds its socket and listens, then writes its metadata
    // (`<pid>.json.tmp`, renamed): while that socket accepts, those are a
    // process starting. Whether a process is gone is what its socket says,
    // not its pid alone, which may be another process's now (`gone`).
    let mut hosts = Vec::new();
    let mut stale: Vec<PathBuf> = Vec::new();
    let entries: Vec<PathBuf> =
        fs::read_dir(&dir).into_iter().flatten().flatten().map(|e| e.path()).collect();
    for path in &entries {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        if name.ends_with(".json.tmp") {
            if !starting(path) {
                stale.push(path.clone());
            }
            continue;
        }
        match path.extension().and_then(OsStr::to_str) {
            Some("json") => {
                let Some(meta) = read_meta(path) else {
                    stale.extend([path.clone(), path.with_extension("sock")]);
                    continue;
                };
                let mut host = Host { meta, status: None };
                match probe(path, &host) {
                    Probe::Answered(status) => {
                        host.status = Some(status);
                        hosts.push((host, None));
                    }
                    Probe::Silent(why) => hosts.push((host, Some(why))),
                    // Its socket goes with it.
                    Probe::Gone => stale.extend([path.clone(), path.with_extension("sock")]),
                }
            }
            Some("sock") if !path.with_extension("json").exists() && !starting(path) => {
                stale.push(path.clone());
            }
            _ => {}
        }
    }
    stale.retain(|p| p.exists());
    stale.sort();
    stale.dedup();
    // Session locks nobody holds: the kernel let go when their process died,
    // and the file stayed.
    let locks: Vec<PathBuf> =
        lock::all().into_iter().filter(|e| e.pid.is_none()).map(|e| e.path).collect();
    if stale.is_empty() && locks.is_empty() {
        return Some(hosts);
    }
    let mut names: Vec<String> =
        stale.iter().map(|p| p.file_name().unwrap_or_default().to_string_lossy().into()).collect();
    for p in &locks {
        names.push(format!("sessions/{}", p.file_name().unwrap_or_default().to_string_lossy()));
    }
    if r.fix {
        stale.iter().for_each(|p| drop(fs::remove_file(p)));
        for p in &locks {
            lock::remove(p);
        }
        r.line(
            Level::Ok,
            what,
            format!("removed what processes that are gone left behind: {}", names.join(", ")),
        );
    } else {
        let msg = format!("left by processes that are gone: {} (brnr doctor --fix)", names.join(", "));
        r.line(Level::Warn, what, msg);
    }
    Some(hosts)
}

/// Whether what a process left without its metadata (`<pid>.sock`,
/// `<pid>.json.tmp`) is a process's that is starting: its socket accepts.
fn starting(path: &Path) -> bool {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let Some(pid) = name.split('.').next().and_then(|pid| pid.parse::<i64>().ok()) else {
        return false;
    };
    let socket = path.with_file_name(format!("{pid}.sock"));
    match UnixStream::connect(&socket) {
        Ok(_) => true,
        Err(e) => !gone(pid, &socket, path, &e),
    }
}

fn sun_path_len() -> usize {
    let addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    addr.sun_path.len()
}

// ---- transcripts ---------------------------------------------------------

fn transcripts(r: &mut Report) {
    let what = "transcripts";
    let root = paths::state_dir();
    let Ok(meta) = fs::symlink_metadata(&root) else {
        r.line(Level::Ok, what, format!("{} (created on first use)", root.display()));
        return;
    };
    // Every directory and file brnr writes: the root, hosts/ and projects/
    // with their files, and each project folder's files.
    let mut paths = vec![(root.clone(), meta)];
    for sub in ["hosts", "projects"] {
        let dir = root.join(sub);
        let Ok(meta) = fs::symlink_metadata(&dir) else { continue };
        paths.push((dir.clone(), meta));
        for entry in fs::read_dir(&dir).into_iter().flatten().flatten() {
            let Ok(meta) = entry.path().symlink_metadata() else { continue };
            let is_dir = meta.is_dir();
            paths.push((entry.path(), meta));
            if sub == "projects" && is_dir {
                for file in fs::read_dir(entry.path()).into_iter().flatten().flatten() {
                    if let Ok(meta) = file.path().symlink_metadata() {
                        paths.push((file.path(), meta));
                    }
                }
            }
        }
    }
    let open: Vec<&(PathBuf, fs::Metadata)> =
        paths.iter().filter(|(_, m)| private_problem(m, m.is_dir()).is_some()).collect();
    if open.is_empty() {
        let msg = format!("{}: private ({} paths)", root.display(), paths.len());
        return r.line(Level::Ok, what, msg);
    }
    let fixable: Vec<_> = open.iter().filter(|(_, m)| fixable(m, m.is_dir())).collect();
    let example = open[0].0.display();
    if r.fix && !fixable.is_empty() {
        let fixed = fixable
            .iter()
            .filter(|(p, m)| chmod(p, if m.is_dir() { 0o700 } else { 0o600 }))
            .count();
        r.line(Level::Ok, what, format!("made {fixed} paths private under {}", root.display()));
        if fixed < open.len() {
            let msg = format!("{} paths are not yours to fix, e.g. {example}", open.len() - fixed);
            r.line(Level::Warn, what, msg);
        }
    } else {
        let hint = if fixable.is_empty() { "" } else { " (brnr doctor --fix)" };
        let msg = format!(
            "{} of {} paths can be read by others, e.g. {example}{hint}",
            open.len(),
            paths.len()
        );
        r.line(Level::Warn, what, msg);
    }
}

// ---- permissions ---------------------------------------------------------

/// What's wrong with a path that should be private, if anything.
fn private_problem(meta: &fs::Metadata, dir: bool) -> Option<String> {
    let mode = meta.mode() & 0o777;
    if meta.file_type().is_symlink() {
        Some("is a symlink".into())
    } else if dir && !meta.is_dir() {
        Some("is not a directory".into())
    } else if meta.uid() != unsafe { libc::getuid() } {
        Some(format!("is owned by uid {}", meta.uid()))
    } else if mode & 0o077 != 0 {
        Some(format!("has mode {mode:o}"))
    } else {
        None
    }
}

/// Only what is ours and what it claims to be gets chmod'ed.
fn fixable(meta: &fs::Metadata, dir: bool) -> bool {
    !meta.file_type().is_symlink()
        && meta.is_dir() == dir
        && meta.uid() == unsafe { libc::getuid() }
}

fn chmod(path: &Path, mode: u32) -> bool {
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).is_ok()
}

// ---- config --------------------------------------------------------------

fn config_file(r: &mut Report) {
    let what = "config";
    let path = paths::config_file();
    let profiles = match config::load_all() {
        Ok(None) => {
            return r.line(Level::Ok, what, format!("{} (none; defaults apply)", path.display()));
        }
        Ok(Some(profiles)) => profiles,
        Err(problems) => {
            for problem in problems {
                r.line(Level::Fail, what, format!("{}: {problem}", path.display()));
            }
            return;
        }
    };
    r.line(Level::Ok, what, format!("{}: {} profiles", path.display(), profiles.len()));
    for (name, profile) in &profiles {
        let what = format!("profile {name}");
        let mut problems = Vec::new();
        for server in &profile.headless.mcp_servers {
            match server.to_acp() {
                Err(e) => problems.push(e),
                Ok(acp) => {
                    if let Some(command) = acp["command"].as_str()
                        && find_program(command).is_none()
                    {
                        problems.push(format!("MCP server {}: {command} not found", server.name));
                    }
                }
            }
        }
        if let Some(cwd) = &profile.headless.cwd
            && !paths::expand(cwd).is_dir()
        {
            problems.push(format!("cwd {cwd} is not a directory"));
        }
        match profile.agent.as_deref() {
            Some([]) => problems.push("agent is empty".into()),
            Some([program, ..]) if find_program(program).is_none() => {
                problems.push(format!("agent {program} not found"));
            }
            _ => {}
        }
        for bridge in &profile.bridges {
            if let Err(e) = check_bridge(bridge) {
                problems.push(e);
            } else if find_program(&bridge.command[0]).is_none() {
                problems.push(format!("bridge {} not found", bridge.command[0]));
            }
        }
        if problems.is_empty() {
            r.line(Level::Ok, &what, describe_profile(profile));
        }
        for problem in problems {
            r.line(Level::Fail, &what, problem);
        }
    }
}

fn describe_profile(profile: &config::Profile) -> String {
    let agent = profile.agent.as_ref().map_or("given on the command line".into(), |a| a.join(" "));
    let bridges = profile.bridges.len();
    let mut out = format!("agent {agent}, {bridges} bridge{}", plural(bridges));
    let servers = profile.headless.mcp_servers.len();
    if servers > 0 {
        out.push_str(&format!(", {servers} MCP server{}", plural(servers)));
    }
    if profile.strict {
        out.push_str(", strict");
    }
    let experimental: Vec<&str> = profile.editor.experimental.iter().map(|e| e.name()).collect();
    let features: Vec<&str> = profile.editor.features.iter().map(|f| f.name()).collect();
    for (what, names) in [("experimental", experimental), ("features", features)] {
        if !names.is_empty() {
            out.push_str(&format!(", {what} {}", names.join(" ")));
        }
    }
    out
}

// ---- adapters ------------------------------------------------------------

/// brnr's adapters and the npm packages', where they are and the version of
/// the npm package each is (or was built from).
fn adapters(r: &mut Report) {
    let adapters = [
        ("brnr-claude-adapter", "@agentclientprotocol/claude-agent-acp"),
        ("brnr-codex-adapter", "@agentclientprotocol/codex-acp"),
        ("claude-agent-acp", "@agentclientprotocol/claude-agent-acp"),
        ("codex-acp", "@agentclientprotocol/codex-acp"),
    ];
    for (name, package) in adapters {
        let Some(path) = find_program(name) else {
            r.line(Level::Info, name, "not found next to brnr or on PATH");
            continue;
        };
        let version = if name.starts_with("brnr-") {
            built_version(&path)
                .unwrap_or_else(|| "version unknown: built before brnr 0.3.0".to_owned())
        } else {
            npm_version(&path, package).unwrap_or_else(|| "version unknown".to_owned())
        };
        r.line(Level::Ok, name, format!("{} ({version})", path.display()));
    }
}

/// What a brnr adapter says it was built from (`--version`): `<package>
/// <version>`. Older builds don't know `--version` and would wait for an
/// editor, so stdin is closed and they get a few seconds.
fn built_version(path: &Path) -> Option<String> {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    let mut child = Command::new(path)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let until = Instant::now() + Duration::from_secs(5);
    while child.try_wait().ok()?.is_none() {
        if Instant::now() > until {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let out = child.wait_with_output().ok()?;
    // brnr-claude-adapter 0.85.1 (@agentclientprotocol/claude-agent-acp)
    let line = String::from_utf8(out.stdout).ok()?;
    let mut words = line.split_whitespace();
    let (_, version, package) = (words.next()?, words.next()?, words.next()?);
    Some(format!("{} {version}", package.trim_matches(['(', ')'])))
}

/// The version of an npm-installed command's package: the `package.json`
/// named `package` above the file its bin link points to.
fn npm_version(path: &Path, package: &str) -> Option<String> {
    let target = fs::canonicalize(path).ok()?;
    let found = target.ancestors().skip(1).find_map(|dir| {
        let manifest: Value =
            serde_json::from_slice(&fs::read(dir.join("package.json")).ok()?).ok()?;
        (manifest["name"] == package).then(|| manifest["version"].as_str().map(str::to_owned))?
    })?;
    Some(format!("{package} {found}"))
}

/// Where the host would find `program`: a path, next to brnr, or on PATH.
fn find_program(program: &str) -> Option<PathBuf> {
    if program.contains('/') {
        let path = paths::expand(program);
        return executable(&path).then_some(path);
    }
    if let Some(path) = spawn::bundled(OsStr::new(program)) {
        return Some(path);
    }
    let dirs = env::var_os("PATH")?;
    env::split_paths(&dirs).map(|d| d.join(program)).find(|p| executable(p))
}

fn executable(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

// ---- running processes ---------------------------------------------------

fn running(r: &mut Report, hosts: &[Running]) {
    let what = "processes";
    if hosts.is_empty() {
        return r.line(Level::Ok, what, "none running");
    }
    let locks = lock::all();
    let mut answering = 0;
    for (host, silent) in hosts {
        let Some(why) = silent else {
            answering += 1;
            continue;
        };
        let held = host.held(&locks);
        let serving = if held.is_empty() {
            String::new()
        } else {
            format!("; it serves {}", held.join(", "))
        };
        r.line(Level::Warn, what, format!("{} is running but {why}{serving}", host.id()));
    }
    if answering > 0 {
        r.line(Level::Ok, what, format!("{answering} running and answering"));
    }
}

// ---- host logs -----------------------------------------------------------

/// How many of the processes that died the line names, the latest first.
const DIED_SHOWN: usize = 3;

/// How much of a host log's end is read for its `exited`.
const LOG_TAIL: u64 = 64 * 1024;

/// Host logs (`~/.brnr/hosts/<run id>.jsonl`) of processes that died
/// without recording it, the cases ADR 11 couldn't record: the log has no
/// `exited`, and no process running has its run id. For each, when its last
/// record was written and the sessions it had open. Only the ends of files
/// are read: of each host log, and of each session's events file for the
/// run it was last written by. `hosts` is `None` when which processes are
/// running can't be known.
fn host_logs(r: &mut Report, hosts: Option<&[Running]>) {
    let what = "host logs";
    let Some(hosts) = hosts else {
        return r.line(Level::Info, what, "not checked: which processes are running is unknown");
    };
    let running: Vec<&str> =
        hosts.iter().filter_map(|(host, _)| host.meta["host_id"].as_str()).collect();
    let dir = paths::state_dir().join("hosts");
    let mut logs = 0;
    let mut died: Vec<(SystemTime, String)> = Vec::new();
    for entry in fs::read_dir(&dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "jsonl") {
            continue;
        }
        let Some(run) = path.file_stem().and_then(OsStr::to_str) else { continue };
        logs += 1;
        if running.contains(&run) || recorded_exit(&path) != Some(false) {
            continue;
        }
        // Appended to only: when it was last modified is its last record.
        let written = entry.metadata().and_then(|m| m.modified()).unwrap_or(UNIX_EPOCH);
        died.push((written, run.to_owned()));
    }
    if died.is_empty() {
        let msg = format!("no process died without recording it ({logs} log{})", plural(logs));
        return r.line(Level::Ok, what, msg);
    }
    died.sort_by(|a, b| b.cmp(a));
    let open = open_sessions();
    let latest: Vec<String> = died
        .iter()
        .take(DIED_SHOWN)
        .map(|(written, run)| {
            let sessions = match open.get(run) {
                Some(ids) => format!("session{} {}", plural(ids.len()), ids.join(", ")),
                None => "no sessions".to_owned(),
            };
            format!("{run} (last record {}; {sessions})", when(&log::rfc3339(*written)))
        })
        .collect();
    let n = died.len();
    let which = if n > DIED_SHOWN { "; the latest" } else { "" };
    let msg = format!(
        "{n} process{} died without recording it{which}: {}",
        if n == 1 { "" } else { "es" },
        latest.join(", ")
    );
    // Nothing to repair: what is left is a record (ADR 11).
    r.line(Level::Info, what, msg);
}

/// Whether a host log records its process's end, `None` if it can't be
/// read. `exited` is the last thing a process writes but for what follows
/// as it lets go (a bridge's exit, its own stderr), so it is in the end of
/// the file.
fn recorded_exit(path: &Path) -> Option<bool> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = fs::File::open(path).ok()?;
    let start = file.metadata().ok()?.len().saturating_sub(LOG_TAIL);
    let mut tail = Vec::new();
    file.seek(SeekFrom::Start(start)).ok()?;
    file.read_to_end(&mut tail).ok()?;
    let mut lines = tail.split(|&b| b == b'\n');
    if start > 0 {
        lines.next(); // Cut off.
    }
    let exited = lines.filter(|line| line.windows(8).any(|w| w == b"\"exited\"")).any(|line| {
        serde_json::from_slice::<Value>(line)
            .is_ok_and(|record| record["event"]["event"] == "exited")
    });
    Some(exited)
}

/// The sessions each run had open when it last wrote to them, by run id:
/// from the last record of every session's events file, but for those it
/// closed.
fn open_sessions() -> HashMap<String, Vec<String>> {
    let mut open: HashMap<String, Vec<String>> = HashMap::new();
    let projects = paths::state_dir().join("projects");
    let files = fs::read_dir(&projects)
        .into_iter()
        .flatten()
        .flatten()
        .flat_map(|dir| fs::read_dir(dir.path()).into_iter().flatten().flatten())
        .map(|e| e.path())
        .filter(|p| paths::is_events_log(p));
    for file in files {
        let Some(last) = last_record(&file) else { continue };
        let (Some(run), Some(session)) = (last["host_id"].as_str(), last["session_id"].as_str())
        else {
            continue;
        };
        if last["event"]["event"] != "session_closed" {
            open.entry(run.to_owned()).or_default().push(session.to_owned());
        }
    }
    for ids in open.values_mut() {
        ids.sort();
    }
    open
}
