#!/usr/bin/env python3
"""A stand-in Claude Code for adapters/check.py: just enough of the Claude
Agent SDK's stream-json protocol for brnr-claude-adapter to open, fork, prompt
and close sessions without credentials.

It answers each control_request with success (initialize with one model, as
the adapter needs one), and each user message with an assistant message "ok"
and a result, and writes both to the session's transcript, where the SDK's
forkSession reads it: $CLAUDE_CONFIG_DIR (else ~/.claude)/projects/<cwd, each
character but a letter or digit a dash>/<session>.jsonl. `auth status` exits 1:
the adapter then reports no identity.
"""

import json
import os
import re
import sys
import uuid

args = sys.argv[1:]
if args[:1] == ["auth"]:
    sys.exit(1)
# --session-id=<id> for a new session, --resume=<id> for one opened again (a
# fork, from claude-agent-acp 0.90).
session = next(a.split("=", 1)[1] for a in args if a.startswith(("--session-id=", "--resume=")))
config = os.environ.get("CLAUDE_CONFIG_DIR") or os.path.expanduser("~/.claude")
project = os.path.join(config, "projects", re.sub(r"[^A-Za-z0-9]", "-", os.getcwd()))
os.makedirs(project, exist_ok=True)
parent = None


def send(message):
    print(json.dumps(message), flush=True)


def record(entry):
    """Appends entry to the transcript, after the last one."""
    global parent
    entry.update(uuid=str(uuid.uuid4()), parentUuid=parent, sessionId=session, cwd=os.getcwd())
    parent = entry["uuid"]
    with open(os.path.join(project, f"{session}.jsonl"), "a") as f:
        f.write(json.dumps(entry) + "\n")


initialized = {
    "commands": [],
    "agents": [],
    "output_style": "default",
    "available_output_styles": ["default"],
    "models": [{"value": "default", "displayName": "Default", "description": "stand-in"}],
    "account": {},
}
for line in sys.stdin:
    message = json.loads(line)
    if message.get("type") == "control_request":
        request = message["request"]
        answer = initialized if request.get("subtype") == "initialize" else {}
        send(
            {
                "type": "control_response",
                "response": {
                    "subtype": "success",
                    "request_id": message["request_id"],
                    "response": answer,
                },
            }
        )
    elif message.get("type") == "user":
        reply = {"role": "assistant", "content": [{"type": "text", "text": "ok"}]}
        record({"type": "user", "message": message["message"]})
        record({"type": "assistant", "message": reply})
        usage = {"input_tokens": 1, "output_tokens": 1}
        reply = dict(reply, id=f"msg_{uuid.uuid4().hex}", stop_reason="end_turn", usage=usage)
        send(
            {
                "type": "assistant",
                "session_id": session,
                "parent_tool_use_id": None,
                "message": reply,
            }
        )
        send(
            {
                "type": "result",
                "subtype": "success",
                "session_id": session,
                "result": "ok",
                "usage": usage,
            }
        )
