#!/usr/bin/env python3
"""A minimal ACP agent for the integration tests.

Every prompt it receives is appended to $PROMPT_LOG as one JSON string per
line, and every message it receives to $CALL_LOG as JSON. Behaviour is chosen
with environment variables:

  NEW_DELAY=<s>     wait before answering session/new
  NEW_GATE=<file>   answer session/new once this file exists, reading on
                    meanwhile
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
  RESUME_FAIL=1     session/resume answers with an error
  NO_LOAD=1         don't offer session/load
  NO_HISTORY=1      session/load replays nothing
  FULL_HISTORY=1    session/load also replays a tool call it never finished,
                    and the session's commands
  LOAD_GATE=<file>  answer session/load, once it has replayed, when this file
                    exists, reading nothing meanwhile
  LOAD_ENDS_TURN=1  session/load while a turn hangs ends it, cancelled,
                    before it replays
  REUSED_TOOL=1     session/load's history also finishes tool call t2, then
                    starts another with its id
  LOAD_LATER=<file>  as LOAD_GATE, but reading on meanwhile
  LOAD_FAIL=1       the first session/load, once it has replayed, answers
                    with an error
  LOAD_REFUSED=1    the first session/load answers with an error at once,
                    replaying nothing
  LATE_HISTORY=1    with LOAD_LATER, a load replays one more agent message,
                    late history, once the file exists, before its answer
  NO_CLOSE=1        don't offer session/close
  CLOSE_GATE=<file> wait for this file before answering session/close
  NO_LIST=1         don't offer session/list
  LIST_TITLE=<t>    session/list gives sess-1 this title (none by default)
  LIST_TIMES=<json> session/list's first page also has these sessions, an
                    object of session id to updatedAt
  NO_DELETE=1       don't offer session/delete
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
                    error), never (it ends the turn, and never answers) or
                    gate (it ends the turn, then answers injected once the
                    file $STEER_GATE exists, reading on meanwhile)
  MODEL_ID=<id>     the model config option's id (model by default)
  MODEL_CATEGORY=<c>  its category (model by default; empty for none)
  MODE_OPTION=<id>  no modes, but a config option of category mode, id <id>
  BOTH_MODES=1      with MODE_OPTION, the modes too (session/set_mode)
  THOUGHT_OPTION=<id>  a config option of category thought_level, id <id>
                    (low or high)
  LEGACY_MODELS=1   also offer the unstable models and session/set_model
  STDERR=<text>     write text on stderr as it starts
  EXIT=<code>       exit with code as it starts (after STDERR), reading
                    nothing
  MODE_GATE=<file>  wait for this file before answering session/set_mode
  QUIET_MODE=1      session/set_mode sends no current_mode_update: ACP answers
                    only the requester
  RAW_LOG=<file>    every line it receives is appended to file, byte for byte
                    (a line that isn't JSON is then skipped)
  SIGCHLD_LOG=<file>  where it writes how it found SIGCHLD as it starts:
                    ignored or default
  SESSION_ID=<id>   the first session it opens has this id
  ALLOW_ONLY=1      a permission request offers only an allow option
  PERM_OPTIONS=<json>  a permission request offers these options (a JSON
                    array of ACP permission options) instead
  PERM_PAD=<n>      a permission request's tool call input has n bytes more
  PROTOCOL_VERSION=<json>  the protocolVersion initialize answers with (1);
                    the missing key with `missing`
  PROMPT_USAGE=<json>  every session/prompt answer has this text as its
                    result's usage, as it is (unstable ACP's token counts)
  ELICIT_AUTH=1     authenticate first asks for a URL to be opened, an
                    elicitation scoped to the authenticate request (no
                    session), and answers once that is answered: {} for an
                    accept, an error otherwise
  GIVE_UP=<file>    once this file exists, sends $/cancel_request for each
                    permission request and elicitation it has open, then
                    deletes the file, reading on meanwhile; it still takes
                    their answers as it would have
  ASK_AND_END=1     a turn that asks permission or for input ends at once,
                    its requests left open; their answers end nothing

An elicitation is asked only of a client whose initialize advertised its mode
(clientCapabilities.elicitation.form or .url, an object): otherwise the turn
says it wasn't asked. The answer is said in the session ("elicited" and the
response as JSON) and ends the turn; an accepted URL elicitation is first
completed (elicitation/complete).

$/cancel_request for the prompt that hangs answers it with an error, -32800.

session/list always has old-1 and sess-1, as an agent's store of sessions
would, a page each. session/load replays a question, an answer and a title. session/delete
deletes any session but one whose id starts with gone, which it doesn't have
(-32002), or stuck, which it fails to delete (-32603). session/prompt serves
any session but one whose id starts with nope, which it doesn't have: it
answers -32002, Session not found, saying nothing of it.

A client whose initialize advertises boolean config options
(clientCapabilities.session.configOptions.boolean) also gets a boolean option,
fast, which takes only type "boolean" and a JSON boolean.

_session/steering is answered as claude-agent-acp's steer() answers it:
injected while a turn runs (the turn goes on with the steered text, as it
would with a prompt's, and ends once that is answered), promptRequired when
none does and the request's _meta.steering.idleBehavior asks for it.

and by the prompt's text:

  hang ...          runs until cancelled (or steered)
  hang tool [twice] runs a tool call (t2) until cancelled; twice: its
                    tool_call comes again, same id, with a new title and status
  slow <s>          answers after s seconds, reading nothing meanwhile
  reply <text>      answers with <text>
  big <n> [think]   answers with one message of n bytes; think: a thought
                    of n bytes first
  long <n> [stderr] writes one line of n MiB of x, 1 MiB at a time, then
                    answers; on stdout the line ends in a newline, on stderr
                    it is left open
  stderr <n> [<t>]  writes n bytes of x, then t, then a newline on stderr,
                    then answers
  many <n> [<size>] answers with n messages, each of its own (and size bytes
                    longer)
  think             thinks, then answers
  tools             runs a tool call, with a plan and usage, then answers
  settings [<m>]    reports its config options, with model m if given (none:
                    no options), then answers
  commands          its commands change: compact goes, review comes
  unknown           an update of a kind ACP's schema doesn't have (an
                    adapter's own) in the middle of a message, then answers
  perm <kind> [<n>] asks permission for n tool calls of that kind (1) at once
                    first, and answers once each is answered
  odd <how> [<n>]   asks permission in a line brnr once couldn't read: how is
                    surrogate (the title ends in half an emoji), deep (the
                    input nests so that the line is n deep, 129 by default:
                    just past brnr's limit) or garbled (the line isn't JSON)
  fail [tool]       the turn fails; with tool, a tool call (t2) running
  elicit <how>      asks for input (elicitation/create), how being form (a
                    form of every field type, scoped to the session), tool
                    (the same, scoped to tool call t1), url (a URL to open,
                    elicitation id el-<n>), other (a mode _other ACP
                    doesn't have) or deep [<n>] (a form whose message nests
                    so that the line is n deep, 129 by default)
  verbatim          writes VERBATIM, then a line that isn't JSON, then answers
  client            uses its client's file system, then its terminal, as far
                    as initialize offered them (clientCapabilities
                    fs.readTextFile, terminal): asks fs/read_text_file of
                    /work/notes.txt, then terminal/create of echo, each once
                    the last is answered; says what each answer was ("read:
                    <content>", "terminal: <terminalId>", "<what> failed:
                    <message>", one line each), or "no fs", "no terminal" for
                    one not offered, and answers the prompt once done
  client anyway     the same, but asks for both whatever initialize offered
"""

import json
import os
import random
import signal
import subprocess
import sys
import threading
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
THOUGHT_ID = env("THOUGHT_OPTION")


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
    if THOUGHT_ID:
        levels = [{"value": "low", "name": "Low"}, {"value": "high", "name": "High"}]
        options.append(
            {
                "id": THOUGHT_ID,
                "name": "Effort",
                "category": "thought_level",
                "type": "select",
                "currentValue": effort,
                "options": levels,
            }
        )
    if fast is not None:
        options.append({"id": "fast", "name": "Fast", "type": "boolean", "currentValue": fast})
    return options


def settings(answer):
    """The modes (unless MODE_OPTION) and config options, in answer."""
    if not MODE_ID or env("BOTH_MODES"):
        answer["modes"] = MODES
    answer["configOptions"] = config(model)
    return answer


# A late answer to session/new (NEW_GATE), session/load (LOAD_LATER) or a
# steer (STEER_GATE) is sent from a thread of its own.
sending = threading.Lock()


def send(msg):
    with sending:
        sys.stdout.write(json.dumps(msg) + "\n")
        sys.stdout.flush()


# GIVE_UP's thread cancels each request open: a request is put in asking or
# eliciting and sent, or taken out, under this lock, so none opens or goes
# between its look and its gate going. Taken before sending, never after.
open_requests = threading.Lock()


def open_request(table, request, value, msg):
    """Puts request in table (asking or eliciting) and sends msg, a message
    or a line, in one step."""
    with open_requests:
        table[request] = value
        if isinstance(msg, str):
            with sending:
                sys.stdout.write(msg + "\n")
                sys.stdout.flush()
        else:
            send(msg)


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
    usage = env("PROMPT_USAGE")
    if usage is None:
        result(mid, {"stopReason": reason})
        return
    answer = {"jsonrpc": "2.0", "id": mid, "result": {"stopReason": reason}}
    line = json.dumps(answer)[:-2] + ', "usage": ' + usage + "}}"
    with sending:
        sys.stdout.write(line + "\n")
        sys.stdout.flush()


def opened(session):
    """What session/new and session/fork answer with."""
    answer = settings({"sessionId": session})
    if env("LEGACY_MODELS"):
        models = [{"modelId": "small", "name": "Small"}, {"modelId": "large", "name": "Large"}]
        answer["models"] = {"currentModelId": model, "availableModels": models}
    return answer


def new_opened(mid, session):
    """Answers session/new, then tells of the session's commands and title."""
    result(mid, opened(session))
    update(
        session,
        {
            "sessionUpdate": "available_commands_update",
            "availableCommands": [{"name": "compact", "description": "Compact the conversation"}],
        },
    )
    update(session, {"sessionUpdate": "session_info_update", "title": "Fake session"})


loads_failed = 0


def load_answer(mid):
    global loads_failed
    if env("LOAD_FAIL") and not loads_failed:
        loads_failed += 1
        error(mid, -32603, "Can't load the session")
    else:
        result(mid, settings({}))


def load_later(gate, mid, session):
    while not os.path.exists(gate):
        time.sleep(0.01)
    if env("LATE_HISTORY"):
        say(session, "late history")
    load_answer(mid)


def new_later(gate, mid, session):
    while not os.path.exists(gate):
        time.sleep(0.05)
    new_opened(mid, session)


def steer_later(gate, mid):
    while not os.path.exists(gate):
        time.sleep(0.05)
    result(mid, {"outcome": "injected"})


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
        if words[1:2] == ["tool"]:
            running_tool(sid, words[2:3] == ["twice"])
        hanging = mid
    elif first == "slow":
        time.sleep(float(words[1]))
        end_turn(mid)
    elif first == "reply":
        say(sid, text[len("reply ") :])
        end_turn(mid)
    elif first == "big":
        if words[2:] == ["think"]:
            say(sid, "t" * int(words[1]), "agent_thought_chunk")
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
    elif first == "stderr":
        tail = words[2].encode() if len(words) > 2 else b""
        sys.stderr.buffer.write(b"x" * int(words[1]) + tail + b"\n")
        sys.stderr.buffer.flush()
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
        if words[1:2] == ["tool"]:
            running_tool(sid)
        error(mid, -32603, "boom")
    elif first == "verbatim":
        sys.stdout.write(VERBATIM % sid + "\n" + "not json, from the agent\n")
        sys.stdout.flush()
        end_turn(mid)
    elif first == "client":
        use_client(mid, sid, ["fs", "terminal"], words[1:2] == ["anyway"])
    elif first == "odd":
        how = words[1] if len(words) > 1 else "surrogate"
        request = f"perm-{len(asking) + 1}"
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
            # The line, its params and the tool call are 3 levels.
            n = (int(words[2]) if len(words) > 2 else 129) - 3
            line = line.replace('"INPUT"', "[" * n + "]" * n)
        elif how == "garbled":
            line = line[:-1]
        open_request(asking, request, None if env("ASK_AND_END") else mid, line)
        if env("ASK_AND_END"):
            end_turn(mid)
    elif first == "elicit":
        elicit(mid, sid, words[1] if len(words) > 1 else "form", words[2:3])
    elif first == "perm" or env("PERMISSION"):
        kind = words[1] if first == "perm" and len(words) > 1 else "edit"
        n = int(words[2]) if first == "perm" and len(words) > 2 else 1
        for _ in range(n):
            ask(mid, sid, kind)
        if env("ASK_AND_END"):
            end_turn(mid)
    else:
        end_turn(mid)


def use_client(mid, sid, steps, anyway=False):
    """Asks the client for the next of steps (fs, terminal) its initialize
    offered (anyway: whatever it offered), saying "no <step>" for each it
    didn't; once none is left, ends prompt mid's turn."""
    global client_requests
    while steps:
        what, steps = steps[0], steps[1:]
        if what == "fs" and (anyway or (client_caps.get("fs") or {}).get("readTextFile")):
            method, params = "fs/read_text_file", {"sessionId": sid, "path": "/work/notes.txt"}
        elif what == "terminal" and (anyway or client_caps.get("terminal")):
            method = "terminal/create"
            params = {"sessionId": sid, "command": "echo", "args": ["hi"]}
        else:
            tell(mid, sid, f"no {what}")
            continue
        client_requests += 1
        request = f"{what}-{client_requests}"
        using[request] = (mid, sid, what, steps, anyway)
        send({"jsonrpc": "2.0", "id": request, "method": method, "params": params})
        return
    end_turn(mid)


def tell(mid, sid, text):
    """Says text in prompt mid's turn, on a line after what it said before."""
    say(sid, "\n" + text if mid in told else text)
    told.add(mid)


def client_answered(msg):
    """The client's answer to one of use_client's requests."""
    turn, sid, what, steps, anyway = using.pop(msg["id"])
    if "error" in msg:
        tell(turn, sid, f"{what} failed: {msg['error'].get('message')}")
    elif what == "fs":
        tell(turn, sid, f"read: {msg['result']['content']}")
    else:
        tell(turn, sid, f"terminal: {msg['result']['terminalId']}")
    use_client(turn, sid, steps, anyway)


# The form `elicit form` asks for: every type of field, with bounds and
# choices.
FORM = {
    "type": "object",
    "title": "About you",
    "properties": {
        "name": {"type": "string", "title": "Name", "minLength": 1, "maxLength": 5},
        "count": {"type": "integer", "minimum": 1, "maximum": 10},
        "ratio": {"type": "number", "maximum": 1.5},
        "ok": {"type": "boolean", "description": "Go ahead?"},
        "color": {"type": "string", "enum": ["red", "green"]},
        "size": {
            "type": "string",
            "oneOf": [{"const": "s", "title": "Small"}, {"const": "l", "title": "Large"}],
        },
        "tags": {
            "type": "array",
            "items": {"type": "string", "enum": ["a", "b", "c"]},
            "maxItems": 2,
        },
    },
    "required": ["name", "color"],
}


def elicit(mid, sid, how, depth=()):
    """Asks for input in prompt mid's turn, if the client advertised the mode;
    deep: with its message nested so that the line is depth deep (129 if
    none)."""
    mode = "url" if how == "url" else "_other" if how == "other" else "form"
    if mode != "_other" and elicit_modes.get(mode) is None:
        say(sid, f"not asked: {mode} not advertised")
        end_turn(mid)
        return
    request = f"elicit-{len(eliciting) + 1}"
    params = {"sessionId": sid, "mode": mode, "message": "Tell me about you"}
    if how == "tool":
        params["toolCallId"] = "t1"
    if mode == "form":
        params["requestedSchema"] = FORM
    elif mode == "url":
        params["message"] = "Log in to the fake"
        params["elicitationId"] = f"el-{len(eliciting) + 1}"
        params["url"] = "https://login.example/a?b=1"
    msg = {"jsonrpc": "2.0", "id": request, "method": "elicitation/create", "params": params}
    if how == "deep":
        # Too deep for json.dumps: put in as text. The line and its params
        # are 2 levels.
        n = (int(depth[0]) if depth else 129) - 2
        params["message"] = "MESSAGE"
        msg = json.dumps(msg).replace('"MESSAGE"', "[" * n + "]" * n)
    turn = None if env("ASK_AND_END") else mid
    open_request(eliciting, request, (turn, sid, params.get("elicitationId")), msg)
    if env("ASK_AND_END"):
        end_turn(mid)


def give_up(gate):
    """Each time gate exists: $/cancel_request for every request it has
    open, then the gate goes."""
    while True:
        while not os.path.exists(gate):
            time.sleep(0.05)
        with open_requests:
            for request in list(asking) + list(eliciting):
                cancel = {"requestId": request}
                send({"jsonrpc": "2.0", "method": "$/cancel_request", "params": cancel})
            os.remove(gate)


def elicited(mid, msg):
    """The client answered elicitation request mid."""
    global authenticated
    with open_requests:
        turn, sid, url = eliciting.pop(mid)
    answer = msg.get("result") or msg.get("error")
    if url and (msg.get("result") or {}).get("action") == "accept":
        params = {"elicitationId": url}
        send({"jsonrpc": "2.0", "method": "elicitation/complete", "params": params})
    if sid is None:
        # An authenticate's, outside any session.
        if (msg.get("result") or {}).get("action") == "accept":
            authenticated = True
            result(turn, {})
        else:
            error(turn, -32000, "Login not completed")
        return
    say(sid, "elicited " + json.dumps(answer, sort_keys=True))
    if turn is not None:
        end_turn(turn)


def running_tool(sid, twice=False):
    """Starts tool call t2, which never finishes; twice: its tool_call again."""
    tool = {"sessionUpdate": "tool_call", "toolCallId": "t2", "kind": "execute"}
    if twice:
        # The second leaves out the first's locations, which stay.
        locations = [{"path": "/work/build.sh"}]
        update(
            sid, {**tool, "title": "Start the build", "status": "pending", "locations": locations}
        )
    update(sid, {**tool, "title": "Run the build", "status": "in_progress"})


def ask(mid, sid, kind):
    """Asks permission for a tool call of kind, in prompt mid's turn."""
    request = f"perm-{len(asking) + 1}"
    options = [
        {"optionId": "allow", "name": "Allow", "kind": "allow_once"},
        {"optionId": "reject", "name": "Reject", "kind": "reject_once"},
    ]
    if env("ALLOW_ONLY"):
        options = options[:1]
    if env("PERM_OPTIONS"):
        options = json.loads(env("PERM_OPTIONS"))
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
    if env("PERM_PAD"):
        tool["rawInput"]["pad"] = "p" * int(env("PERM_PAD"))
    params = {"sessionId": sid, "toolCall": tool, "options": options}
    msg = {
        "jsonrpc": "2.0",
        "id": request,
        "method": "session/request_permission",
        "params": params,
    }
    open_request(asking, request, None if env("ASK_AND_END") else mid, msg)


if env("SIGCHLD_LOG"):
    with open(env("SIGCHLD_LOG"), "w") as f:
        ignored = signal.getsignal(signal.SIGCHLD) == signal.SIG_IGN
        f.write("ignored" if ignored else "default")
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
effort = "low"
fast = None  # the boolean option's value, once the client says it takes booleans
sessions = int(env("FIRST_SESSION") or 0)
authenticated = False
hanging = None  # id of a prompt that runs until cancelled
asking = {}  # permission request id -> (prompt id, session)
client_caps = {}  # what initialize's clientCapabilities offered
client_requests = 0  # fs and terminal requests made of the client
using = {}  # such a request's id -> (prompt id, session, what, steps left, anyway)
told = set()  # the prompts whose turns use_client has said something in
eliciting = {}  # elicitation request id -> (prompt id, session, elicitation id)
elicit_modes = {}  # the client's clientCapabilities.elicitation
if env("GIVE_UP"):
    threading.Thread(target=give_up, args=(env("GIVE_UP"),), daemon=True).start()

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
        client_caps = params.get("clientCapabilities") or {}
        client_session = client_caps.get("session") or {}
        elicit_modes = {
            k: v for k, v in (client_caps.get("elicitation") or {}).items() if isinstance(v, dict)
        }
        if (client_session.get("configOptions") or {}).get("boolean") is not None:
            fast = False
        session_caps = {"list": {}, "fork": {}, "close": {}}
        if not env("NO_RESUME"):
            session_caps["resume"] = {}
        if env("NO_CLOSE"):
            del session_caps["close"]
        if env("NO_LIST"):
            del session_caps["list"]
        if not env("NO_DELETE"):
            session_caps["delete"] = {}
        caps = {
            "loadSession": not env("NO_LOAD"),
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
        if env("ELICIT_AUTH"):
            request = f"elicit-{len(eliciting) + 1}"
            ask_url = {
                "mode": "url",
                "requestId": mid,
                "message": "Log in to the fake",
                "elicitationId": "el-auth",
                "url": "https://login.example/auth",
            }
            create = {"id": request, "method": "elicitation/create", "params": ask_url}
            open_request(eliciting, request, (mid, None, "el-auth"), {"jsonrpc": "2.0", **create})
        elif env("AUTH_FAIL") or params.get("methodId") != "fake-login":
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
        gate = env("NEW_GATE")
        if gate and not os.path.exists(gate):
            threading.Thread(target=new_later, args=(gate, mid, session), daemon=True).start()
            continue
        new_opened(mid, session)
        if env("STALL"):
            time.sleep(100000)
    elif method in ("session/resume", "session/load"):
        if method == "session/load" and env("LOAD_ENDS_TURN") and hanging is not None:
            end_turn(hanging, "cancelled")
            hanging = None
        if method == "session/load" and env("LOAD_REFUSED") and not loads_failed:
            loads_failed += 1
            error(mid, -32603, "Can't load the session")
            continue
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
            if env("REUSED_TOOL"):
                tool = {"toolCallId": "t2", "title": "Run the build", "kind": "execute"}
                update(sid, {"sessionUpdate": "tool_call", **tool, "status": "completed"})
                update(sid, {"sessionUpdate": "tool_call", **tool, "status": "in_progress"})
            if env("FULL_HISTORY"):
                tool = {"toolCallId": "t0", "title": "Old tool", "kind": "read"}
                update(sid, {"sessionUpdate": "tool_call", **tool, "status": "in_progress"})
                commands = [{"name": "replayed", "description": "A command from history"}]
                update(
                    sid,
                    {"sessionUpdate": "available_commands_update", "availableCommands": commands},
                )
        while (
            method == "session/load" and env("LOAD_GATE") and not os.path.exists(env("LOAD_GATE"))
        ):
            time.sleep(0.01)
        if method == "session/resume" and env("RESUME_FAIL"):
            error(mid, -32603, "Can't resume the session")
        elif method == "session/resume":
            result(mid, settings({}))
        elif env("LOAD_LATER"):
            later = (env("LOAD_LATER"), mid, sid)
            threading.Thread(target=load_later, args=later, daemon=True).start()
        else:
            load_answer(mid)
    elif method == "session/fork":
        sessions += 1
        result(mid, opened(f"sess-{sessions}"))
    elif method == "session/close":
        while env("CLOSE_GATE") and not os.path.exists(env("CLOSE_GATE")):
            time.sleep(0.01)
        result(mid, {})
    elif method == "session/delete" and not env("NO_DELETE"):
        if sid.startswith("gone"):
            error(mid, -32002, f"Session not found: {sid}")
        elif sid.startswith("stuck"):
            error(mid, -32603, f"Can't delete {sid}")
        else:
            result(mid, {})
    elif method == "session/list":
        old = {
            "sessionId": "old-1",
            "cwd": params.get("cwd"),
            "title": "An old session",
            "updatedAt": "2026-10-01T10:00:00Z",
        }
        known = {"sessionId": "sess-1", "cwd": params.get("cwd")}
        if env("LIST_TITLE"):
            known["title"] = env("LIST_TITLE")
        if params.get("cursor") == "2":
            result(mid, {"sessions": [known]})
        else:
            timed = json.loads(env("LIST_TIMES") or "{}").items()
            more = [{"sessionId": i, "cwd": params.get("cwd"), "updatedAt": t} for i, t in timed]
            result(mid, {"sessions": [old, *more], "nextCursor": "2"})
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
            THOUGHT_ID
            and params.get("configId") == THOUGHT_ID
            and params.get("value") in ("low", "high")
        ):
            effort = params["value"]
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
    elif method == "session/prompt" and str(sid).startswith("nope"):
        error(mid, -32002, f"Session not found: {sid}")
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
                if late in ("error", "never", "gate"):
                    end_turn(turn)
                else:
                    run(turn, sid, text_of(params["prompt"]))
            if late == "late":
                result(mid, {"outcome": "injected"})
            elif late == "error":
                error(mid, -32603, "steering failed")
            elif late == "gate":
                gate = env("STEER_GATE")
                threading.Thread(target=steer_later, args=(gate, mid), daemon=True).start()
        elif idle == "promptRequired":
            result(mid, {"outcome": "promptRequired", "reason": "noRunningTurn"})
        else:
            result(mid, {"outcome": "startedNewTurn"})
    elif method == "session/cancel":
        time.sleep(float(env("CANCEL_DELAY", "0")))
        if hanging is not None:
            end_turn(hanging, "cancelled")
            hanging = None
    elif method == "$/cancel_request":
        if hanging is not None and params.get("requestId") == hanging:
            error(hanging, -32800, "Request cancelled")
            hanging = None
    elif method is None and mid in using:
        client_answered(msg)
    elif method is None and mid in eliciting:
        elicited(mid, msg)
    elif method is None and mid in asking:
        with open_requests:
            turn = asking.pop(mid)
        if turn is not None and turn not in asking.values():
            end_turn(turn)
    elif method is not None and mid is not None:
        error(mid, -32601, f"Method not found: {method}")

if stubborn == "all":
    time.sleep(100000)
