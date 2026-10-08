#!/usr/bin/env python3
"""Tests adapters/check.py against stub adapters: scripts in a scratch
directory that answer --version as the pinned adapters do, and initialize as
each case says. Runs without bun or the adapters built.

    adapters/test_check.py
"""

import json
import os
import subprocess
import sys
import tempfile
import unittest

here = os.path.dirname(os.path.abspath(__file__))
pins = json.load(open(os.path.join(here, "package.json")))["dependencies"]
packages = {
    "brnr-claude-adapter": "@agentclientprotocol/claude-agent-acp",
    "brnr-codex-adapter": "@agentclientprotocol/codex-acp",
}
result = {"protocolVersion": 1, "agentCapabilities": {"loadSession": True}, "authMethods": []}
exited = {
    "code": 1001,
    "message": "Codex process has exited with code 3:\nstand-in agent: app-server",
}
# What a stub writes after reading initialize, by name: a line, or None to
# exit, or "hang" to say nothing.
answers = {
    "result": {"jsonrpc": "2.0", "id": 1, "result": result},
    "exited": {"jsonrpc": "2.0", "id": 1, "error": exited},
    "other error": {"jsonrpc": "2.0", "id": 1, "error": {"code": -32603, "message": "Internal"}},
    "exited otherwise": {
        "jsonrpc": "2.0",
        "id": 1,
        "error": dict(exited, message="Codex process has exited with code 127"),
    },
    "protocol 2": {"jsonrpc": "2.0", "id": 1, "result": dict(result, protocolVersion=2)},
    "capabilities a list": {
        "jsonrpc": "2.0",
        "id": 1,
        "result": dict(result, agentCapabilities=[]),
    },
    "wrong id": {"jsonrpc": "2.0", "id": 2, "result": result},
    "malformed": "not json \x1b[31m" + "x" * 1000,
    "eof": None,
    "hang": "hang",
}

stub = """#!{python}
import json, sys, time
if sys.argv[1:] == ["--version"]:
    print({version!r})
    sys.exit(0)
sys.stdin.readline()
answer = {answer!r}
if answer == "hang":
    time.sleep(60)
elif answer is not None:
    print(answer if isinstance(answer, str) else json.dumps(answer), flush=True)
"""


def check(claude="result", codex="exited", version=None):
    """check.py's exit status and output, run on stubs answering claude and codex."""
    with tempfile.TemporaryDirectory() as out:
        for name, answer in (("brnr-claude-adapter", claude), ("brnr-codex-adapter", codex)):
            path = os.path.join(out, name)
            with open(path, "w") as f:
                v = version or f"{name} {pins[packages[name]]} ({packages[name]})"
                f.write(stub.format(python=sys.executable, version=v, answer=answers[answer]))
            os.chmod(path, 0o755)
        p = subprocess.run(
            [os.path.join(here, "check.py"), out],
            capture_output=True,
            text=True,
            timeout=60,
            env=dict(os.environ, BRNR_CHECK_TIMEOUT="1"),
        )
        return p.returncode, p.stdout


class Check(unittest.TestCase):
    def test_passes_when_claude_initializes_and_codex_reports_its_exit(self):
        status, output = check()
        self.assertEqual(status, 0, output)
        self.assertIn("ok   brnr-claude-adapter initialize: a result, protocol version 1", output)
        self.assertIn("ok   brnr-codex-adapter initialize, transport only: needs the Codex", output)

    def test_passes_when_codex_initializes(self):
        status, output = check(codex="result")
        self.assertEqual(status, 0, output)
        self.assertIn("ok   brnr-codex-adapter initialize: a result, protocol version 1", output)

    def test_fails_on_claude_errors(self):
        for answer in ("other error", "exited"):
            with self.subTest(answer):
                status, output = check(claude=answer)
                self.assertEqual(status, 1, output)
                self.assertIn("FAIL brnr-claude-adapter initialize: error {'code':", output)

    def test_fails_on_an_unexpected_codex_error(self):
        for answer in ("other error", "exited otherwise"):
            with self.subTest(answer):
                status, output = check(codex=answer)
                self.assertEqual(status, 1, output)
                self.assertIn("FAIL brnr-codex-adapter initialize: error {'code':", output)

    def test_fails_on_answers_that_arent_a_successful_initialize(self):
        for answer, why in (
            ("protocol 2", "not a result with protocolVersion 1"),
            ("capabilities a list", "agentCapabilities not an object"),
            ("wrong id", "not an answer to request 1"),
            ("malformed", "not JSON: 'not json \\x1b[31mxxx"),
            ("eof", "no answer: its output ended"),
            ("hang", "no answer in 1s"),
        ):
            for name, answered in (("claude", {"claude": answer}), ("codex", {"codex": answer})):
                with self.subTest(answer, adapter=name):
                    status, output = check(**answered)
                    self.assertEqual(status, 1, output)
                    self.assertIn(f"FAIL brnr-{name}-adapter initialize: {why}", output)

    def test_cuts_what_it_shows_short(self):
        _, output = check(claude="malformed")
        line = next(ln for ln in output.splitlines() if ln.startswith("FAIL"))
        self.assertTrue(line.endswith("..."), line)
        self.assertLess(len(line), 400)

    def test_fails_on_the_wrong_version(self):
        status, output = check(version="brnr-claude-adapter 0.0.0 (somewhere)")
        self.assertEqual(status, 1, output)
        self.assertIn("FAIL brnr-claude-adapter --version: 'brnr-claude-adapter 0.0.0", output)


if __name__ == "__main__":
    unittest.main()
