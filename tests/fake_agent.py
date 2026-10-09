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
  FAULT=<kind>     crash (exit 86), hang (stop reading), or flood until cut off
  FAULT_SEED=<n>   reproducible checkpoint, default 0; turn emits 0..31 chunks
                    before faulting, setup chooses initialize or session/new
  FAULT_PHASE=<p>  turn (default) or setup (before the selected reply)
  FAULT_LOG=<file> append kind, seed, checkpoint and pid just before faulting
  FLOOD=<n>         stream n agent_message_chunk updates for every prompt
  NOISE=<n>         write n notifications brnr doesn't interpret (500 bytes
                    each) for every prompt
  PERMISSION=1      ask permission (kind edit) before answering a prompt
  CANCEL_DELAY=<s>  wait before honouring session/cancel
  NO_RESUME=1       offer session/load but not session/resume
  NO_HISTORY=1      session/load replays nothing
  NO_CLOSE=1        don't offer session/close
  NO_IMAGE=1        don't take images in prompts
  AUTH=1            session/new fails: authentication required, unless
                    authenticate with fake-login came first
  AUTH_FAIL=1       authenticate fails
  FIRST_SESSION=<n> number the sessions it opens from n + 1 (sess-<n+1>)
  PERM_COMMAND=<c>  a permission request is for running command c (kind
                    execute, titled c)
  LEAVE_GROUP=1     move to its parent's process group, out of its own
  NO_STEERING=1     don't advertise _session/steering, nor answer it
  STEER_ANSWER=<a>  a steer into a running turn is answered after that turn
                    ends: late (it takes up the steered text, then answers
                    injected), error (it ends the turn, then answers with an
                    error) or never (it ends the turn, and never answers)
  MODEL_ID=<id>     the model config option's id (model by default)
  MODEL_CATEGORY=<c>  its category (model by default; empty for none)
  MODE_OPTION=<id>  no modes, but a config option of category mode, id <id>
  LEGACY_MODELS=1   also offer the unstable models and session/set_model
  STDERR=<text>     write text on stderr as it starts
  EXIT=<code>       exit with code as it starts (after STDERR), reading
                    nothing
  MODE_GATE=<file>  wait for this file before answering session/set_mode
  QUIET_MODE=1      session/set_mode sends no current_mode_update: ACP answers
                    only the requester
  RAW_LOG=<file>    every line it receives is appended to file, byte for byte
                    (a line that isn't JSON is then skipped)
  SESSION_ID=<id>   the first session it opens has this id
  ALLOW_ONLY=1      a permission request offers only an allow option
  PROTOCOL_VERSION=<json>  the protocolVersion initialize answers with (1);
                    the missing key with `missing`

session/list always has old-1 and sess-1, as an agent's store of sessions
would. session/load replays a question, an answer and a title.

A client whose initialize advertises boolean config options
(clientCapabilities.session.configOptions.boolean) also gets a boolean option,
fast, which takes only type "boolean" and a JSON boolean.

_session/steering is answered as claude-agent-acp's steer() answers it:
injected while a turn runs (the turn goes on with the steered text, as it
would with a prompt's, and ends once that is answered), promptRequired when
none does and the request's _meta.steering.idleBehavior asks for it.

and by the prompt's text:

  hang ...          runs until cancelled (or steered)
  slow <s>          answers after s seconds, reading nothing meanwhile
  reply <text>      answers with <text>
  big <n>           answers with one message of n bytes
  long <n> [stderr] writes one line of n MiB of x, 1 MiB at a time, then
                    answers; on stdout the line ends in a newline, on stderr
                    it is left open
  many <n> [<size>] answers with n messages, each of its own (and size bytes
                    longer)
  think             thinks, then answers
  tools             runs a tool call, with a plan and usage, then answers
  settings [<m>]    reports its config options, with model m if given (none:
                    no options), then answers
  commands          its commands change: compact goes, review comes
  unknown           an update of a kind ACP's schema doesn't have (an
                    adapter's own) in the middle of a message, then answers
  perm <kind>       asks permission for a tool call of that kind first
  odd <how> [<n>]   asks permission in a line brnr once couldn't read: how is
                    surrogate (the title ends in half an emoji), deep (the
                    input nests n deep, 10000 by default) or garbled (the
                    line isn't JSON)
  fail              the turn fails
  verbatim          writes VERBATIM, then a line that isn't JSON, then answers
"""

import json
import os
import random
import signal
import subprocess
import sys
import time

env = os.environ.get

# An update as no JSON library writes it: spaces, key order, escapes (%s is
# the session).
VERBATIM = (
    '{ "params" : {"update":{"content":{"text":"caf\\u00e9 \\/ é \\ud83d\\ude00",'
    '"type":"text"},  "sessionUpdate":"agent_message_chunk"},"sessionId":"%s"},'
    '"method":"session/update" ,"jsonrpc":"2.0"}'
)

MODES = {
    "currentModeId": "default",
    "availableModes": [
        {"id": "default", "name": "Default", "description": "Asks before edits"},
        {"id": "plan", "name": "Plan", "description": "Plans, doesn't edit"},
    ],
}


MODEL_ID = env("MODEL_ID", "model")
MODE_ID = env("MODE_OPTION")


def config(model="small"):
    options = []
    if MODE_ID:
        choices = [{"value": m["id"], "name": m["name"]} for m in MODES["availableModes"]]
        options.append(
            {
                "id": MODE_ID,
                "name": "Mode",
                "category": "mode",
                "type": "select",
                "currentValue": mode,
                "options": choices,
            }
        )
    option = {
        "id": MODEL_ID,
        "name": "Model",
        "type": "select",
        "currentValue": model,
        "options": [{"value": "small", "name": "Small"}, {"value": "large", "name": "Large"}],
    }
    if env("MODEL_CATEGORY", "model"):
        option["category"] = env("MODEL_CATEGORY", "model")
    options.append(option)
    if fast is not None:
        options.append({"id": "fast", "name": "Fast", "type": "boolean", "currentValue": fast})
    return options


def settings(answer):
    """The modes (unless MODE_OPTION) and config options, in answer."""
    if not MODE_ID:
        answer["modes"] = MODES
    answer["configOptions"] = config(model)
    return answer


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
    send(
        {
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": {"sessionId": session, "update": value},
        }
    )


def say(session, text, kind="agent_message_chunk"):
    update(session, {"sessionUpdate": kind, "content": {"type": "text", "text": text}})


def end_turn(mid, reason="end_turn"):
    result(mid, {"stopReason": reason})


def opened(session):
    """What session/new and session/fork answer with."""
    answer = settings({"sessionId": session})
    if env("LEGACY_MODELS"):
        models = [{"modelId": "small", "name": "Small"}, {"modelId": "large", "name": "Large"}]
        answer["models"] = {"currentModelId": model, "availableModels": models}
    return answer


def text_of(prompt):
    return "\n".join(b.get("text", "") for b in prompt if b.get("type") == "text")


def fault(checkpoint, sid=None):
    """A seeded failure, with a marker so tests never race a wall-clock delay."""
    kind = env("FAULT")
    append(
        "FAULT_LOG",
        {"kind": kind, "seed": fault_seed, "checkpoint": checkpoint, "pid": os.getpid()},
    )
    if kind == "crash":
        os._exit(86)
    if kind == "hang":
        time.sleep(100000)
    if kind == "flood":
        try:
            while True:
                if sid:
                    say(sid, "f" * 4096)
                else:
                    send({"jsonrpc": "2.0", "method": "_fake/noise", "params": {"pad": "f" * 4096}})
                time.sleep(0.001)
        except BrokenPipeError:
            # Avoid a second flush of the broken pipe during interpreter exit.
            os._exit(0)


def run(mid, sid, text):
    """Takes up the text of prompt mid, or of a steer into its turn."""
    global hanging, model
    if env("FAULT") and env("FAULT_PHASE", "turn") == "turn":
        for step in range(fault_point):
            say(sid, f"checkpoint {step}")
        fault(fault_point, sid)
    for _ in range(int(env("FLOOD", "0"))):
        say(sid, "y" * 500)
    for _ in range(int(env("NOISE", "0"))):
        send({"jsonrpc": "2.0", "method": "_fake/noise", "params": {"pad": "n" * 500}})
    words = text.split()
    first = words[0] if words else ""
    if first == "hang":
        hanging = mid
    elif first == "slow":
        time.sleep(float(words[1]))
        end_turn(mid)
    elif first == "reply":
        say(sid, text[len("reply ") :])
        end_turn(mid)
    elif first == "big":
        say(sid, "z" * int(words[1]))
        end_turn(mid)
    elif first == "long":
        out = sys.stderr if words[2:] == ["stderr"] else sys.stdout
        for _ in range(int(words[1])):
            out.write("x" * (1 << 20))
            out.flush()
        if out is sys.stdout:
            out.write("\n")
        end_turn(mid)
    elif first == "many":
        pad = "m" * int(words[2]) if len(words) > 2 else ""
        for i in range(int(words[1])):
            update(
                sid,
                {
                    "sessionUpdate": "agent_message_chunk",
                    "messageId": f"msg-{i}",
                    "content": {"type": "text", "text": f"message {i}{pad}"},
                },
            )
        end_turn(mid)
    elif first == "think":
        say(sid, "pondering", "agent_thought_chunk")
        say(sid, "thought about it")
        end_turn(mid)
    elif first == "tools":
        plan = [
            {"content": "Run the tests", "status": "in_progress", "priority": "high"},
            {"content": "Fix them", "status": "pending", "priority": "high"},
        ]
        update(sid, {"sessionUpdate": "plan", "entries": plan})
        update(
            sid,
            {
                "sessionUpdate": "tool_call",
                "toolCallId": "t1",
                "title": "Run the tests",
                "kind": "execute",
                "status": "pending",
            },
        )
        update(
            sid, {"sessionUpdate": "tool_call_update", "toolCallId": "t1", "status": "in_progress"}
        )
        update(
            sid, {"sessionUpdate": "tool_call_update", "toolCallId": "t1", "status": "completed"}
        )
        plan[0]["status"] = "completed"
        update(sid, {"sessionUpdate": "plan", "entries": plan})
        update(
            sid,
            {
                "sessionUpdate": "usage_update",
                "used": 12345,
                "size": 200000,
                "cost": {"amount": 0.42, "currency": "USD"},
            },
        )
        say(sid, "did the tools")
        end_turn(mid)
    elif first == "settings":
        if words[1:] and words[1] != "none":
            model = words[1]
        options = [] if words[1:] == ["none"] else config(model)
        update(sid, {"sessionUpdate": "config_option_update", "configOptions": options})
        end_turn(mid)
    elif first == "commands":
        update(
            sid,
            {
                "sessionUpdate": "available_commands_update",
                "availableCommands": [{"name": "review", "description": "Review the changes"}],
            },
        )
        end_turn(mid)
    elif first == "unknown":
        chunk = {"sessionUpdate": "agent_message_chunk", "messageId": "u1"}
        update(sid, {**chunk, "content": {"type": "text", "text": "one "}})
        spawned = {
            "sessionUpdate": "subagent_spawned",
            "subagentSessionId": "sub-1",
            "name": "helper",
            "task": "look",
            "capabilities": {},
        }
        update(sid, spawned)
        update(sid, {**chunk, "content": {"type": "text", "text": "message"}})
        end_turn(mid)
    elif first == "fail":
        error(mid, -32603, "boom")
    elif first == "verbatim":
        sys.stdout.write(VERBATIM % sid + "\n" + "not json, from the agent\n")
        sys.stdout.flush()
        end_turn(mid)
    elif first == "odd":
        how = words[1] if len(words) > 1 else "surrogate"
        request = f"perm-{len(asking) + 1}"
        asking[request] = mid
        title = "Edit " + chr(0xD83D) if how == "surrogate" else "Edit src/lib.rs"
        tool = {"toolCallId": request, "title": title, "kind": "edit", "rawInput": "INPUT"}
        options = [{"optionId": "allow", "name": "Allow", "kind": "allow_once"}]
        params = {"sessionId": sid, "toolCall": tool, "options": options}
        line = json.dumps(
            {
                "jsonrpc": "2.0",
                "id": request,
                "method": "session/request_permission",
                "params": params,
            }
        )
        if how == "deep":
            n = int(words[2]) if len(words) > 2 else 10000
            line = line.replace('"INPUT"', "[" * n + "]" * n)
        elif how == "garbled":
            line = line[:-1]
        sys.stdout.write(line + "\n")
        sys.stdout.flush()
    elif first == "perm" or env("PERMISSION"):
        kind = words[1] if first == "perm" and len(words) > 1 else "edit"
        request = f"perm-{len(asking) + 1}"
        asking[request] = mid
        options = [
            {"optionId": "allow", "name": "Allow", "kind": "allow_once"},
            {"optionId": "reject", "name": "Reject", "kind": "reject_once"},
        ]
        if env("ALLOW_ONLY"):
            options = options[:1]
        tool = {
            "toolCallId": request,
            "title": "Edit src/lib.rs" if kind == "edit" else f"A {kind} tool",
            "kind": kind,
            "locations": [{"path": "src/lib.rs", "line": 2}],
            "rawInput": {"file_path": "src/lib.rs"},
            "content": [
                {
                    "type": "diff",
                    "path": "src/lib.rs",
                    "oldText": "one\nold line\nthree\n",
                    "newText": "one\nnew line\nthree\n",
                }
            ],
        }
        if env("PERM_COMMAND"):
            command = env("PERM_COMMAND")
            tool = {
                "toolCallId": request,
                "title": command,
                "kind": "execute",
                "rawInput": {"command": command},
            }
        params = {"sessionId": sid, "toolCall": tool, "options": options}
        send(
            {
                "jsonrpc": "2.0",
                "id": request,
                "method": "session/request_permission",
                "params": params,
            }
        )
    else:
        end_turn(mid)


if env("STDERR"):
    sys.stderr.write(env("STDERR") + "\n")
    sys.stderr.flush()
if env("EXIT"):
    sys.exit(int(env("EXIT")))

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

fault_seed = int(env("FAULT_SEED", "0"))
fault_random = random.Random(fault_seed)
fault_point = fault_random.randrange(32)
fault_setup = fault_random.choice(("initialize", "session/new"))
if env("FAULT") not in (None, "crash", "hang", "flood"):
    raise ValueError("FAULT must be crash, hang or flood")

model = "small"
mode = "default"
fast = None  # the boolean option's value, once the client says it takes booleans
sessions = int(env("FIRST_SESSION") or 0)
authenticated = False
hanging = None  # id of a prompt that runs until cancelled
asking = {}  # permission request id -> (prompt id, session)

for line in sys.stdin.buffer:
    if env("RAW_LOG"):
        with open(env("RAW_LOG"), "ab") as f:
            f.write(line)
        try:
            msg = json.loads(line)
        except ValueError:
            continue
    else:
        msg = json.loads(line)
    append("CALL_LOG", msg)
    method, mid = msg.get("method"), msg.get("id")
    params = msg.get("params") or {}
    sid = params.get("sessionId")
    if env("FAULT") and env("FAULT_PHASE") == "setup" and method == fault_setup:
        fault(method)
    if method == "initialize":
        client_session = (params.get("clientCapabilities") or {}).get("session") or {}
        if (client_session.get("configOptions") or {}).get("boolean") is not None:
            fast = False
        session_caps = {"list": {}, "fork": {}, "close": {}}
        if not env("NO_RESUME"):
            session_caps["resume"] = {}
        if env("NO_CLOSE"):
            del session_caps["close"]
        caps = {
            "loadSession": True,
            "promptCapabilities": {"image": not env("NO_IMAGE")},
            "mcpCapabilities": {"http": True, "sse": False},
            "sessionCapabilities": session_caps,
        }
        methods = [{"id": "fake-login", "name": "Log in to the fake"}]
        info = {"name": "fake-agent", "version": "1.2.3"}
        answer = {
            "protocolVersion": 1,
            "agentCapabilities": caps,
            "authMethods": methods,
            "agentInfo": info,
        }
        if env("PROTOCOL_VERSION") == "missing":
            del answer["protocolVersion"]
        elif env("PROTOCOL_VERSION"):
            answer["protocolVersion"] = json.loads(env("PROTOCOL_VERSION"))
        if not env("NO_STEERING"):
            answer["_meta"] = {"steering": {"supported": True}}
        result(mid, answer)
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
        if env("SESSION_ID") and sessions == 1:
            session = env("SESSION_ID")
        result(mid, opened(session))
        update(
            session,
            {
                "sessionUpdate": "available_commands_update",
                "availableCommands": [
                    {"name": "compact", "description": "Compact the conversation"}
                ],
            },
        )
        update(session, {"sessionUpdate": "session_info_update", "title": "Fake session"})
        if env("STALL"):
            time.sleep(100000)
    elif method in ("session/resume", "session/load"):
        if method == "session/load" and not env("NO_HISTORY"):
            update(
                sid,
                {
                    "sessionUpdate": "user_message_chunk",
                    "content": {"type": "text", "text": "old question"},
                },
            )
            say(sid, "replayed history")
            update(sid, {"sessionUpdate": "session_info_update", "title": "Loaded session"})
        result(mid, settings({}))
    elif method == "session/fork":
        sessions += 1
        result(mid, opened(f"sess-{sessions}"))
    elif method == "session/close":
        result(mid, {})
    elif method == "session/list":
        old = {
            "sessionId": "old-1",
            "cwd": params.get("cwd"),
            "title": "An old session",
            "updatedAt": "2026-10-01T10:00:00Z",
        }
        known = {"sessionId": "sess-1", "cwd": params.get("cwd")}
        result(mid, {"sessions": [old, known]})
    elif method == "session/set_mode":
        while env("MODE_GATE") and not os.path.exists(env("MODE_GATE")):
            time.sleep(0.01)
        if params.get("modeId") in ("default", "plan"):
            result(mid, {})
            if not env("QUIET_MODE"):
                update(
                    sid, {"sessionUpdate": "current_mode_update", "currentModeId": params["modeId"]}
                )
        else:
            error(mid, -32602, f"no mode {params.get('modeId')}")
    elif method == "session/set_model" and env("LEGACY_MODELS"):
        model = params["modelId"]
        result(mid, {})
    elif method == "session/set_config_option":
        if params.get("configId") == MODEL_ID and params.get("value") in ("small", "large"):
            model = params["value"]
            result(mid, {"configOptions": config(model)})
        elif (
            MODE_ID
            and params.get("configId") == MODE_ID
            and params.get("value") in ("default", "plan")
        ):
            mode = params["value"]
            result(mid, {"configOptions": config(model)})
        elif (
            fast is not None
            and params.get("configId") == "fast"
            and params.get("type") == "boolean"
            and isinstance(params.get("value"), bool)
        ):
            fast = params["value"]
            result(mid, {"configOptions": config(model)})
        else:
            error(mid, -32602, f"bad option {params.get('configId')}={params.get('value')}")
    elif method == "session/prompt":
        text = text_of(params["prompt"])
        append("PROMPT_LOG", text)
        run(mid, sid, text)
    elif method == "_session/steering" and not env("NO_STEERING"):
        idle = ((params.get("_meta") or {}).get("steering") or {}).get("idleBehavior")
        if idle not in (None, "promptRequired"):
            error(mid, -32602, "unsupported steering idleBehavior")
        elif hanging is not None or asking:
            late = env("STEER_ANSWER")
            if not late:
                result(mid, {"outcome": "injected"})
            if hanging is not None:
                turn, hanging = hanging, None
                if late in ("error", "never"):
                    end_turn(turn)
                else:
                    run(turn, sid, text_of(params["prompt"]))
            if late == "late":
                result(mid, {"outcome": "injected"})
            elif late == "error":
                error(mid, -32603, "steering failed")
        elif idle == "promptRequired":
            result(mid, {"outcome": "promptRequired", "reason": "noRunningTurn"})
        else:
            result(mid, {"outcome": "startedNewTurn"})
    elif method == "session/cancel":
        time.sleep(float(env("CANCEL_DELAY", "0")))
        if hanging is not None:
            end_turn(hanging, "cancelled")
            hanging = None
    elif method is None and mid in asking:
        end_turn(asking.pop(mid))
    elif method is not None and mid is not None:
        error(mid, -32601, f"Method not found: {method}")

if stubborn == "all":
    time.sleep(100000)
