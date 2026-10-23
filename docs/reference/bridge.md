# Bridge protocol

A bridge is a program a brnr process starts from its profile's `bridges`
([configuration](configuration.md#bridges)): it writes requests to its
stdout, and reads the replies and the [events](events.md) from its stdin.
Its stderr goes to the host log. An external client can instead connect to
the control socket, `$BRNR_DIR/<pid>.sock`, and speak the same protocol.
Access is limited by the private runtime directory: this is a local control
interface, not an HTTP service, nor a sandbox boundary against other programs
of the same user.

**Covered** from 1.0.0: the protocol as a bridge brnr starts from a profile
speaks it, on its stdout and stdin: each `cmd`, its fields and its reply, the
events, and how long the bridge lasts; and `BRNR_PID` in its environment.
**Not covered**: the control socket, which is in the private runtime
directory and is not an interface: its path, how processes and their
metadata files are found, and a client that connects to it itself, which
speaks this protocol today but has no promise and can change in any release;
`BRNR_SOCKET`; and the wording of an error's `message`.

## Requests and replies

A request is one UTF-8 JSON object, then a newline. A reply is one too,
matched to its request by `req_id`: replies to requests the agent answers
can come later, and events come between replies. A line that isn't JSON gets
an error reply; a request of more than 32 MiB is answered with an error, and
the peer is cut off. Handle EOF and a closed connection, rather than wait
for ever. The examples below show a long reply across lines; on the wire
each is one line.

| Field | Type | Meaning |
|---|---|---|
| `cmd` | string | The request: one of the sections below. |
| `req_id` | any | Optional. Echoed, as given, in the reply. Use a unique one. |
| `ok` | boolean | In every reply: `true` for success. |
| `error` | object, optional | In a failed reply, `ok` `false`: `{code, message}`, an [error](errors.md). Not there otherwise. |
| `error.code` | string | The [error code](errors.md). |
| `error.message` | string | What failed, for people. Not covered. |

A `cmd` the process doesn't have, or a field the section doesn't list for
it, is refused with `command_unknown`, and nothing is done: a process can be
an older brnr than the one that wrote the request (an editor's lives as long
as the editor's session), and does a request as asked or not at all. A field
of the wrong type, or one missing, is `command_invalid`. CLI `--json` results
leave out `ok` and `req_id`.

<!-- example: bridge; before: Idle -->
```json
{"cmd":"cancel","session":"sess-9","req_id":"r2"}
{
  "ok": false,
  "error": {
    "code": "session_not_found",
    "message": "no session sess-9"
  },
  "req_id": "r2"
}
```

A request about a session needs the exact id of a session this process
serves; one that is closing or opening takes none (`session_closing`,
`session_opening`). What the agent can't do, what strict mode refuses, and an
action an editor's profile doesn't enable are errors
(`capability_missing`, `strict_mode_refused`, `experimental_disabled`). On an
editor's session, every request that acts on it (`send`, `cancel`, `queue`'s
`clear_context`, `allow`, `reject`, `elicitation`, `set_config` and `close`)
is experimental: refused unless the profile enables it
([ADR 7.1](../adr/0007-experimental-actions-on-an-editors-session.md)).
Bridges observe it freely. `stop` is process management, not an
experimental action.

## Versions

A process says what it is in `status` (and its metadata file): `brnr`, its
version as `brnr --version` prints it, and `protocol`, the version of what
the socket speaks. `protocol` grows by one with each release that adds a
request, a field or an event, and only grows within a major version. The CLI
compares it with its own, and an error that may come of a difference says
which brnr the process is, and how to restart it: a headless one with `brnr
process stop <pid>` and `brnr session resume <session>`, an editor's from the
editor
([ADR 8.8](../adr/0008-the-command-line-and-compatibility.md#88-a-process-and-the-commands-can-be-different-brnrs)).

## Events and how long a bridge lasts

- A started bridge is subscribed from the start, to every event but `acp`
  unless its profile's `events` chooses. A socket client gets events once it
  sends `subscribe`. Bridge `events` take event names, not `--events`'s
  `default`.
- Read continuously. A peer 16 MiB behind is cut off: a connection is
  closed, a started bridge is sent SIGTERM. The host log's `peer_dropped`
  says so, with `last_queued_seq`, and the other peers get
  [`peer_cut_off`](events.md#peer_cut_off). A socket client catches up: it
  connects again, asks `was_cut_off`, sends `subscribe` again (its `seq` is
  where live events start), sends `logged`, and reads the process's records
  between its last event's `seq` and that one from the transcript (the host
  log names each session's files), as brnr's own commands do. A started
  bridge can't.
- When its process stops, a started bridge gets the last events, then EOF on
  its stdin. It should exit on EOF: it is sent SIGTERM if it still runs 2 s
  later.
- A started bridge's stdout closing ends its requests, not its events.
- [When a reader falls behind](../guide/events.md#when-a-reader-falls-behind)
  has the 16 MiB queue, long lines and the foreground's skipped events.

## `status`

The process's full report: its metadata, its state, and each session's.

**Request:** no fields.

**Reply:**

| Field | Type | Meaning |
|---|---|---|
| `id` | string | The process's pid, as text. |
| `host_id` | string | Its run, `<started>-<pid>`. |
| `brnr` | string | brnr's version, as `brnr --version` prints it. |
| `protocol` | number | The protocol's version. |
| `profile` | string or null | The profile it started with; null for none. |
| `host_pid` | number | The process. |
| `proxy_pid` | number or null | An editor's `brnr acp`; null headless. |
| `agent_pid` | number or null | The agent's process. |
| `agent` | list of strings | The agent's command, as the profile's `agent` has it. |
| `cwd` | string | Where it started. |
| `host_log` | string | Its host log. |
| `socket` | string | Its control socket. Not covered. |
| `started` | string | When it started. |
| `capabilities` | object | What the agent advertised, as booleans. |
| `capabilities.resume` | boolean | `session/resume`. |
| `capabilities.load` | boolean | `session/load`. |
| `capabilities.list` | boolean | `session/list`. |
| `capabilities.fork` | boolean | `session/fork`. |
| `capabilities.close` | boolean | `session/close`. |
| `capabilities.image` | boolean | Images in a prompt. |
| `capabilities.steering` | boolean | `_session/steering`. |
| `capabilities.mcp_http` | boolean | MCP servers over HTTP. |
| `capabilities.mcp_sse` | boolean | MCP servers over SSE. |
| `agent_info` | object or null, ACP's | The agent's `agentInfo` from `initialize`, as it sent it; null without one. |
| `owner` | string | `editor` or `headless`. |
| `pending_count` | number | Approvals waiting, in all its sessions. |
| `elicitation_count` | number | Elicitations waiting, outside a session too. |
| `uptime` | number | Seconds since it started. |
| `stop_when_idle` | number or null | Its idle timeout in seconds; null for none. |
| `starting` | boolean | Until its first session commits. |
| `stopping` | boolean | Whether it is stopping. |
| `closing` | list of strings | The sessions it has closed, or failed to open, and holds the locks of until their records are written ([ADR 5.5](../adr/0005-events-transcripts-and-observers.md#55-the-log-of-a-running-session-waits-for-its-transcript)). |
| `bridges` | list of strings | Its started bridges' names. |
| `sessions` | list of objects | Its sessions. |
| `sessions[].session` | string | The session's id. |
| `sessions[].state` | string | `idle`, `busy` or `waiting`. |
| `sessions[].busy` | boolean | Whether a turn runs. |
| `sessions[].title` | string or null | Its title. |
| `sessions[].mode` | string or null | Its current mode. |
| `sessions[].modes` | object or null, ACP's | ACP's v1 modes, as the agent gave them. |
| `sessions[].model` | string or null | Its current model. |
| `sessions[].config` | list or null, ACP's | ACP's config options, as the agent gave them. |
| `sessions[].commands` | list or null, ACP's | ACP's available commands, as the agent gave them. |
| `sessions[].plan` | list or null, ACP's | The last plan's entries. |
| `sessions[].usage` | object or null | The last usage, as [`usage`](events.md#usage) has it. |
| `sessions[].tools` | list of objects | The running tool calls, as [`tool_call`](events.md#tool_call) has them. |
| `sessions[].last_message` | string or null | The agent's last message, at most 4000 characters of it. |
| `sessions[].last_active` | string | When it last had an event. |
| `sessions[].cwd` | string | Its working directory. |
| `sessions[].forked_from` | string or null | The session it was forked from, where this process knows it. |
| `sessions[].turn_elapsed` | number or null | Seconds the running turn has run. |
| `sessions[].prompts` | number | Prompts the agent hasn't answered. |
| `sessions[].held_count` | number | Messages held. |
| `sessions[].context_count` | number | Context entries held. |
| `sessions[].pending_count` | number | Approvals waiting. |
| `sessions[].elicitation_count` | number | Elicitations waiting. |
| `sessions[].shared` | boolean | Whether it serves the session without its lock. |
| `sessions[].lock_error` | string or null | Why it has no lock on it. |
| `sessions[].last_turn` | object or null | How its last turn ended, as `session status` has it. |
| `sessions[].last_turn.stop_reason` | string or null | As [`turn_ended`](events.md#turn_ended) has it. |
| `sessions[].last_turn.error` | object or null | As `turn_ended` has it. |
| `sessions[].last_turn.tokens` | object or null | As `turn_ended` has it. |
| `sessions[].events_log` | string or null | Its events file; null when not kept (`log = "off"`). |
| `sessions[].acp_log` | string or null | Its raw ACP file; null when not kept. |

<!-- example: bridge; before: Idle -->
```json
{"cmd":"status","req_id":"r1"}
{
  "id": "4466",
  "host_id": "20261023T150820-4466",
  "brnr": "0.10.0",
  "protocol": 1,
  "profile": null,
  "host_pid": 4466,
  "proxy_pid": null,
  "agent_pid": 4468,
  "agent": [
    "brnr-claude-adapter"
  ],
  "cwd": "/home/me/project",
  "host_log": "/home/me/.brnr/hosts/20261023T150820-4466.jsonl",
  "socket": "/run/user/1000/brnr/4466.sock",
  "started": "2026-10-23T15:08:20.323457Z",
  "capabilities": {
    "resume": true,
    "load": true,
    "list": true,
    "fork": true,
    "close": true,
    "image": true,
    "steering": true,
    "mcp_http": true,
    "mcp_sse": false
  },
  "agent_info": {
    "name": "fake-agent",
    "version": "1.2.3"
  },
  "ok": true,
  "owner": "headless",
  "pending_count": 0,
  "elicitation_count": 0,
  "uptime": 0,
  "stop_when_idle": null,
  "starting": false,
  "stopping": false,
  "closing": [],
  "bridges": [
    "slack-bridge#0"
  ],
  "sessions": [
    {
      "title": "Fake session",
      "mode": "default",
      "modes": {
        "currentModeId": "default",
        "availableModes": [
          {
            "id": "default",
            "name": "Default",
            "description": "Asks before edits"
          },
          {
            "id": "plan",
            "name": "Plan",
            "description": "Plans, doesn't edit"
          }
        ]
      },
      "model": "small",
      "config": [
        {
          "id": "model",
          "name": "Model",
          "type": "select",
          "currentValue": "small",
          "options": [
            {
              "value": "small",
              "name": "Small"
            },
            {
              "value": "large",
              "name": "Large"
            }
          ],
          "category": "model"
        }
      ],
      "commands": [
        {
          "name": "compact",
          "description": "Compact the conversation"
        }
      ],
      "plan": null,
      "usage": null,
      "tools": [],
      "last_message": "hi",
      "session": "sess-1",
      "state": "idle",
      "last_active": "2026-10-23T15:08:20.532866Z",
      "cwd": "/home/me/project",
      "forked_from": null,
      "busy": false,
      "turn_elapsed": null,
      "prompts": 0,
      "held_count": 0,
      "context_count": 0,
      "pending_count": 0,
      "elicitation_count": 0,
      "shared": false,
      "lock_error": null,
      "last_turn": {
        "stop_reason": "end_turn",
        "error": null,
        "tokens": null
      },
      "events_log": "/home/me/.brnr/projects/-home-me-project-abd1481c/sess-1.jsonl",
      "acp_log": "/home/me/.brnr/projects/-home-me-project-abd1481c/sess-1.acp.jsonl"
    }
  ],
  "req_id": "r1"
}
```

## `logged`

Answered once what the process recorded before it was asked is in its
transcript, which another thread writes.

**Request:** no fields.

**Reply:**

| Field | Type | Meaning |
|---|---|---|
| `lost` | boolean | `true` when a file has lost records it hasn't noted yet (a full disk): look for the gaps and errors recorded. |

<!-- example: bridge; before: Idle -->
```json
{"cmd":"logged","req_id":"r1"}
{"ok":true,"req_id":"r1","lost":false}
```

## `send`

Sends a message to a session, as [`prompt send`](prompt.md#brnr-prompt-send)
does. An accepted send is not a turn that has ended: subscribe, and match its
`message` with `turn_ended`'s `messages` or `message_dropped`'s `message`.

**Request:**

| Field | Type | Meaning |
|---|---|---|
| `session` | string | The session. |
| `text` | string | The text. It or `blocks` is needed. |
| `blocks` | list of objects, ACP's | ACP content blocks to add: `text`, `resource_link` (`uri`), `image` (`data`, `mimeType`). |
| `mode` | string | `prompt` (the default), `steer`, `interrupt` or `context`. `context` is text only. |
| `replace` | boolean | With `context`: replace the last entry. |

**Reply:**

| Field | Type | Meaning |
|---|---|---|
| `status` | string | `delivered`, `held`, `steered` or `interrupting`; `held` for context. |
| `session` | string | The session. |
| `message` | string, optional | The message's id. Not there for context, which has none. |

<!-- example: bridge; before: Turn -->
```json
{"cmd":"send","session":"sess-1","text":"also update the changelog","req_id":"r1"}
{"ok":true,"status":"held","session":"sess-1","message":"m2","req_id":"r1"}
```

## `cancel`

Cancels the running turn, as [`prompt cancel`](prompt.md#brnr-prompt-cancel)
does.

**Request:**

| Field | Type | Meaning |
|---|---|---|
| `session` | string | The session. |
| `keep_held` | boolean | Keep the held messages. |

**Reply:**

| Field | Type | Meaning |
|---|---|---|
| `status` | string | `cancelling` or `idle`. |
| `session` | string | The session. |
| `dropped` | list of objects | The messages dropped, `{message, text, reason}`. |

<!-- example: bridge; before: Turn -->
```json
{"cmd":"cancel","session":"sess-1","req_id":"r1"}
{"ok":true,"status":"cancelling","session":"sess-1","dropped":[],"req_id":"r1"}
```

## `queue`

The held messages and context, after what was asked was dropped, as the
[queue](queue.md) commands print them; or, with `show`, one held message in
full, as [`queue show`](queue.md#brnr-queue-show) prints it.

**Request:**

| Field | Type | Meaning |
|---|---|---|
| `session` | string | The session. |
| `drop` | string | A held message's id, to drop. |
| `clear` | boolean | Drop every held message. |
| `clear_context` | boolean | Drop the held context. |
| `show` | string | A held message's id, to show; alone. One that isn't an id is refused. |

**Reply:** without `show`, `session`, `held`, `context` and `dropped`, as
[queue](queue.md) has them; with `show`, `session`, `message`, `text`,
`interrupt`, `attachments` and `blocks`, as `queue show` has them.

| Field | Type | Meaning |
|---|---|---|
| `session` | string | The session. |
| `held` | list of objects | The held messages, `{message, text, interrupt, attachments}`. |
| `context` | list of strings | The held context. |
| `dropped` | list of objects | The messages dropped, `{message, text, reason}`. |

<!-- example: bridge; before: Idle -->
```json
{"cmd":"queue","session":"sess-1","req_id":"r1"}
{"ok":true,"session":"sess-1","held":[],"context":[],"dropped":[],"req_id":"r1"}
```

## `subscribe`

Events follow on this connection. Selection is the process's: filter
sessions on the client.

**Request:**

| Field | Type | Meaning |
|---|---|---|
| `events` | list of strings, `"all"` or null, optional | The events. Not there, or null: every event but `acp`; `"all"`: every event; an empty list: none. A note's name is refused (`event_unknown`). |

**Reply:**

| Field | Type | Meaning |
|---|---|---|
| `events` | list of strings | The events subscribed to. |
| `unknown_events` | list of strings | The names asked for that this process doesn't have (a newer brnr's), left out of the subscription. |
| `peer` | string | This connection's name, `socket#7`. |
| `host_id` | string | The process's run. |
| `pid` | number | The process. |
| `seq` | number | The last event's position before the subscription. |

<!-- example: bridge; before: Idle -->
```json
{"cmd":"subscribe","events":["turn_ended","tool_dance"],"req_id":"r1"}
{
  "ok": true,
  "events": [
    "turn_ended"
  ],
  "unknown_events": [
    "tool_dance"
  ],
  "peer": "socket#2",
  "host_id": "20261023T150820-4466",
  "pid": 4466,
  "seq": 14,
  "req_id": "r1"
}
```

## `was_cut_off`

Whether the process cut a connection off for falling behind, of its last 64
cut-offs: a position only, never an event. Catch up from the transcript
([ADR 10.2](../adr/0010-bounded-resources.md#102-a-peer-cut-off-catches-up-from-the-transcript)).

**Request:**

| Field | Type | Meaning |
|---|---|---|
| `peer` | string | A connection's name, as `subscribe` gave it. |

**Reply:**

| Field | Type | Meaning |
|---|---|---|
| `peer` | string | The connection. |
| `cut_off` | boolean | Whether it was cut off. |
| `last_queued_seq` | number or null | The last position queued for it then; null if it wasn't. |

<!-- example: bridge; before: Idle -->
```json
{"cmd":"was_cut_off","peer":"socket#7","req_id":"r1"}
{"ok":true,"peer":"socket#7","cut_off":false,"last_queued_seq":null,"req_id":"r1"}
```

## `pending`

The approvals and elicitations waiting in this process, in full.

**Request:** no fields.

**Reply:**

| Field | Type | Meaning |
|---|---|---|
| `pending` | list of objects | The approvals, as [`permission show --json`](permission.md#brnr-permission-show) prints them. |
| `pending[].request` | string | As `permission show` has it. |
| `pending[].session` | string | As `permission show` has it. |
| `pending[].owner` | string | As `permission show` has it. |
| `pending[].answerable` | boolean | As `permission show` has it. |
| `pending[].why_not` | string or null | As `permission show` has it. |
| `pending[].title` | string or null | As `permission show` has it. |
| `pending[].kind` | string or null | As `permission show` has it. |
| `pending[].tool_call` | object, ACP's | As `permission show` has it. |
| `pending[].options` | list of objects, ACP's | As `permission show` has it. |
| `pending[].timeout_left` | number or null | As `permission show` has it. |
| `elicitations` | list of objects | The elicitations, as [`elicitation show --json`](elicitation.md#brnr-elicitation-show) prints them, each with the process's `pid`. |

<!-- example: bridge; before: Approval -->
```json
{"cmd":"pending","req_id":"r1"}
{
  "ok": true,
  "pending": [
    {
      "request": "p1",
      "session": "sess-1",
      "owner": "headless",
      "answerable": true,
      "why_not": null,
      "title": "Edit src/lib.rs",
      "kind": "edit",
      "tool_call": {
        "toolCallId": "perm-1",
        "title": "Edit src/lib.rs",
        "kind": "edit",
        "locations": [
          {
            "path": "src/lib.rs",
            "line": 2
          }
        ],
        "rawInput": {
          "file_path": "src/lib.rs"
        },
        "content": [
          {
            "type": "diff",
            "path": "src/lib.rs",
            "oldText": "one\nold line\nthree\n",
            "newText": "one\nnew line\nthree\n"
          }
        ]
      },
      "options": [
        {
          "optionId": "allow",
          "name": "Allow",
          "kind": "allow_once"
        },
        {
          "optionId": "reject",
          "name": "Reject",
          "kind": "reject_once"
        }
      ],
      "timeout_left": null
    }
  ],
  "elicitations": [],
  "req_id": "r1"
}
```

## `allow`, `reject`

Answers an approval, as [`permission allow` and
`reject`](permission.md#brnr-permission-allow-brnr-permission-reject) do.

**Request:**

| Field | Type | Meaning |
|---|---|---|
| `session` | string | The session. |
| `request` | string | The handle, `p1`. |
| `always` | boolean | The `always` kind. |
| `option_id` | string | The option's `optionId`, in place of a kind. Anything but a string, null too, is refused (`command_invalid`). Its old name, `option`, is a field `allow` doesn't take (`command_unknown`). |

**Reply:**

| Field | Type | Meaning |
|---|---|---|
| `session` | string | The session. |
| `request` | string | The handle. |
| `outcome` | object, ACP's | ACP's outcome, as sent. |

<!-- example: bridge; before: Approval -->
```json
{"cmd":"allow","session":"sess-1","request":"p1","req_id":"r1"}
{
  "ok": true,
  "session": "sess-1",
  "request": "p1",
  "outcome": {
    "outcome": "selected",
    "optionId": "allow"
  },
  "req_id": "r1"
}
```

## `elicitation`

Answers an elicitation with ACP's action, as [`elicitation
accept`](elicitation.md#brnr-elicitation-accept-decline-cancel) and the others
do.

**Request:**

| Field | Type | Meaning |
|---|---|---|
| `session` | string or null | The session; null for one outside any session. |
| `request` | string | The handle, `e1`. |
| `action` | string | `accept`, `decline` or `cancel`. |
| `content` | object | For a form's accept: its fields, checked against its schema before anything is sent. |

**Reply:**

| Field | Type | Meaning |
|---|---|---|
| `session` | string or null | The session. |
| `request` | string | The handle. |
| `result` | object, ACP's | The response sent to the agent: `action`, and an accept's `content`. |

<!-- example: bridge; before: Elicitation -->
```json
{"cmd":"elicitation","session":"sess-1","request":"e1","action":"decline","req_id":"r1"}
{"ok":true,"session":"sess-1","request":"e1","result":{"action":"decline"},"req_id":"r1"}
```

## `set_config`

Sets a session's settings, as [`config set`](config.md#brnr-config-set)
does: resolved and sent the same way, and answered once all are set, or with
the first failure, which names what was already set.

**Request:**

| Field | Type | Meaning |
|---|---|---|
| `session` | string | The session. |
| `mode` | string | The mode. |
| `model` | string | The model. |
| `thought_level` | string | The thought level. |
| `options` | object | Option ids to values, strings. |

At least one of `mode`, `model`, `thought_level` and `options` is needed.

**Reply:**

| Field | Type | Meaning |
|---|---|---|
| `session` | string | The session. |
| `set` | list of objects | Each setting sent, `{option, category, value}`, as `config set` has them. |
| `set[].option` | string or null | As `config set` has it. |
| `set[].category` | string or null | As `config set` has it. |
| `set[].value` | string | As `config set` has it. |
| `mode` | string or null | The current mode. |
| `config` | list or null, ACP's | The agent's config options now, as ACP has them. |

<!-- example: bridge; before: Idle -->
```json
{"cmd":"set_config","session":"sess-1","model":"large","req_id":"r1"}
{
  "ok": true,
  "session": "sess-1",
  "set": [
    {
      "option": "model",
      "category": "model",
      "value": "large"
    }
  ],
  "mode": "default",
  "config": [
    {
      "id": "model",
      "name": "Model",
      "type": "select",
      "currentValue": "large",
      "options": [
        {
          "value": "small",
          "name": "Small"
        },
        {
          "value": "large",
          "name": "Large"
        }
      ],
      "category": "model"
    }
  ],
  "req_id": "r1"
}
```

## `fork`

Forks a session into a new one in this process, as [`session
fork`](session.md#brnr-session-fork) does. Never in an editor's process.

**Request:**

| Field | Type | Meaning |
|---|---|---|
| `session` | string | The session. |

**Reply:**

| Field | Type | Meaning |
|---|---|---|
| `session` | string | The new session's id. |

<!-- example: bridge; before: Idle -->
```json
{"cmd":"fork","session":"sess-1","req_id":"r1"}
{"ok":true,"session":"sess-2","req_id":"r1"}
```

## `new`

Opens a session in this process (`session/new`, with the process's MCP
servers), sets its settings over the profile's, then sends the prompt, if
there is one: [`session new --pid`](session.md#brnr-session-new). Refused by
an editor's process, and with `stop_when_idle` when the agent can't close
sessions.

**Request:**

| Field | Type | Meaning |
|---|---|---|
| `cwd` | string | Its working directory, absolute; the process's by default. |
| `mode`, `model`, `thought_level`, `options` | | As for `set_config`. |
| `text` | string | The first prompt's text. |
| `blocks` | list of objects, ACP's | The first prompt's content blocks, as for `send`. |
| `timeout` | number | Seconds it may take; past it, the opening is abandoned. |

**Reply:** answered once the session is open and its settings set, before
the prompt goes.

| Field | Type | Meaning |
|---|---|---|
| `pid` | number | The process. |
| `session` | string | The new session's id. |
| `message` | string or null | The prompt's message id; null without a prompt. |

A failure after the agent opened the session (a setting, `timeout` passing,
the process stopping) fails the request, and the prompt isn't sent. The
session is closed again (or ends with a stopping process), and the error
says so, or names it, left open, when the agent can't close sessions.

<!-- example: bridge; before: Idle -->
```json
{"cmd":"new","text":"review the diff","req_id":"r1"}
{"ok":true,"pid":4466,"session":"sess-2","message":"m2","req_id":"r1"}
```

## `resume`

Opens an existing session in this process: `session/resume`, or
`session/load` where the agent has only that.
[`session resume --pid`](session.md#brnr-session-resume). The session's lock
is taken first, so one another process holds, or this one serves, is
refused.

**Request:** `session`, and the fields of `new`, `cwd` being the session's.

| Field | Type | Meaning |
|---|---|---|
| `session` | string | The session's id. |

**Reply:** as `new`.

| Field | Type | Meaning |
|---|---|---|
| `pid` | number | The process. |
| `session` | string | The session's id, the one resumed. |
| `message` | string or null | The prompt's message id; null without a prompt. |

<!-- example: bridge; before: Idle -->
```json
{"cmd":"resume","session":"old-1","req_id":"r1"}
{"ok":true,"pid":4466,"session":"old-1","message":null,"req_id":"r1"}
```

## `close`

Cancels a running turn, then closes the session (`session/close`), as
[`session close`](session.md#brnr-session-close) does. Answered once the
agent has closed it. A headless process whose last session closes stops.

**Request:**

| Field | Type | Meaning |
|---|---|---|
| `session` | string | The session. |
| `take_over` | number | The pid of the process that `session resume --take-over` resumes it in. |

**Reply:**

| Field | Type | Meaning |
|---|---|---|
| `session` | string | The session closed. |

<!-- example: bridge; before: Idle -->
```json
{"cmd":"close","session":"sess-1","req_id":"r1"}
{"ok":true,"session":"sess-1","req_id":"r1"}
```

## `stop`

Stops the process: it closes the agent's stdin, then sends SIGTERM, then
SIGKILL, to the agent's process group. It doesn't wait for the process to
exit.

**Request:** no fields.

**Reply:**

| Field | Type | Meaning |
|---|---|---|
| `status` | string | `stopping`. |

<!-- example: bridge; before: Idle -->
```json
{"cmd":"stop","req_id":"r1"}
{"ok":true,"status":"stopping","req_id":"r1"}
```
