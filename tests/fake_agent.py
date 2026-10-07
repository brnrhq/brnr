#!/usr/bin/env python3
"""A minimal ACP agent for the integration tests.

Every prompt it receives is appended to $PROMPT_LOG as one JSON string per
line, and every message it receives to $CALL_LOG as JSON. Behaviour is chosen
with environment variables:

  NEW_DELAY=<s>     wait before answering session/new
  STALL=1           stop reading stdin once the session exists
  STUBBORN=all      ignore SIGTERM and stdin EOF; start a child that also
                    ignores SIGTERM
  STUBBORN=child    only the child ignores SIGTERM
  CHILD_PID=<file>  where the child's pid is written
  FLOOD=<n>         stream n agent_message_chunk updates for every prompt
  PERMISSION=1      ask permission (kind edit) before answering a prompt
  CANCEL_DELAY=<s>  wait before honouring session/cancel
  NO_RESUME=1       offer session/load but not session/resume
  NO_IMAGE=1        don't take images in prompts
  AUTH=1            session/new fails: authentication required, unless
                    authenticate with fake-login came first
  AUTH_FAIL=1       authenticate fails
  FIRST_SESSION=<n> number the sessions it opens from n + 1 (sess-<n+1>)
  PERM_COMMAND=<c>  a permission request is for running command c (kind
                    execute, titled c)
  LEAVE_GROUP=1     move to its parent's process group, out of its own

session/list always has old-1 and sess-1, as an agent's store of sessions
would. session/load replays a question, an answer and a title.

and by the prompt's text:

  hang ...          runs until cancelled
  reply <text>      answers with <text>
  big <n>           answers with one message of n bytes
  think             thinks, then answers
  tools             runs a tool call, with a plan and usage, then answers
  perm <kind>       asks permission for a tool call of that kind first
  fail              the turn fails
"""

import json
import os
import signal
import subprocess
import sys
import time

env = os.environ.get

MODES = {
    "currentModeId": "default",
    "availableModes": [
        {"id": "default", "name": "Default", "description": "Asks before edits"},
        {"id": "plan", "name": "Plan", "description": "Plans, doesn't edit"},
    ],
}


def config(model="small"):
    return [
        {
            "id": "model",
            "name": "Model",
            "category": "model",
            "type": "select",
            "currentValue": model,
            "options": [{"value": "small", "name": "Small"}, {"value": "large", "name": "Large"}],
        }
    ]


def send(msg):
    sys.stdout.write(json.dumps(msg) + "\n")
    sys.stdout.flush()


def append(var, value):
    path = env(var)
    if path:
        with open(path, "a") as f:
            f.write(json.dumps(value) + "\n")


def result(mid, value):
    send({"jsonrpc": "2.0", "id": mid, "result": value})


def error(mid, code, message):
    send({"jsonrpc": "2.0", "id": mid, "error": {"code": code, "message": message}})


def update(session, value):
    send({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": session, "update": value}})


def say(session, text, kind="agent_message_chunk"):
    update(session, {"sessionUpdate": kind, "content": {"type": "text", "text": text}})


def end_turn(mid, reason="end_turn"):
    result(mid, {"stopReason": reason})


def opened(session):
    """What session/new and session/fork answer with."""
    return {"sessionId": session, "modes": MODES, "configOptions": config(model)}


if env("LEAVE_GROUP"):
    os.setpgid(0, os.getpgid(os.getppid()))

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

model = "small"
sessions = int(env("FIRST_SESSION") or 0)
authenticated = False
hanging = None  # id of a prompt that runs until cancelled
asking = {}  # permission request id -> (prompt id, session)

for line in sys.stdin:
    msg = json.loads(line)
    append("CALL_LOG", msg)
    method, mid = msg.get("method"), msg.get("id")
    params = msg.get("params") or {}
    sid = params.get("sessionId")
    if method == "initialize":
        session_caps = {"list": {}, "fork": {}, "close": {}}
        if not env("NO_RESUME"):
            session_caps["resume"] = {}
        caps = {
            "loadSession": True,
            "promptCapabilities": {"image": not env("NO_IMAGE")},
            "mcpCapabilities": {"http": True, "sse": False},
            "sessionCapabilities": session_caps,
        }
        methods = [{"id": "fake-login", "name": "Log in to the fake"}]
        info = {"name": "fake-agent", "version": "1.2.3"}
        result(mid, {"protocolVersion": 1, "agentCapabilities": caps, "authMethods": methods, "agentInfo": info})
    elif method == "authenticate":
        if env("AUTH_FAIL") or params.get("methodId") != "fake-login":
            error(mid, -32000, "Login failed")
        else:
            authenticated = True
            result(mid, {})
    elif method == "session/new":
        time.sleep(float(env("NEW_DELAY", "0")))
        if env("AUTH") and not authenticated:
            error(mid, -32000, "Authentication required")
            continue
        sessions += 1
        session = f"sess-{sessions}"
        result(mid, opened(session))
        update(session, {"sessionUpdate": "available_commands_update", "availableCommands": [{"name": "compact", "description": "Compact the conversation"}]})
        update(session, {"sessionUpdate": "session_info_update", "title": "Fake session"})
        if env("STALL"):
            time.sleep(100000)
    elif method in ("session/resume", "session/load"):
        if method == "session/load":
            update(sid, {"sessionUpdate": "user_message_chunk", "content": {"type": "text", "text": "old question"}})
            say(sid, "replayed history")
            update(sid, {"sessionUpdate": "session_info_update", "title": "Loaded session"})
        result(mid, {"modes": MODES, "configOptions": config(model)})
    elif method == "session/fork":
        sessions += 1
        result(mid, opened(f"sess-{sessions}"))
    elif method == "session/close":
        result(mid, {})
    elif method == "session/list":
        old = {"sessionId": "old-1", "cwd": params.get("cwd"), "title": "An old session", "updatedAt": "2026-10-01T10:00:00Z"}
        known = {"sessionId": "sess-1", "cwd": params.get("cwd")}
        result(mid, {"sessions": [old, known]})
    elif method == "session/set_mode":
        if params.get("modeId") in ("default", "plan"):
            result(mid, {})
            update(sid, {"sessionUpdate": "current_mode_update", "currentModeId": params["modeId"]})
        else:
            error(mid, -32602, f"no mode {params.get('modeId')}")
    elif method == "session/set_config_option":
        if params.get("configId") == "model" and params.get("value") in ("small", "large"):
            model = params["value"]
            result(mid, {"configOptions": config(model)})
        else:
            error(mid, -32602, f"bad option {params.get('configId')}={params.get('value')}")
    elif method == "session/prompt":
        text = "\n".join(b.get("text", "") for b in params["prompt"] if b.get("type") == "text")
        append("PROMPT_LOG", text)
        for _ in range(int(env("FLOOD", "0"))):
            say(sid, "y" * 500)
        words = text.split()
        first = words[0] if words else ""
        if first == "hang":
            hanging = mid
        elif first == "reply":
            say(sid, text[len("reply "):])
            end_turn(mid)
        elif first == "big":
            say(sid, "z" * int(words[1]))
            end_turn(mid)
        elif first == "think":
            say(sid, "pondering", "agent_thought_chunk")
            say(sid, "thought about it")
            end_turn(mid)
        elif first == "tools":
            plan = [{"content": "Run the tests", "status": "in_progress", "priority": "high"},
                    {"content": "Fix them", "status": "pending", "priority": "high"}]
            update(sid, {"sessionUpdate": "plan", "entries": plan})
            update(sid, {"sessionUpdate": "tool_call", "toolCallId": "t1", "title": "Run the tests", "kind": "execute", "status": "pending"})
            update(sid, {"sessionUpdate": "tool_call_update", "toolCallId": "t1", "status": "in_progress"})
            update(sid, {"sessionUpdate": "tool_call_update", "toolCallId": "t1", "status": "completed"})
            plan[0]["status"] = "completed"
            update(sid, {"sessionUpdate": "plan", "entries": plan})
            update(sid, {"sessionUpdate": "usage_update", "used": 12345, "size": 200000, "cost": {"amount": 0.42, "currency": "USD"}})
            say(sid, "did the tools")
            end_turn(mid)
        elif first == "fail":
            error(mid, -32603, "boom")
        elif first == "perm" or env("PERMISSION"):
            kind = words[1] if first == "perm" and len(words) > 1 else "edit"
            request = f"perm-{len(asking) + 1}"
            asking[request] = mid
            options = [
                {"optionId": "allow", "name": "Allow", "kind": "allow_once"},
                {"optionId": "reject", "name": "Reject", "kind": "reject_once"},
            ]
            tool = {
                "toolCallId": request,
                "title": "Edit src/lib.rs" if kind == "edit" else f"A {kind} tool",
                "kind": kind,
                "locations": [{"path": "src/lib.rs", "line": 2}],
                "rawInput": {"file_path": "src/lib.rs"},
                "content": [{"type": "diff", "path": "src/lib.rs", "oldText": "one\nold line\nthree\n", "newText": "one\nnew line\nthree\n"}],
            }
            if env("PERM_COMMAND"):
                command = env("PERM_COMMAND")
                tool = {"toolCallId": request, "title": command, "kind": "execute", "rawInput": {"command": command}}
            params = {"sessionId": sid, "toolCall": tool, "options": options}
            send({"jsonrpc": "2.0", "id": request, "method": "session/request_permission", "params": params})
        else:
            end_turn(mid)
    elif method == "session/cancel":
        time.sleep(float(env("CANCEL_DELAY", "0")))
        if hanging is not None:
            end_turn(hanging, "cancelled")
            hanging = None
    elif method is None and mid in asking:
        end_turn(asking.pop(mid))

if stubborn == "all":
    time.sleep(100000)
