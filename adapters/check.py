#!/usr/bin/env python3
"""Checks built adapters: each says the npm version adapters/package.json
pins, and answers ACP's initialize in JSON-RPC (with a stand-in for the agent,
which initialize doesn't need).

    adapters/check.py [<dir>]      default: target/release
"""

import json
import os
import queue
import subprocess
import sys
import tempfile
import threading

here = os.path.dirname(os.path.abspath(__file__))
out = sys.argv[1] if len(sys.argv) > 1 else os.path.join(here, "..", "target", "release")
pins = json.load(open(os.path.join(here, "package.json")))["dependencies"]
adapters = {
    "brnr-claude-adapter": "@agentclientprotocol/claude-agent-acp",
    "brnr-codex-adapter": "@agentclientprotocol/codex-acp",
}
request = {"jsonrpc": "2.0", "id": 1, "method": "initialize",
           "params": {"protocolVersion": 1, "clientCapabilities": {}}}

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
    got = subprocess.run([path, "--version"], capture_output=True, text=True, timeout=30).stdout.strip()
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
        print(f"ok   {got}: initialize {'answered' if 'result' in answer else 'answered with an error'}")
sys.exit(1 if failed else 0)
