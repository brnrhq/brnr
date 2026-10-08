#!/usr/bin/env python3
"""Checks built adapters: each says the npm version adapters/package.json
pins, and answers ACP's initialize as it should without its agent, which is
a stand-in that prints its arguments on stderr and exits 3:

- brnr-claude-adapter initializes (Claude Code isn't needed for it): the
  answer is a result with protocolVersion 1, and agentCapabilities and
  authMethods, if given, an object and a list.
- brnr-codex-adapter can't initialize without the Codex CLI, so this checks
  what it can, and says so (transport only): that it ran the stand-in as
  `codex app-server` and answered with the error it gives when Codex exits,
  exactly: code 1001, "Codex process has exited with code 3:\\nstand-in agent:
  app-server". Any other error fails; a result is checked as Claude's is.

With --brnr, <dir> has brnr in it too, and brnr is checked with them, in a
scratch home: brnr doctor finds each next to it and reads its version, and
brnr sessions lists the Claude adapter's sessions (initialize and
session/list through brnr's ACP client).

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
pins = json.load(open(os.path.join(here, "package.json")))["dependencies"]
# Each adapter's package, and the error its initialize must give instead of
# a result when its agent is the stand-in, with what it needs to initialize.
adapters = {
    "brnr-claude-adapter": ("@agentclientprotocol/claude-agent-acp", None),
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
with open(agent, "w") as f:
    f.write('#!/bin/sh\necho "stand-in agent: $*" >&2\nexit 3\n')
os.chmod(agent, 0o755)
env = dict(os.environ, CLAUDE_CODE_EXECUTABLE=agent, CODEX_PATH=agent)


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


def initialize(path, expected):
    """What's wrong with path's answer to initialize, or None; and what it showed."""
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
        if expected and answer["error"] == expected[0]:
            return None, f"initialize, transport only: {expected[1]}"
        return f"error {shown(answer['error'])}", None
    result = answer.get("result")
    if not isinstance(result, dict) or result.get("protocolVersion") != 1:
        return f"not a result with protocolVersion 1: {shown(line)}", None
    if not isinstance(result.get("agentCapabilities", {}), dict):
        return f"agentCapabilities not an object: {shown(line)}", None
    if not isinstance(result.get("authMethods", []), list):
        return f"authMethods not a list: {shown(line)}", None
    return None, "initialize: a result, protocol version 1"


failed = False
for name, (package, expected) in adapters.items():
    path = os.path.join(out, name)
    want = f"{name} {pins[package]} ({package})"
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
    wrong, seen = initialize(path, expected)
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

    def run_brnr(*args):
        return subprocess.run(
            [brnr, *args], capture_output=True, text=True, timeout=60, env=benv, cwd=scratch
        )

    doctor = run_brnr("doctor", "--json")
    checks = {c["check"]: c for c in json.loads(doctor.stdout or "[]")}
    for name, (package, _) in adapters.items():
        # After its path, which may be out's through a symlink.
        want = f"/{name} ({package} {pins[package]})"
        got = checks.get(name, {})
        if got.get("level") != "ok" or not got.get("message", "").endswith(want):
            print(f"FAIL brnr doctor {name}: {got!r}, not ok and ending {want!r}")
            failed = True
        else:
            print(f"ok   brnr doctor: {name} {pins[package]}")
    sessions = run_brnr("sessions", "--json", "--", "brnr-claude-adapter")
    try:
        listed = json.loads(sessions.stdout) if sessions.returncode == 0 else None
    except json.JSONDecodeError:
        listed = None
    if not isinstance(listed, list):
        print(f"FAIL brnr sessions -- brnr-claude-adapter: {sessions.stdout + sessions.stderr!r}")
        failed = True
    else:
        print(f"ok   brnr sessions -- brnr-claude-adapter: {len(listed)} sessions")
    shutil.rmtree(scratch, ignore_errors=True)
sys.exit(1 if failed else 0)
