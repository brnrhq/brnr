#!/usr/bin/env python3
"""Checks built adapters: each says the npm version adapters/package.json
pins, and answers ACP's initialize as it should without its agent, which is
a stand-in that prints its arguments on stderr and exits 3:

- brnr-claude-adapter initializes (Claude Code isn't needed for it): the
  answer is a result with protocolVersion 1, and agentCapabilities and
  authMethods, if given, an object and a list.
- brnr-muse-adapter answers ACP v1 initialize after its stand-in `muse serve`
  exits. This is transport only, not authenticated Muse behavior.
- brnr-codex-adapter can't initialize without the Codex CLI, so this checks
  what it can, and says so (transport only): that it ran the stand-in as
  `codex app-server` and answered with the error it gives when Codex exits:
  code 1001 and, exactly, "Codex process has exited with code 3:\\nstand-in
  agent: app-server". Its other keys aren't checked (codex-acp 2.2 added
  `data`, with the exit status). Any other error fails; a result is checked
  as Claude's is.

With --brnr, <dir> has brnr in it too, and brnr is checked with them, in a
scratch home: brnr doctor finds each next to it and reads its version, and
brnr session list lists the Claude adapter's sessions (initialize and
session/list through brnr's ACP client). Then a fork's lifecycle in one brnr
process (ADR 3.12), through the Claude adapter with fake_claude.py as its
Claude Code: a session started with a prompt, forked, the fork prompted and
closed, and the session prompted again; each prompt answered "ok".

    adapters/check.py [--brnr] [<dir>]      default: target/release

Prints a line per check, `ok   <what>: <what was seen>` or `FAIL <what>:
<why>` (what an adapter said repr'd, cut to 300 characters), and exits 1 if
any failed, else 0. An adapter has $BRNR_CHECK_TIMEOUT seconds (default 30)
to answer each. adapters/test_check.py tests this against stub adapters.
"""

import json
import os
import queue
import shutil
import subprocess
import sys
import tempfile
import threading

here = os.path.dirname(os.path.abspath(__file__))
args = sys.argv[1:]
with_brnr = "--brnr" in args
args = [a for a in args if a != "--brnr"]
out = os.path.abspath(args[0] if args else os.path.join(here, "..", "target", "release"))
timeout = float(os.environ.get("BRNR_CHECK_TIMEOUT", "30"))
manifest = json.load(open(os.path.join(here, "package.json")))
pins = manifest["dependencies"] | manifest["devDependencies"]
# Each adapter's package, and the code and message of the error its initialize
# must give instead of a result when its agent is the stand-in, with what it
# needs to initialize.
adapters = {
    "brnr-claude-adapter": ("@agentclientprotocol/claude-agent-acp", None),
    "brnr-muse-adapter": ("@brokkai/muse-acp", None),
    "brnr-codex-adapter": (
        "@agentclientprotocol/codex-acp",
        (
            # codex-acp's CODEX_PROCESS_EXITED_ERROR_CODE, and its message
            # with the stand-in's exit status and stderr.
            {
                "code": 1001,
                "message": "Codex process has exited with code 3:\nstand-in agent: app-server",
            },
            "needs the Codex CLI: ran the stand-in as `codex app-server`, answered error 1001",
        ),
    ),
}
request = {
    "jsonrpc": "2.0",
    "id": 1,
    "method": "initialize",
    "params": {"protocolVersion": 1, "clientCapabilities": {}},
}

agent = os.path.join(tempfile.mkdtemp(), "agent")
agent_calls = agent + ".calls"
with open(agent, "w") as f:
    f.write(
        '#!/bin/sh\nprintf "%s\\n" "$*" >> "$BRNR_CHECK_AGENT_LOG"\n'
        'echo "stand-in agent: $*" >&2\nexit 3\n'
    )
os.chmod(agent, 0o755)
env = dict(
    os.environ,
    CLAUDE_CODE_EXECUTABLE=agent,
    CODEX_PATH=agent,
    MUSE_CLI=agent,
    BRNR_CHECK_AGENT_LOG=agent_calls,
)


def shown(s):
    """s for a message: repr'd, so nothing in it is a control character, and cut short."""
    r = repr(s)
    return r if len(r) <= 300 else r[:300] + "..."


def first_line(p):
    """p's first line of output: "" at EOF, None if none comes in time."""
    lines = queue.Queue()
    threading.Thread(target=lambda: lines.put(p.stdout.readline()), daemon=True).start()
    try:
        return lines.get(timeout=timeout)
    except queue.Empty:
        return None


def initialize(name, path, expected):
    """What's wrong with path's answer to initialize, or None; and what it showed."""
    open(agent_calls, "w").close()
    try:
        # Stdin stays open until the answer is in: an adapter may exit at EOF.
        p = subprocess.Popen(
            [path], stdin=subprocess.PIPE, stdout=subprocess.PIPE, env=env, text=True
        )
    except OSError as e:
        return f"can't run it: {e}", None
    try:
        p.stdin.write(json.dumps(request) + "\n")
        p.stdin.flush()
    except OSError:
        pass  # It exited already: EOF, below.
    line = first_line(p)
    p.kill()
    p.wait()
    if line is None:
        return f"no answer in {timeout:g}s", None
    if not line:
        return "no answer: its output ended", None
    try:
        answer = json.loads(line)
    except json.JSONDecodeError:
        return f"not JSON: {shown(line)}", None
    if not isinstance(answer, dict) or answer.get("id") != 1:
        return f"not an answer to request 1: {shown(line)}", None
    if "error" in answer:
        error = answer["error"]
        # The code and message expected; other keys, such as data, may be there.
        if (
            expected
            and isinstance(error, dict)
            and all(error.get(k) == v for k, v in expected[0].items())
        ):
            return None, f"initialize, transport only: {expected[1]}"
        return f"error {shown(error)}", None
    result = answer.get("result")
    if not isinstance(result, dict) or result.get("protocolVersion") != 1:
        return f"not a result with protocolVersion 1: {shown(line)}", None
    if not isinstance(result.get("agentCapabilities", {}), dict):
        return f"agentCapabilities not an object: {shown(line)}", None
    if not isinstance(result.get("authMethods", []), list):
        return f"authMethods not a list: {shown(line)}", None
    if name == "brnr-muse-adapter":
        info = result.get("agentInfo")
        if not isinstance(info, dict) or info.get("version") != pins["@brokkai/muse-acp"]:
            return f"Muse agentInfo version does not match the pin: {shown(line)}", None
        with open(agent_calls) as calls:
            if not any(call.strip() == "serve" or call.startswith("serve ") for call in calls):
                return "Muse did not launch the stand-in as `muse serve`", None
        return (
            None,
            "initialize, transport only: protocol version 1 after stand-in `muse serve` exited",
        )
    return None, "initialize: a result, protocol version 1"


failed = False
for name, (package, expected) in adapters.items():
    path = os.path.join(out, name)
    want = (
        f"muse-acp {pins[package]}"
        if name == "brnr-muse-adapter"
        else f"{name} {pins[package]} ({package})"
    )
    try:
        got = subprocess.run(
            [path, "--version"], capture_output=True, text=True, timeout=timeout
        ).stdout.strip()
    except (OSError, subprocess.TimeoutExpired) as e:
        got = f"<{e}>"
    if got != want:
        print(f"FAIL {name} --version: {shown(got)}, not {want!r}")
        failed = True
    else:
        print(f"ok   {name} --version: {pins[package]} ({package})")
    wrong, seen = initialize(name, path, expected)
    if wrong:
        print(f"FAIL {name} initialize: {wrong}")
        failed = True
    else:
        print(f"ok   {name} {seen}")

if with_brnr:
    brnr = os.path.join(out, "brnr")
    # Short: brnr's control sockets go in BRNR_DIR, and must fit in sun_path.
    scratch = tempfile.mkdtemp(prefix="brnr-check.", dir="/tmp")
    os.mkdir(os.path.join(scratch, "home"))
    benv = dict(
        env,
        BRNR_DIR=os.path.join(scratch, "run"),
        BRNR_HOME=os.path.join(scratch, "brnr"),
        BRNR_CONFIG=os.path.join(scratch, "none.toml"),
        HOME=os.path.join(scratch, "home"),
    )

    def run_brnr(*args, env=benv):
        return subprocess.run(
            [brnr, *args], capture_output=True, text=True, timeout=60, env=env, cwd=scratch
        )

    doctor = run_brnr("doctor", "--json")
    checks = {c["check"]: c for c in json.loads(doctor.stdout or "{}").get("checks", [])}
    for name, (package, _) in adapters.items():
        # After its path, which may be out's through a symlink.
        want = f"/{name} ({package} {pins[package]})"
        got = checks.get(name, {})
        if got.get("level") != "ok" or not got.get("message", "").endswith(want):
            print(f"FAIL brnr doctor {name}: {got!r}, not ok and ending {want!r}")
            failed = True
        else:
            print(f"ok   brnr doctor: {name} {pins[package]}")
    sessions = run_brnr("session", "list", "--json", "--", "brnr-claude-adapter")
    try:
        listed = json.loads(sessions.stdout) if sessions.returncode == 0 else None
    except json.JSONDecodeError:
        listed = None
    listed = listed.get("sessions") if isinstance(listed, dict) else None
    if not isinstance(listed, list):
        print(
            f"FAIL brnr session list -- brnr-claude-adapter: {sessions.stdout + sessions.stderr!r}"
        )
        failed = True
    else:
        print(f"ok   brnr session list -- brnr-claude-adapter: {len(listed)} sessions")

    # A fork prompted and closed in the process that forked it, and its parent
    # prompted after (ADR 3.12), with fake_claude.py as Claude Code.
    fenv = dict(
        benv,
        CLAUDE_CODE_EXECUTABLE=os.path.join(here, "fake_claude.py"),
        CLAUDE_CONFIG_DIR=os.path.join(scratch, "claude"),
    )

    def step(what, args, want, seen=""):
        """brnr's JSON answer to args if it has want's values, else None; prints
        what's line, with seen if ok."""
        global failed
        p = run_brnr(*args, env=fenv)
        try:
            got = json.loads(p.stdout)
        except json.JSONDecodeError:
            got = None
        if (
            p.returncode == 0
            and isinstance(got, dict)
            and all(got.get(k) == v for k, v in want.items())
        ):
            print(f"ok   {what}{seen}")
            return got
        error = got.get("error") if isinstance(got, dict) else None
        print(f"FAIL {what}: {shown(error or got or p.stdout + p.stderr)}")
        failed = True
        return None

    wait = ("--wait", "--timeout", f"{timeout:g}", "--json")
    replied = ({"reply": "ok", "error": None}, ": replied 'ok'")
    parent = step(
        "brnr session new --prompt -- brnr-claude-adapter, with a stand-in Claude Code",
        ("session", "new", "--prompt", "Reply ok.", *wait, "--", "brnr-claude-adapter"),
        *replied,
    )
    parent = parent and parent["session"]
    fork = parent and step(
        "brnr session fork", ("session", "fork", parent, "--json"), {"forked_from": parent}
    )
    if fork:
        fork = fork["session"]
        step(
            "brnr prompt send to the fork, in the process that forked it",
            ("prompt", "send", fork, *wait, "--prompt", "Reply ok."),
            *replied,
        )
        step("brnr session close the fork", ("session", "close", fork, "--json"), {"session": fork})
        step(
            "brnr prompt send to its parent, after",
            ("prompt", "send", parent, *wait, "--prompt", "Reply ok."),
            *replied,
        )
    # Only this scratch BRNR_DIR's processes: the one session new started.
    processes = run_brnr("process", "list", "--json")
    for process in json.loads(processes.stdout or "{}").get("processes", []):
        run_brnr("process", "stop", str(process["pid"]))
    shutil.rmtree(scratch, ignore_errors=True)
sys.exit(1 if failed else 0)
