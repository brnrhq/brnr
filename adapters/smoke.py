#!/usr/bin/env python3
"""Checks an agent through brnr, with the agent itself (ADR 11.9): a smoke
check for agents that speak ACP themselves, run by hand. It uses the
agent's login and its account's quota, and its prompts are short: four
turns in all, at most.

    adapters/smoke.py [--brnr <brnr>] junie | cursor | -- <agent> [args...]

`junie` is `junie --acp true`, `cursor` is `cursor-agent acp`. brnr is
<brnr>, by default target/release/brnr. It runs in a scratch BRNR_HOME,
BRNR_DIR and project folder (no config); HOME stays, for the agent's login.

It prints brnr's version and the agent's (`<program> --version`, and the
agentInfo it gives in initialize), then a line per check: `ok   <check>:
<what was seen>`, `FAIL <check>: <why>`, or `--   <check>: unsupported
(<the capability the agent doesn't advertise>)` or `not run: <why>`. In order:

- initialize: the process starts, and what the agent advertises.
- session/new: it opens a session.
- prompt: a prompt is answered "ok" (any case, a full stop allowed), the
  turn ending end_turn.
- cancel: a long turn, cancelled once it runs, ends cancelled.
- fork (sessionCapabilities.fork): a fork's prompt is answered "ok", and
  the fork closed, where the agent can close sessions.
- shutdown: `brnr process stop`; the process, the agent and every process
  the agent started are gone, and the host log ends with process_exited.
- list (sessionCapabilities.list): the agent lists the session, in its cwd.
- resume (sessionCapabilities.resume, else loadSession): the session
  resumed in a new process answers a prompt "ok".
- close (sessionCapabilities.close): the session closes, and its process
  stops with it; without it, the process is stopped.

Then the sessions it made are deleted from the agent's store where the
agent can, and said where it can't. However it ends (Ctrl-C too), it stops
the processes it started and waits for them to go before it removes its
scratch folder, which it keeps, saying so, if one doesn't go. Exits 1 if a
check failed, else 0. A
step has $BRNR_SMOKE_TIMEOUT seconds (default 120). $BRNR_SMOKE_PROMPT and
$BRNR_SMOKE_LONG_PROMPT replace the prompts (the test with the fake agent
uses them). $BRNR_SMOKE_KEEP set keeps the scratch folder, with brnr's
transcripts and host logs, to look into a failure. Where a failure comes from, brnr or the agent, is for a person
to find out; docs/guide/native-agents.md has the results.
"""

import atexit
import json
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import time

here = os.path.dirname(os.path.abspath(__file__))
known = {"junie": ["junie", "--acp", "true"], "cursor": ["cursor-agent", "acp"]}
usage = "usage: adapters/smoke.py [--brnr <brnr>] junie | cursor | -- <agent> [args...]"

args = sys.argv[1:]
brnr = os.path.join(here, "..", "target", "release", "brnr")
if args[:1] == ["--brnr"] and len(args) > 1:
    brnr, args = args[1], args[2:]
if args[:1] == ["--"] and len(args) > 1:
    agent = args[1:]
elif len(args) == 1 and args[0] in known:
    agent = known[args[0]]
else:
    sys.exit(usage)
brnr = os.path.abspath(brnr)
timeout = float(os.environ.get("BRNR_SMOKE_TIMEOUT", "120"))
reply_prompt = os.environ.get("BRNR_SMOKE_PROMPT", "Reply with the word ok and nothing else.")
long_prompt = os.environ.get("BRNR_SMOKE_LONG_PROMPT", "Count from 1 to 500, one number per line.")

# Short: the control sockets go in BRNR_DIR, and must fit in sun_path. Real:
# an agent may record the cwd with its symlinks resolved (/tmp on macOS).
scratch = os.path.realpath(tempfile.mkdtemp(prefix="brnr-smoke.", dir="/tmp"))
project = os.path.join(scratch, "project")
os.mkdir(project)
env = dict(
    os.environ,
    BRNR_DIR=os.path.join(scratch, "run"),
    BRNR_HOME=os.path.join(scratch, "brnr"),
    BRNR_CONFIG=os.path.join(scratch, "none.toml"),
)
failed = False


def shown(s):
    """s for a message: repr'd, so nothing in it is a control character, and cut short."""
    r = repr(s)
    return r if len(r) <= 300 else r[:300] + "..."


def say(level, check, what):
    global failed
    failed = failed or level == "FAIL"
    print(f"{level:<4} {check}: {what}", flush=True)


def run(*argv, wait=None):
    """brnr's exit status, its JSON answer (None if it gave none) and all it printed."""
    try:
        p = subprocess.run(
            [brnr, *argv],
            capture_output=True,
            text=True,
            timeout=wait or timeout + 30,
            env=env,
            cwd=project,
        )
    except subprocess.TimeoutExpired:
        return None, None, f"no answer in {wait or timeout + 30:g}s"
    try:
        got = json.loads(p.stdout)
    except json.JSONDecodeError:
        got = None
    return p.returncode, got if isinstance(got, dict) else None, p.stdout + p.stderr


def why(got, out):
    """What a failed command said: its error, else what it printed."""
    return shown((got or {}).get("error") or out.strip())


def status(pid):
    """The process's status reply (the bridge protocol's), or None."""
    try:
        with socket.socket(socket.AF_UNIX) as s:
            s.settimeout(10)
            s.connect(os.path.join(env["BRNR_DIR"], f"{pid}.sock"))
            s.sendall(b'{"cmd":"status","req_id":"smoke"}\n')
            for line in s.makefile():
                answer = json.loads(line)
                if answer.get("req_id") == "smoke":
                    return answer
    except (OSError, ValueError):
        return None


def replied(check, got, rc, out):
    """Whether a --wait --json answer is "ok" ending end_turn; says so."""
    reply = (got or {}).get("reply")
    stop = (got or {}).get("stop_reason")
    if rc == 0 and stop == "end_turn" and (reply or "").strip().rstrip(".!").lower() == "ok":
        say("ok", check, f"replied {reply.strip()!r} ({stop})")
        return True
    if got and "reply" in got:
        say("FAIL", check, f"replied {shown(reply)}, stop reason {stop}, error {got.get('error')}")
    else:
        say("FAIL", check, why(got, out))
    return False


def alive(pid):
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    # A zombie has exited: only its parent hasn't waited for it yet.
    state = subprocess.run(["ps", "-o", "stat=", "-p", str(pid)], capture_output=True, text=True)
    return not state.stdout.strip().startswith("Z")


def descendants(pid):
    """The processes pid started, and theirs, as ps has them now."""
    ps = subprocess.run(["ps", "-A", "-o", "pid=,ppid="], capture_output=True, text=True)
    children = {}
    for line in ps.stdout.splitlines():
        child, parent = map(int, line.split())
        children.setdefault(parent, []).append(child)
    found, todo = [], [pid]
    while todo:
        for child in children.get(todo.pop(), []):
            found.append(child)
            todo.append(child)
    return found


def gone(pids, within):
    """The time it took for all of pids to go, or None if one is still there."""
    start = time.monotonic()
    while time.monotonic() - start < within:
        if not any(alive(p) for p in pids):
            return time.monotonic() - start
        time.sleep(0.1)
    return None


def stop(pid, check):
    """Stops process pid, and checks that it and its agent's processes go."""
    st = status(pid) or {}
    agent_pid = st.get("agent_pid")
    started = descendants(agent_pid) if agent_pid else []
    pids = [pid] + ([agent_pid] if agent_pid else []) + started
    rc, got, out = run("process", "stop", str(pid), "--json")
    if rc != 0:
        say("FAIL", check, f"brnr process stop: {why(got, out)}")
        return
    # brnr closes the agent's stdin, then sends SIGTERM 5 s later, SIGKILL 5 s
    # after that.
    took = gone(pids, 20)
    if took is None:
        left = [p for p in pids if alive(p)]
        say("FAIL", check, f"still running 20 s after brnr process stop: {left}")
        return
    ended = None
    try:
        with open(st.get("host_log", "")) as log:
            records = [json.loads(line) for line in log if line.strip()]
        ended = records[-1]["event"] if records else None
    except (OSError, ValueError, KeyError):
        pass
    if not ended or ended.get("event") != "process_exited":
        say("FAIL", check, f"the host log doesn't end with process_exited: {shown(ended)}")
        return
    exited = ended.get("exit_status")
    said = f"process {pid}, agent {agent_pid} and its {len(started)} processes gone in {took:.1f} s"
    term = took >= 5 and exited in ({"code": 143}, {"signal": 15}, {"signal": "SIGTERM"})
    note = "; SIGTERM: it didn't exit when its stdin closed" if term else ""
    say("ok", check, f"{said}; the agent exited {exited}{note}")


def version():
    """The agent's own version: the last line `<program> --version` prints."""
    try:
        p = subprocess.run(
            [agent[0], "--version"],
            capture_output=True,
            text=True,
            timeout=30,
            stdin=subprocess.DEVNULL,
            env=env,
            cwd=project,
        )
        lines = [ln.strip() for ln in p.stdout.splitlines() if ln.strip()]
        return lines[-1] if lines else "(it printed none)"
    except (OSError, subprocess.TimeoutExpired) as e:
        return f"(--version failed: {e})"


def cleanup():
    """Stops what is left in this scratch BRNR_DIR, and only that; removes
    the scratch folder once they are gone, and keeps it if one isn't."""
    rc, got, _ = run("process", "list", "--json", wait=30)
    pids = [p["pid"] for p in (got or {}).get("processes", [])]
    for pid in pids:
        run("process", "stop", str(pid), wait=30)
    stopped = rc == 0 and got is not None and gone(pids, 20) is not None
    if not stopped:
        left = [p for p in pids if alive(p)] if got is not None else "unknown"
        print(f"note: kept {scratch}: its processes may not have stopped ({left})")
    elif os.environ.get("BRNR_SMOKE_KEEP"):
        print(f"note: kept {scratch}, its transcripts and host logs")
    else:
        shutil.rmtree(scratch, ignore_errors=True)


# Whatever ends the check, an exit, an exception or Ctrl-C, stops what it
# started (P14).
atexit.register(cleanup)
rc, got, out = run("--version")
print(f"brnr: {out.strip()} ({brnr})")
print(f"agent: {' '.join(agent)}: {version()}")
made = []

# initialize and session/new: a process started with no prompt.
rc, got, out = run("session", "new", "--cwd", project, "--json", "--", *agent)
session, pid = (got or {}).get("session"), (got or {}).get("pid")
st = status(pid) if rc == 0 and pid else None
if not st:
    say("FAIL", "initialize", why(got, out))
    say("FAIL", "session/new", "no process")
    sys.exit(1)
caps = st.get("capabilities", {})
info = st.get("agent_info") or {}
advertised = ", ".join(k for k, v in caps.items() if v) or "nothing optional"
named = f"{info.get('name')} {info.get('version')}" if info else "no agentInfo"
say("ok", "initialize", f"{named}; advertises {advertised}")
say("ok", "session/new", f"{session} in process {pid}")
made.append(session)

wait = ("--wait", "--timeout", f"{timeout:g}", "--json")
rc, got, out = run("prompt", "send", session, "--prompt", reply_prompt, *wait)
replied("prompt", got, rc, out)

# cancel: a long turn, cancelled once it runs.
turn = subprocess.Popen(
    [brnr, "prompt", "send", session, "--prompt", long_prompt, *wait],
    stdout=subprocess.PIPE,
    stderr=subprocess.PIPE,
    text=True,
    env=env,
    cwd=project,
)
began, deadline = False, time.monotonic() + timeout
while time.monotonic() < deadline and turn.poll() is None:
    rc, got, _ = run("session", "status", session, "--json")
    if (got or {}).get("state") == "busy":
        began = True
        break
    time.sleep(0.2)
if began:
    time.sleep(1)  # Into the turn, not at its start.
    rc, cancel, out = run("prompt", "cancel", session, "--json")
try:
    stdout, stderr = turn.communicate(timeout=timeout)
except subprocess.TimeoutExpired:
    turn.kill()
    stdout, stderr = turn.communicate()
try:
    ended = json.loads(stdout)
except json.JSONDecodeError:
    ended = {}
if not began:
    say("FAIL", "cancel", f"the turn never ran: {shown(stdout + stderr)}")
elif rc != 0:
    say("FAIL", "cancel", f"brnr prompt cancel: {why(cancel, out)}")
elif ended.get("stop_reason") == "cancelled":
    say("ok", "cancel", f"the turn ended cancelled after {len(ended.get('reply', ''))} characters")
else:
    say("FAIL", "cancel", f"the turn ended {shown(ended or stdout + stderr)}")

if not caps.get("fork"):
    say("--", "fork", "unsupported (sessionCapabilities.fork)")
else:
    rc, got, out = run("session", "fork", session, "--json")
    fork = (got or {}).get("session")
    if rc != 0 or not fork:
        say("FAIL", "fork", why(got, out))
    else:
        made.append(fork)
        rc, got, out = run("prompt", "send", fork, "--prompt", reply_prompt, *wait)
        if replied(f"fork {fork}, its prompt", got, rc, out) and caps.get("close"):
            rc, got, out = run("session", "close", fork, "--json")
            if rc == 0:
                say("ok", "fork, closed", fork)
            else:
                say("FAIL", "fork, closed", why(got, out))

stop(pid, "shutdown")

if not caps.get("list"):
    say("--", "list", "unsupported (sessionCapabilities.list)")
else:
    rc, got, out = run("session", "list", "--cwd", project, "--json", "--", *agent)
    rows = [r for r in (got or {}).get("sessions", []) if r.get("session") == session]
    if rc != 0:
        say("FAIL", "list", why(got, out))
    elif rows and rows[0].get("source") in ("agent", "both"):
        say("ok", "list", f"the agent lists {session} in {project}")
    else:
        say("FAIL", "list", f"the agent doesn't list {session} in {project}")

how = "session/resume" if caps.get("resume") else "session/load" if caps.get("load") else None
pid = None
if not how:
    say("--", "resume", "unsupported (sessionCapabilities.resume, loadSession)")
else:
    rc, got, out = run("session", "resume", session, "--prompt", reply_prompt, *wait)
    pid = (got or {}).get("pid")
    replied(f"resume ({how})", got, rc, out)

if pid and caps.get("close"):
    rc, got, out = run("session", "close", session, "--json")
    took = gone([pid], 20) if rc == 0 else None
    if rc != 0:
        say("FAIL", "close", why(got, out))
    elif took is None:
        say("FAIL", "close", f"closed, but process {pid} still runs 20 s later")
    else:
        say("ok", "close", f"closed; process {pid} stopped with it in {took:.1f} s")
elif not caps.get("close"):
    say("--", "close", "unsupported (sessionCapabilities.close)")
else:
    say("--", "close", "not run: no session was resumed to close")
if pid and alive(pid):
    stop(pid, "shutdown, after resume")

# The sessions made, out of the agent's store, where it can delete them.
for s in made:
    rc, got, out = run("session", "delete", s, "--json", "--", *agent)
    if rc != 0:
        print(f"note: {s} stays in the agent's store: {why(got, out)}")

sys.exit(1 if failed else 0)
