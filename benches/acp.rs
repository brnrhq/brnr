//! What `brnr acp` adds (ADR 1, N3): the latency per message and the
//! throughput through it, each against the same agent on a direct pipe; the
//! memory of an idle `brnr acp` and its process; and the threads of the
//! process and each peer's. Each measure has a budget in benches/budgets.toml,
//! and the run fails when one is passed.
//!
//! ```text
//! cargo bench --bench acp [-- [--quick] [--markdown <file>]]
//! ```
//!
//! The agent is this program again (`--agent`), a fake that answers at once,
//! so that the numbers are brnr's and not a slow agent's. A latency is a
//! prompt's round trip, sent to its answer; the throughput is a turn that
//! streams `STREAM` chunks before its answer, a message every `MESSAGE`
//! chunks (one message of 20 MB would measure ADR 10.3's gap, a message
//! assembled whole, not the stream). Shared CI runners are noisy, so
//! brnr's numbers are budgeted as ratios to the direct pipe's, measured
//! interleaved on the same machine at the same time, and the budgets are
//! wide. The ratios are against an agent that costs nothing: with a real
//! agent the difference is the same, and the ratio smaller.
//! `--markdown` writes the results as the pull request's comment has them
//! (.github/workflows/bench.yml).

use std::fmt::Write as _;
use std::fs;
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, ExitCode, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{Value, json};

const BRNR: &str = env!("CARGO_BIN_EXE_brnr");
const BUDGETS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/benches/budgets.toml");

/// The streamed turn: this many chunks of `CHUNK` bytes of text, `MESSAGE`
/// chunks to a message.
const STREAM: usize = 20_000;
const CHUNK: usize = 1024;
const MESSAGE: usize = 16;

/// Upper bounds, as benches/budgets.toml has them.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Budgets {
    latency_ratio: f64,
    throughput_ratio: f64,
    proxy_rss_kib: u64,
    host_rss_kib: u64,
    host_streamed_rss_kib: u64,
    proxy_threads: u64,
    host_threads: u64,
    connection_threads: u64,
    bridge_threads: u64,
}

/// One line of the results: a number, and its budget if it has one.
struct Measure {
    name: &'static str,
    value: String,
    budget: Option<(String, bool)>,
}

fn main() -> ExitCode {
    // cargo bench adds --bench. Without it, run by `cargo test
    // --all-targets` (coverage.yml), the bench only shows that it works: a
    // quick run, its budgets not checked, as an instrumented build is slow.
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "--agent") {
        return agent();
    }
    let benching = args.iter().any(|a| a == "--bench");
    args.retain(|a| a != "--bench");
    let (mut quick, mut markdown) = (!benching, None);
    let mut rest = args.into_iter();
    while let Some(arg) = rest.next() {
        let file = if arg == "--markdown" { rest.next() } else { None };
        match (arg.as_str(), file) {
            ("--quick", _) => quick = true,
            ("--markdown", Some(file)) => markdown = Some(file),
            _ => {
                eprintln!("usage: acp [--quick] [--markdown <file>]");
                return ExitCode::from(2);
            }
        }
    }
    let budgets: Budgets = toml::from_str(&fs::read_to_string(BUDGETS).unwrap()).unwrap();
    let scratch = Scratch::new();
    let measures = measure(&scratch, &budgets, quick);
    drop(scratch);

    let machine = machine();
    let over = measures.iter().filter(|m| m.budget.as_ref().is_some_and(|b| !b.1)).count();
    println!("brnr {} on {machine}", version());
    for m in &measures {
        let budget = m.budget.as_ref().map_or(String::new(), |(b, ok)| {
            format!("budget {b:<8} {}", if *ok { "ok" } else { "OVER" })
        });
        println!("  {:<38} {:>14}   {budget}", m.name, m.value);
    }
    if let Some(path) = markdown {
        fs::write(path, table(&measures, &machine)).unwrap();
    }
    if over > 0 && !benching {
        println!("{over} over budget, not checked: not run by cargo bench");
    } else if over > 0 {
        println!("{over} over budget");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

fn measure(scratch: &Scratch, budgets: &Budgets, quick: bool) -> Vec<Measure> {
    let (rounds, pings) = if quick { (2, 50) } else { (10, 200) };
    let mut direct = Editor::direct(scratch);
    let mut brnr = Editor::brnr(scratch, "plain", None);
    let (proxy, host) = (brnr.child.id() as i32, brnr.host);

    // At rest: initialized, a session open, nothing sent yet.
    let proxy_rss = settled(|| rss_kib(proxy));
    let host_rss = settled(|| rss_kib(host));
    let proxy_threads = settled(|| threads(proxy));
    let host_threads = settled(|| threads(host));

    // A connection to the control socket, answered.
    let mut conn = UnixStream::connect(&brnr.socket).unwrap();
    writeln!(conn, r#"{{"cmd":"bench","req_id":1}}"#).unwrap();
    BufReader::new(&conn).read_line(&mut String::new()).unwrap();
    let connection_threads = settled(|| threads(host)).saturating_sub(host_threads);
    drop(conn);

    // The same, with a bridge.
    let bridged = Editor::brnr(scratch, "bridged", Some("bridged"));
    let bridge_threads = settled(|| threads(bridged.host)).saturating_sub(host_threads);
    bridged.close();

    let mut times = [Vec::new(), Vec::new()];
    for editor in [&mut direct, &mut brnr] {
        (0..pings).for_each(|_| _ = editor.ping());
    }
    for _ in 0..rounds {
        for (i, editor) in [&mut direct, &mut brnr].into_iter().enumerate() {
            times[i].extend((0..pings).map(|_| editor.ping()));
        }
    }
    let [direct_ping, brnr_ping] = times.map(|mut t| median(&mut t));

    let mut streams = [Duration::MAX; 2];
    for _ in 0..if quick { 1 } else { 3 } {
        for (i, editor) in [&mut direct, &mut brnr].into_iter().enumerate() {
            streams[i] = streams[i].min(editor.stream());
        }
    }
    let host_streamed_rss = settled(|| rss_kib(host));
    direct.close();
    brnr.close();

    let latency_ratio = brnr_ping.as_secs_f64() / direct_ping.as_secs_f64();
    let throughput_ratio = streams[1].as_secs_f64() / streams[0].as_secs_f64();
    let mbps = |t: Duration| (STREAM * CHUNK) as f64 / t.as_secs_f64() / 1e6;
    let ratio = |v: f64, b: f64| Some((format!("{b:.0}×"), v <= b));
    let most = |v: u64, b: u64| Some((b.to_string(), v <= b));
    let kib = |v: u64, b: u64| Some((format!("{} MiB", b / 1024), v <= b));
    let m = |name, value: String, budget| Measure { name, value, budget };
    vec![
        m("latency, direct pipe (median)", micros(direct_ping), None),
        m("latency, brnr acp (median)", micros(brnr_ping), None),
        m("latency added", micros(brnr_ping.saturating_sub(direct_ping)), None),
        m("latency, brnr acp ÷ direct", format!("{latency_ratio:.1}×"), {
            ratio(latency_ratio, budgets.latency_ratio)
        }),
        m("throughput, direct pipe", format!("{:.0} MB/s", mbps(streams[0])), None),
        m("throughput, brnr acp", format!("{:.0} MB/s", mbps(streams[1])), None),
        m("stream time, brnr acp ÷ direct", format!("{throughput_ratio:.1}×"), {
            ratio(throughput_ratio, budgets.throughput_ratio)
        }),
        m("RSS, brnr acp at rest", mib(proxy_rss), kib(proxy_rss, budgets.proxy_rss_kib)),
        m("RSS, its process at rest", mib(host_rss), kib(host_rss, budgets.host_rss_kib)),
        m("RSS, its process after the streams", mib(host_streamed_rss), {
            kib(host_streamed_rss, budgets.host_streamed_rss_kib)
        }),
        m("threads, brnr acp", proxy_threads.to_string(), {
            most(proxy_threads, budgets.proxy_threads)
        }),
        m("threads, its process at rest", host_threads.to_string(), {
            most(host_threads, budgets.host_threads)
        }),
        m("threads per control connection", connection_threads.to_string(), {
            most(connection_threads, budgets.connection_threads)
        }),
        m("threads per bridge", bridge_threads.to_string(), {
            most(bridge_threads, budgets.bridge_threads)
        }),
    ]
}

/// The results as a Markdown table, with what they came from.
fn table(measures: &[Measure], machine: &str) -> String {
    let mut out = format!("brnr {} on {machine}\n\n", version());
    out.push_str("| Measure | Value | Budget | |\n|---|---:|---:|---|\n");
    for m in measures {
        let (budget, ok) = match &m.budget {
            Some((b, ok)) => (b.as_str(), if *ok { "ok" } else { "**over**" }),
            None => ("", ""),
        };
        writeln!(out, "| {} | {} | {budget} | {ok} |", m.name, m.value).unwrap();
    }
    out
}

/// An editor: the fake agent run directly, or through `brnr acp`.
struct Editor {
    child: Child,
    to: BufWriter<ChildStdin>,
    from: BufReader<ChildStdout>,
    next: u64,
    session: String,
    /// brnr's process (0 for the direct pipe) and its control socket.
    host: i32,
    socket: PathBuf,
}

impl Editor {
    fn direct(scratch: &Scratch) -> Editor {
        let mut cmd = Command::new(std::env::current_exe().unwrap());
        cmd.arg("--agent").current_dir(&scratch.0);
        Editor::start(cmd, None)
    }

    /// `brnr acp [--profile <profile>]`, its runtime directory `run`.
    fn brnr(scratch: &Scratch, run: &str, profile: Option<&str>) -> Editor {
        let run = scratch.0.join(run);
        let mut cmd = Command::new(BRNR);
        cmd.arg("acp");
        if let Some(profile) = profile {
            cmd.args(["--profile", profile]);
        }
        cmd.arg("--").arg(std::env::current_exe().unwrap()).arg("--agent");
        cmd.current_dir(&scratch.0)
            .env("BRNR_DIR", &run)
            .env("BRNR_HOME", scratch.0.join("home"))
            .env("BRNR_CONFIG", scratch.config());
        Editor::start(cmd, Some(&run))
    }

    /// Started, initialized and with a session open; through brnr, its
    /// process found in `run`.
    fn start(mut cmd: Command, run: Option<&Path>) -> Editor {
        let mut child = cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
        let to = BufWriter::new(child.stdin.take().unwrap());
        let from = BufReader::new(child.stdout.take().unwrap());
        let mut e = Editor {
            child,
            to,
            from,
            next: 0,
            session: String::new(),
            host: 0,
            socket: PathBuf::new(),
        };
        e.request("initialize", json!({ "protocolVersion": 1, "clientCapabilities": {} }));
        let cwd = std::env::current_dir().unwrap();
        let opened = e.request("session/new", json!({ "cwd": cwd, "mcpServers": [] }));
        e.session = opened["result"]["sessionId"].as_str().unwrap().to_owned();
        if let Some(run) = run {
            let mut meta = fs::read_dir(run).unwrap().flatten().map(|e| e.path());
            let meta = meta.find(|p| p.extension().is_some_and(|x| x == "json")).unwrap();
            let meta: Value = serde_json::from_slice(&fs::read(meta).unwrap()).unwrap();
            e.host = meta["host_pid"].as_i64().unwrap() as i32;
            e.socket = meta["socket"].as_str().unwrap().into();
        }
        e
    }

    /// Sends a request, and reads to its answer: what comes before it, and
    /// its bytes.
    fn send(&mut self, method: &str, params: Value) -> (Value, usize) {
        self.next += 1;
        let msg = json!({ "jsonrpc": "2.0", "id": self.next, "method": method, "params": params });
        writeln!(self.to, "{msg}").unwrap();
        self.to.flush().unwrap();
        let (mut line, mut read) = (String::new(), 0);
        loop {
            line.clear();
            let n = self.from.read_line(&mut line).unwrap();
            assert!(n > 0, "no answer to {method}");
            read += n;
            // Every line parsed, as an editor does.
            let msg: Value = serde_json::from_str(&line).unwrap();
            if msg["id"] == self.next && msg.get("method").is_none() {
                assert!(msg.get("error").is_none(), "{method}: {msg}");
                return (msg, read);
            }
        }
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        self.send(method, params).0
    }

    fn prompt(&mut self, text: &str) -> usize {
        let prompt = json!([{ "type": "text", "text": text }]);
        self.send("session/prompt", json!({ "sessionId": self.session, "prompt": prompt })).1
    }

    /// A prompt's round trip.
    fn ping(&mut self) -> Duration {
        let start = Instant::now();
        self.prompt("ping");
        start.elapsed()
    }

    /// A turn that streams `STREAM` chunks: until its answer.
    fn stream(&mut self) -> Duration {
        let start = Instant::now();
        let read = self.prompt(&format!("stream {STREAM} {CHUNK}"));
        assert!(read > STREAM * CHUNK, "{read} bytes streamed");
        start.elapsed()
    }

    /// Closes the agent's stdin, as an editor does, and waits for it to go.
    fn close(self) {
        let Editor { mut child, to, .. } = self;
        drop(to);
        child.wait().unwrap();
    }
}

/// The fake agent: answers initialize, session/new and session/prompt at
/// once; a prompt `stream <n> <bytes>` first streams n chunks of that many
/// bytes, `MESSAGE` chunks to a message.
fn agent() -> ExitCode {
    let (stdin, stdout) = (io::stdin().lock(), io::stdout().lock());
    let mut out = BufWriter::with_capacity(1 << 16, stdout);
    let session = format!("bench-{}", std::process::id());
    for line in stdin.lines() {
        let msg: Value = serde_json::from_str(&line.unwrap()).unwrap();
        let (Some(id), Some(method)) = (msg.get("id"), msg["method"].as_str()) else { continue };
        let result = match method {
            "initialize" => json!({ "protocolVersion": 1, "agentCapabilities": {} }),
            "session/new" => json!({ "sessionId": session }),
            "session/prompt" => {
                let text = msg["params"]["prompt"][0]["text"].as_str().unwrap_or_default();
                let mut words = text.split(' ').skip(1).map(|w| w.parse::<usize>().unwrap());
                if let (Some(n), Some(bytes)) = (words.next(), words.next()) {
                    // Rendered once, the message's number put in: the agent's
                    // own cost stays out of the ratio.
                    let content = json!({ "type": "text", "text": "x".repeat(bytes) });
                    let update = json!({
                        "sessionUpdate": "agent_message_chunk",
                        "content": content,
                        "messageId": "m#",
                    });
                    let params = json!({ "sessionId": session, "update": update });
                    let chunk =
                        json!({ "jsonrpc": "2.0", "method": "session/update", "params": params });
                    let chunk = chunk.to_string();
                    let (before, after) = chunk.split_once("m#").unwrap();
                    for i in 0..n {
                        writeln!(out, "{before}m{}{after}", i / MESSAGE).unwrap();
                    }
                }
                json!({ "stopReason": "end_turn" })
            }
            _ => {
                let error = json!({ "code": -32601, "message": "Method not found" });
                writeln!(out, "{}", json!({ "jsonrpc": "2.0", "id": id, "error": error })).unwrap();
                out.flush().unwrap();
                continue;
            }
        };
        writeln!(out, "{}", json!({ "jsonrpc": "2.0", "id": id, "result": result })).unwrap();
        out.flush().unwrap();
    }
    ExitCode::SUCCESS
}

/// A directory of the run's own: runtime directories, transcripts and the
/// config, short enough for a socket's path. Removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Scratch {
        let dir = std::env::temp_dir().join(format!("brnr-b{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::DirBuilder::new().mode(0o700).create(&dir).unwrap();
        let scratch = Scratch(dir);
        // Read only if nobody else can reach it (ADR 9.4). The bridge reads
        // its events and sends nothing, its stdout open: sh holds it.
        fs::DirBuilder::new().mode(0o700).create(scratch.0.join("config")).unwrap();
        let config = "[profiles.bridged]\n[[profiles.bridged.bridges]]\n\
                      command = \"/bin/sh\"\nargs = [\"-c\", \"cat > /dev/null\"]\n";
        let mut file =
            fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(scratch.config());
        file.as_mut().unwrap().write_all(config.as_bytes()).unwrap();
        scratch
    }

    fn config(&self) -> PathBuf {
        self.0.join("config/config.toml")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// `read` once it has given the same value four times in a row, 50 ms
/// apart: a thread or a page that comes a moment late is counted.
fn settled(mut read: impl FnMut() -> u64) -> u64 {
    let (mut last, mut same) = (read(), 0);
    let deadline = Instant::now() + Duration::from_secs(5);
    while same < 4 && Instant::now() < deadline {
        sleep(Duration::from_millis(50));
        let now = read();
        same = if now == last { same + 1 } else { 0 };
        last = now;
    }
    last
}

/// A process's resident memory, in KiB.
fn rss_kib(pid: i32) -> u64 {
    if cfg!(target_os = "linux") {
        return proc_status(pid, "VmRSS:");
    }
    let out = Command::new("ps").args(["-o", "rss=", "-p", &pid.to_string()]).output().unwrap();
    String::from_utf8_lossy(&out.stdout).trim().parse().unwrap()
}

/// A process's threads.
fn threads(pid: i32) -> u64 {
    if cfg!(target_os = "linux") {
        return proc_status(pid, "Threads:");
    }
    // A header, then a line per thread.
    let out = Command::new("ps").args(["-M", "-p", &pid.to_string()]).output().unwrap();
    String::from_utf8_lossy(&out.stdout).lines().count() as u64 - 1
}

/// The first number of `key`'s line in /proc/<pid>/status.
fn proc_status(pid: i32, key: &str) -> u64 {
    let status = fs::read_to_string(format!("/proc/{pid}/status")).unwrap();
    let line = status.lines().find(|l| l.starts_with(key)).unwrap();
    line[key.len()..].split_whitespace().next().unwrap().parse().unwrap()
}

fn median(times: &mut [Duration]) -> Duration {
    times.sort();
    times[times.len() / 2]
}

fn micros(t: Duration) -> String {
    format!("{} µs", t.as_micros())
}

fn mib(kib: u64) -> String {
    format!("{:.1} MiB", kib as f64 / 1024.0)
}

/// brnr's version, and the commit it was built from when git knows it.
fn version() -> String {
    let git = Command::new("git").args(["describe", "--always", "--dirty"]).output();
    let commit = git.ok().filter(|o| o.status.success()).map(|o| o.stdout);
    let commit = commit.map(|c| String::from_utf8_lossy(&c).trim().to_owned());
    let version = env!("CARGO_PKG_VERSION");
    commit.map_or(version.to_owned(), |c| format!("{version} ({c})"))
}

/// The OS, the CPU and how many there are.
fn machine() -> String {
    let cpu = if cfg!(target_os = "linux") {
        let info = fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
        let model = info.lines().find(|l| l.starts_with("model name"));
        model.and_then(|l| l.split(':').nth(1)).unwrap_or("").trim().to_owned()
    } else {
        let out = Command::new("sysctl").args(["-n", "machdep.cpu.brand_string"]).output();
        out.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned()).unwrap_or_default()
    };
    let cpus = std::thread::available_parallelism().map_or(0, |n| n.get());
    format!("{} {}, {cpu}, {cpus} CPUs", std::env::consts::OS, std::env::consts::ARCH)
}
