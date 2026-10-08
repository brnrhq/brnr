#!/usr/bin/env python3
"""Checks built adapters: each says the npm version adapters/package.json
pins, and answers ACP's initialize in JSON-RPC (with a stand-in for the agent,
which initialize doesn't need).

With --brnr, <dir> has brnr in it too, and brnr is checked with them, in a
scratch home: brnr doctor finds each next to it and reads its version, and
brnr sessions lists the Claude adapter's sessions (initialize and
session/list through brnr's ACP client, which Claude Code isn't needed for;
the Codex adapter needs the Codex CLI to answer either).

    adapters/check.py [--brnr] [<dir>]      default: target/release
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
pins = json.load(open(os.path.join(here, "package.json")))["dependencies"]
adapters = {
    "brnr-claude-adapter": "@agentclientprotocol/claude-agent-acp",
    "brnr-codex-adapter": "@agentclientprotocol/codex-acp",
}
request = {
    "jsonrpc": "2.0",
    "id": 1,
    "method": "initialize",
    "params": {"protocolVersion": 1, "clientCapabilities": {}},
}

agent = os.path.join(tempfile.mkdtemp(), "agent")
with open(agent, "w") as f:
    f.write("#!/bin/sh\nexit 1\n")
os.chmod(agent, 0o755)
env = dict(os.environ, CLAUDE_CODE_EXECUTABLE=agent, CODEX_PATH=agent)


def first_line(p, timeout=30):
    """p's first line of output, or "" if none comes within timeout seconds."""
    lines = queue.Queue()
    threading.Thread(target=lambda: lines.put(p.stdout.readline()), daemon=True).start()
    try:
        return lines.get(timeout=timeout)
    except queue.Empty:
        return ""


failed = False
for name, package in adapters.items():
    path = os.path.join(out, name)
    want = f"{name} {pins[package]} ({package})"
    got = subprocess.run(
        [path, "--version"], capture_output=True, text=True, timeout=30
    ).stdout.strip()
    if got != want:
        print(f"FAIL {name} --version: {got!r}, not {want!r}")
        failed = True
    # Stdin stays open until the answer is in: an adapter may exit at EOF.
    p = subprocess.Popen([path], stdin=subprocess.PIPE, stdout=subprocess.PIPE, env=env, text=True)
    p.stdin.write(json.dumps(request) + "\n")
    p.stdin.flush()
    line = first_line(p)
    p.kill()
    answer = json.loads(line) if line else {}
    if answer.get("id") != 1:
        print(f"FAIL {name} initialize: {line!r}")
        failed = True
    else:
        print(
            f"ok   {got}: initialize {'answered' if 'result' in answer else 'answered with an error'}"
        )

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
    for name, package in adapters.items():
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
