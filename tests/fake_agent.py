#!/usr/bin/env python3
"""A minimal ACP agent for the integration tests in headless.rs.

Every prompt it receives is appended to $PROMPT_LOG as one JSON string per
line. Behaviour is chosen with environment variables:

  NEW_DELAY=<s>     wait before answering session/new
  STALL=1           stop reading stdin once the session exists
  STUBBORN=all      ignore SIGTERM and stdin EOF; start a child that also
                    ignores SIGTERM
  STUBBORN=child    only the child ignores SIGTERM
  CHILD_PID=<file>  where the child's pid is written
  FLOOD=<n>         stream n agent_message_chunk updates for every prompt
  PERMISSION=1      ask permission before answering a prompt
  CANCEL_DELAY=<s>  wait before honouring session/cancel

A prompt whose text starts with "hang" runs until it is cancelled.
"""

import json
import os
import signal
import subprocess
import sys
import time

env = os.environ.get


def send(msg):
    sys.stdout.write(json.dumps(msg) + "\n")
    sys.stdout.flush()


def log_prompt(text):
    with open(env("PROMPT_LOG"), "a") as f:
        f.write(json.dumps(text) + "\n")


def end_turn(mid, reason="end_turn"):
    send({"jsonrpc": "2.0", "id": mid, "result": {"stopReason": reason}})


stubborn = env("STUBBORN")
if stubborn:
    if stubborn == "all":
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
    child = subprocess.Popen(
        ["sleep", "1000"],
        preexec_fn=lambda: signal.signal(signal.SIGTERM, signal.SIG_IGN),
    )
    with open(env("CHILD_PID"), "w") as f:
        f.write(str(child.pid))

hanging = None  # id of a prompt that runs until cancelled
asking = None  # id of a prompt waiting for a permission answer

for line in sys.stdin:
    msg = json.loads(line)
    method, mid = msg.get("method"), msg.get("id")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": mid, "result": {"protocolVersion": 1, "agentCapabilities": {}}})
    elif method == "session/new":
        time.sleep(float(env("NEW_DELAY", "0")))
        send({"jsonrpc": "2.0", "id": mid, "result": {"sessionId": "sess-1"}})
        if env("STALL"):
            time.sleep(100000)
    elif method == "session/prompt":
        text = "\n".join(b.get("text", "") for b in msg["params"]["prompt"])
        log_prompt(text)
        for _ in range(int(env("FLOOD", "0"))):
            update = {"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "y" * 500}}
            send({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": "sess-1", "update": update}})
        if text.startswith("hang"):
            hanging = mid
        elif env("PERMISSION"):
            asking = mid
            options = [
                {"optionId": "allow", "name": "Allow", "kind": "allow_once"},
                {"optionId": "reject", "name": "Reject", "kind": "reject_once"},
            ]
            params = {"sessionId": "sess-1", "toolCall": {"toolCallId": "t1", "title": "Edit a file"}, "options": options}
            send({"jsonrpc": "2.0", "id": "perm-1", "method": "session/request_permission", "params": params})
        else:
            end_turn(mid)
    elif method == "session/cancel":
        time.sleep(float(env("CANCEL_DELAY", "0")))
        if hanging is not None:
            end_turn(hanging, "cancelled")
            hanging = None
    elif method is None and mid == "perm-1" and asking is not None:
        end_turn(asking)
        asking = None

if stubborn == "all":
    time.sleep(100000)
