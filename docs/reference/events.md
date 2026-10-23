# Events

A process's events are the story of its sessions
([ADR 5.1](../adr/0005-events-transcripts-and-observers.md#51-the-hosts-events-are-the-story-and-transcripts-are-two-files)).
The same events go everywhere: a started bridge's stdin, the socket's
`subscribe`, `event watch --json`, `event log --json` (read back from the
transcript), `session new --foreground --json`, `event notify`'s stdin, and
the transcript's records. Each is one JSON object on one line; the examples
here show it across lines.

Every name and fixed value is snake_case, the subject first
([ADR 5.9](../adr/0005-events-transcripts-and-observers.md#59-events-records-and-their-fields-follow-one-naming-scheme)).
Who acted is `by`: `editor`, `agent` or `control` (brnr's commands, bridges
and timers). Beside it, `peer` is the connection that asked, where brnr knows
one (`socket#7`, a bridge's name), and `reason` is why, a word.

Every event has these fields:

| Field | Type | Meaning |
|---|---|---|
| `event` | string | Its name: one of the sections below. |
| `session` | string | The session it is of. Not there on the process's own events: `line_too_long`, `line_too_deep`, `peer_cut_off`, `process_exited`. |
| `ts` | string | When it happened. |
| `host_id` | string or null | The process's run, `<started>-<pid>`. Null on the `session_deleted` that `session delete` records, which no process does. |
| `pid` | number or null | The process. Null where `host_id` is. |
| `seq` | number | Its position: one more than the last event's or ACP message's of the process, live and in the transcript. Not there on the `session_deleted` that `session delete` records. |
| `replayed` | boolean | `true` on an event of a load's replayed history, which brnr had no transcript of ([ADR 5.6](../adr/0005-events-transcripts-and-observers.md#56-a-loads-history-is-recorded-once)). Not there otherwise. |

The process's own events, `process_exited`, `line_too_long` and
`line_too_deep`, are in every session's transcript; `peer_cut_off` is in the
host log.

**Covered** from 1.0.0: each event's name and fields, wherever it goes: a
bridge's stdin, `--json` lines, `event notify`'s stdin and its variables.
Ignore the events and fields you don't know: a minor release can add them.

Which events: a bridge gets every event but `acp`, unless its profile's
`events` chooses; a socket client gets none until it subscribes. `event
watch`, `event log` and the foreground leave out `acp`, `agent_thought`,
`usage` and `tool_progress` unless `--events` asks for them
([event](event.md)).

## `user_message`

A message sent to the agent, as its turn takes it.

| Field | Type | Meaning |
|---|---|---|
| `by` | string | `control` (brnr's) or `editor` (the editor's own prompt). |
| `message` | string | The message's id, `m1`, as `prompt send` gave it. Not there for an editor's own prompt. |
| `prompt` | string | The turn's key: its `session/prompt` request's JSON-RPC id, as JSON text (`"\"brnr-3\""`). `turn_ended` has the same. |
| `text` | string | Its text; an attachment as `[resource_link <uri>]`, `[image]`. |

A replayed `user_message` (a load's history) has `text` and `replayed`
only.

<!-- example: brnr event log $s --json; before: Idle; event: user_message -->
```json
{
  "event": "user_message",
  "session": "sess-1",
  "by": "control",
  "message": "m1",
  "prompt": "\"brnr-3\"",
  "text": "reply hi",
  "ts": "2026-10-23T15:02:31.807712Z",
  "host_id": "20261023T150231-4466",
  "pid": 4466,
  "seq": 5
}
```

## `agent_message`

The agent's message, whole: its chunks put together.

| Field | Type | Meaning |
|---|---|---|
| `text` | string | The message. |

<!-- example: brnr event log $s --json; before: Idle; event: agent_message -->
```json
{
  "event": "agent_message",
  "session": "sess-1",
  "text": "hi",
  "ts": "2026-10-23T15:02:31.808162Z",
  "host_id": "20261023T150231-4466",
  "pid": 4466,
  "seq": 25
}
```

## `agent_thought`

The agent's thought, whole, as `agent_message` has a message.

| Field | Type | Meaning |
|---|---|---|
| `text` | string | The thought. |

<!-- example: brnr prompt send $s --wait --prompt think && brnr event log $s --events all --json; before: Idle; event: agent_thought -->
```json
{
  "event": "agent_thought",
  "session": "sess-1",
  "text": "pondering",
  "ts": "2026-10-23T15:02:31.812650Z",
  "host_id": "20261023T150231-4466",
  "pid": 4466,
  "seq": 30
}
```

## `tool_call`

A tool call starting (`started`), and again ending (`status` `completed` or
`failed`).

| Field | Type | Meaning |
|---|---|---|
| `tool_call_id` | string | The agent's id of the call. |
| `title` | string | Its title. |
| `kind` | string | ACP's kind: `read`, `edit`, `execute`, … |
| `status` | string | ACP's status: `pending`, `in_progress`, `completed`, `failed`. |
| `locations` | list of objects | The paths it works on, as the agent gave them. |
| `started` | boolean | `true` when it starts. Not there when it ends. |

<!-- example: brnr prompt send $s --wait --prompt tools && brnr event log $s --json; before: Idle; event: tool_call -->
```json
{
  "tool_call_id": "t1",
  "title": "Run the tests",
  "kind": "execute",
  "status": "pending",
  "locations": [],
  "event": "tool_call",
  "session": "sess-1",
  "started": true,
  "ts": "2026-10-23T15:02:31.808098Z",
  "host_id": "20261023T150231-4466",
  "pid": 4466,
  "seq": 13
}
```

## `tool_progress`

A running tool call's status changing in between (`in_progress`). Progress
that doesn't change it is no event.

| Field | Type | Meaning |
|---|---|---|
| `tool_call_id` | string | The call. |
| `title` | string | Its title. |
| `kind` | string | Its kind. |
| `status` | string | Its new status. |
| `locations` | list of objects | The paths it works on. |

<!-- example: brnr prompt send $s --wait --prompt tools && brnr event log $s --events all --json; before: Idle; event: tool_progress -->
```json
{
  "tool_call_id": "t1",
  "title": "Run the tests",
  "kind": "execute",
  "status": "in_progress",
  "locations": [],
  "event": "tool_progress",
  "session": "sess-1",
  "ts": "2026-10-23T15:02:31.808109Z",
  "host_id": "20261023T150231-4466",
  "pid": 4466,
  "seq": 15
}
```

## `plan`

The agent's plan, whole, each time it changes.

| Field | Type | Meaning |
|---|---|---|
| `entries` | list of objects | ACP's plan entries: `content`, `status`, `priority`. |

<!-- example: brnr prompt send $s --wait --prompt tools && brnr event log $s --json; before: Idle; event: plan -->
```json
{
  "event": "plan",
  "session": "sess-1",
  "entries": [
    {
      "content": "Run the tests",
      "status": "in_progress",
      "priority": "high"
    },
    {
      "content": "Fix them",
      "status": "pending",
      "priority": "high"
    }
  ],
  "ts": "2026-10-23T15:02:31.808057Z",
  "host_id": "20261023T150231-4466",
  "pid": 4466,
  "seq": 11
}
```

## `usage`

The context window and its cost, as the agent reports them.

| Field | Type | Meaning |
|---|---|---|
| `usage` | object | What the agent reported. |
| `usage.used` | number | Tokens in the context window. |
| `usage.size` | number | The context window's size. |
| `usage.cost` | object or null | The cost, as the agent gave it (`{amount, currency}`); null if it gave none. |

<!-- example: brnr prompt send $s --wait --prompt tools && brnr event log $s --events all --json; before: Idle; event: usage -->
```json
{
  "event": "usage",
  "session": "sess-1",
  "usage": {
    "used": 12345,
    "size": 200000,
    "cost": {
      "amount": 0.42,
      "currency": "USD"
    }
  },
  "ts": "2026-10-23T15:02:31.808144Z",
  "host_id": "20261023T150231-4466",
  "pid": 4466,
  "seq": 21
}
```

## `session_changed`

The session's title, mode, config options or commands changed.

| Field | Type | Meaning |
|---|---|---|
| `what` | string | `title`, `mode`, `config` or `commands`. |
| `value` | string or object | A `title`'s or `mode`'s new value. For `config` and `commands`, a JSON merge patch by option id or command name: each one new or changed, with its value (an option's `currentValue`, a command's object), and `null` for one gone: `{"review": {"name": "review", …}, "compact": null}`. |

<!-- example: brnr prompt send $s --wait --prompt "settings large" && brnr event log $s --last 1 --json; before: Idle; event: session_changed -->
```json
{
  "event": "session_changed",
  "session": "sess-1",
  "what": "config",
  "value": {
    "model": "large"
  },
  "ts": "2026-10-23T15:02:31.816832Z",
  "host_id": "20261023T150231-4466",
  "pid": 4466,
  "seq": 37
}
```

## `permission_request`

An approval waiting: the agent's `session/request_permission`, held for an
answer. Its fields are those of [`permission
show`](permission.md#brnr-permission-show) that don't change.

| Field | Type | Meaning |
|---|---|---|
| `request` | string | Its handle, `p1`. |
| `owner` | string | `editor` or `headless`. |
| `title` | string or null | The tool call's title, as the agent gave it. |
| `kind` | string or null | The tool call's kind, as the agent gave it. |
| `tool_call` | object | The agent's tool call, whole. |
| `options` | list of objects | The agent's options, `{optionId, name, kind}`. |

<!-- example: brnr event log $s --json; before: Approval; event: permission_request -->
```json
{
  "event": "permission_request",
  "session": "sess-1",
  "request": "p1",
  "owner": "headless",
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
  "ts": "2026-10-23T15:07:45.069842Z",
  "host_id": "20261023T150744-4466",
  "pid": 4466,
  "seq": 12
}
```

## `permission_resolved`

An approval answered.

| Field | Type | Meaning |
|---|---|---|
| `request` | string | Its handle. |
| `answer` | string or null | `allowed`, `rejected` or `cancelled`; null for an editor's answer with an option of a kind brnr doesn't know. |
| `option_kind` | string or null | The chosen option's kind; null when none was chosen. |
| `outcome` | object | ACP's outcome, as sent: `{"outcome": "selected", "optionId": …}` or `{"outcome": "cancelled"}`. |
| `by` | string | `control`, `editor`, or `agent`: the agent gave the request up with `$/cancel_request`. |
| `peer` | string or null | The connection that answered (`socket#7`, a bridge's name); null where there is none. |
| `reason` | string or null | Why brnr answered by itself: `timeout` (the response timeout) or `cancel` (its turn was cancelled); null when someone answered. |

`by: agent` is on a request the agent gave up with `$/cancel_request`
([ADR 4.5](../adr/0004-turns-messages-and-requests.md)): headless, brnr
answered it `cancelled` (an elicitation, `cancel`); an editor answers its
own. It is no longer waiting, and an `allow`, `reject` or `elicitation` for
it fails.

<!-- example: brnr permission allow $s p1 && brnr event log $s --json; before: Approval; event: permission_resolved -->
```json
{
  "event": "permission_resolved",
  "session": "sess-1",
  "request": "p1",
  "answer": "allowed",
  "option_kind": "allow_once",
  "outcome": {
    "outcome": "selected",
    "optionId": "allow"
  },
  "by": "control",
  "peer": "socket#7",
  "reason": null,
  "ts": "2026-10-23T15:07:45.078788Z",
  "host_id": "20261023T150744-4466",
  "pid": 4466,
  "seq": 14
}
```

## `elicitation_request`

An elicitation waiting: the agent's `elicitation/create`, held for an answer.
Its fields are those of [`elicitation
show`](elicitation.md#brnr-elicitation-show) that don't change.

| Field | Type | Meaning |
|---|---|---|
| `session` | string or null | Its session; null for one outside any session. |
| `request` | string | Its handle, `e1`. |
| `owner` | string | `editor` or `headless`. |
| `mode` | string or null | `form`, `url`, or a mode brnr doesn't know. |
| `text` | string or null | The agent's message. |
| `scope` | string | `session`, `tool_call` or `request`. |
| `tool_call_id` | string or null | The tool call, for scope `tool_call`. |
| `scope_request_id` | string, number or null | The request, for scope `request`. |
| `schema` | object or null | A form's `requestedSchema`; null for a URL. |
| `url` | string or null | A URL's address; null for a form. |
| `elicitation_id` | string or null | A URL's `elicitationId`; null for a form. |

<!-- example: brnr prompt send $s --prompt "elicit url" && brnr event wait $s --for elicitation && brnr event log $s --json; before: Idle; event: elicitation_request -->
```json
{
  "event": "elicitation_request",
  "session": "sess-1",
  "request": "e1",
  "owner": "headless",
  "mode": "url",
  "text": "Log in to the fake",
  "scope": "session",
  "tool_call_id": null,
  "scope_request_id": null,
  "schema": null,
  "url": "https://login.example/a?b=1",
  "elicitation_id": "el-1",
  "ts": "2026-10-23T15:07:57.998001Z",
  "host_id": "20261023T150757-4466",
  "pid": 4466,
  "seq": 12
}
```

## `elicitation_resolved`

An elicitation answered.

| Field | Type | Meaning |
|---|---|---|
| `session` | string or null | Its session; null for one outside any session. |
| `request` | string | Its handle. |
| `action` | string or null | ACP's action: `accept`, `decline` or `cancel`, or what an editor sent. |
| `content` | object or null | An accept's form values; null otherwise. |
| `by` | string | `control`, `editor` or `agent`, as for `permission_resolved`. |
| `peer` | string or null | The connection that answered; null where there is none. |
| `reason` | string or null | `timeout` or `cancel` when brnr answered by itself; null otherwise. |

<!-- example: brnr elicitation accept $s e1 --field name=Ada --field color=green && brnr event log $s --json; before: Elicitation; event: elicitation_resolved -->
```json
{
  "event": "elicitation_resolved",
  "session": "sess-1",
  "request": "e1",
  "action": "accept",
  "content": {
    "color": "green",
    "name": "Ada"
  },
  "by": "control",
  "peer": "socket#7",
  "reason": null,
  "ts": "2026-10-23T15:07:49.039746Z",
  "host_id": "20261023T150748-4466",
  "pid": 4466,
  "seq": 14
}
```

## `elicitation_completed`

The agent says a URL elicitation's flow is done (`elicitation/complete`).

| Field | Type | Meaning |
|---|---|---|
| `session` | string or null | The elicitation's session; null if brnr doesn't know it. |
| `request` | string or null | Its handle; null if brnr doesn't know it. |
| `elicitation_id` | string | The agent's `elicitationId`. |

<!-- example: brnr prompt send $s --prompt "elicit url" && brnr event wait $s --for elicitation && brnr elicitation accept $s e1 && brnr event wait $s && brnr event log $s --json; before: Idle; event: elicitation_completed -->
```json
{
  "event": "elicitation_completed",
  "session": "sess-1",
  "request": "e1",
  "elicitation_id": "el-1",
  "ts": "2026-10-23T15:07:58.006073Z",
  "host_id": "20261023T150757-4466",
  "pid": 4466,
  "seq": 16
}
```

## `turn_ended`

A turn ended: the agent answered its `session/prompt`.

| Field | Type | Meaning |
|---|---|---|
| `by` | string | `control` for a turn brnr started, `editor` for the editor's. |
| `prompt` | string | The turn's key, as its `user_message` has it. |
| `messages` | list of strings | The messages it carried: the prompt's, and those steered into it. |
| `stop_reason` | string or null | ACP's stop reason: `end_turn`, `cancelled`, `max_tokens`, …; null if the turn failed. |
| `error` | object or null | The turn's failure: `{code, message}`, `agent_failed` with the agent's JSON-RPC error as `rpc_error` ([errors](errors.md)); null if it didn't fail. |
| `tokens` | object or null | The counts the agent put in its answer (`usage`, unstable in ACP); null when the answer has no `usage` object. |
| `tokens.total` | number or null | `totalTokens`. |
| `tokens.input` | number or null | `inputTokens`. |
| `tokens.output` | number or null | `outputTokens`. |
| `tokens.thought` | number or null | `thoughtTokens`. |
| `tokens.cached_read` | number or null | `cachedReadTokens`. |
| `tokens.cached_write` | number or null | `cachedWriteTokens`. |

Each count is the agent's number, or null where it gave none that is a count
(a JSON integer that fits a `u64`; not `1e3`). brnr adds nothing up and
doesn't say what they cover: whether subagents or earlier model calls are in
them is the agent's to say
([ADR 4.4](../adr/0004-turns-messages-and-requests.md)).

<!-- example: brnr event log $s --json; before: Idle; event: turn_ended; agent: PROMPT_USAGE={"totalTokens":1234,"inputTokens":1000,"outputTokens":234} -->
```json
{
  "event": "turn_ended",
  "session": "sess-1",
  "by": "control",
  "prompt": "\"brnr-3\"",
  "messages": [
    "m1"
  ],
  "stop_reason": "end_turn",
  "error": null,
  "tokens": {
    "total": 1234,
    "input": 1000,
    "output": 234,
    "thought": null,
    "cached_read": null,
    "cached_write": null
  },
  "ts": "2026-10-23T15:02:31.808172Z",
  "host_id": "20261023T150231-4466",
  "pid": 4466,
  "seq": 26
}
```

## `message_dropped`

A message that never went to the agent.

| Field | Type | Meaning |
|---|---|---|
| `message` | string | Its id. |
| `text` | string | Its text. |
| `reason` | string | `cancel` (`prompt cancel`), `queue` (`queue drop`, `queue clear`), `close` (its session closed), `exit` (the agent exited) or `steer` (the agent refused it as a steer). |

<!-- example: brnr prompt send $s --prompt later && brnr prompt cancel $s && brnr event log $s --json; before: Turn; event: message_dropped -->
```json
{
  "event": "message_dropped",
  "session": "sess-1",
  "message": "m2",
  "text": "later",
  "reason": "cancel",
  "ts": "2026-10-23T15:15:01.068955Z",
  "host_id": "20261023T151500-4466",
  "pid": 4466,
  "seq": 11
}
```

## `context_dropped`

Context that never joined a prompt.

| Field | Type | Meaning |
|---|---|---|
| `text` | string | Its text. |
| `reason` | string | `queue` (`queue clear`), `close` or `exit`. |

<!-- example: brnr prompt send $s --context --prompt "the API key is in .env.local" && brnr queue clear $s --context && brnr event log $s --json; before: Idle; event: context_dropped -->
```json
{
  "event": "context_dropped",
  "session": "sess-1",
  "text": "the API key is in .env.local",
  "reason": "queue",
  "ts": "2026-10-23T15:15:01.074214Z",
  "host_id": "20261023T151500-4466",
  "pid": 4466,
  "seq": 15
}
```

## `session_closed`

A session closed. What it held is dropped first, each with its event. The
last session of a process that stops when idle isn't closed: the process
stops, and [`process_exited`](#process_exited) says so in place of this
([ADR 3.8](../adr/0003-processes-sessions-and-ownership.md#38-stopping-when-idle)).

| Field | Type | Meaning |
|---|---|---|
| `by` | string | `control` or `editor`. |
| `reason` | string | By `control`: `close` (`session close`, `--take-over`) or `idle` (`stop_when_idle`). By `editor`: `close`. |

<!-- example: brnr event log $s --json; before: Ended; event: session_closed -->
```json
{
  "event": "session_closed",
  "session": "sess-1",
  "by": "control",
  "reason": "close",
  "ts": "2026-10-23T15:15:01.084882Z",
  "host_id": "20261023T151500-4466",
  "pid": 4466,
  "seq": 18
}
```

## `session_reopened`

An editor's load of a session that the agent answered with success after it
answered the session's close: the session is open again, and the load's
`history` follows. None if another process took the session meanwhile, which
the editor's process then doesn't serve
([ADR 3.9](../adr/0003-processes-sessions-and-ownership.md#39-ownership-is-a-per-session-lock)).

| Field | Type | Meaning |
|---|---|---|
| `by` | string | `editor`. |

```json
{
  "event": "session_reopened",
  "session": "sess-1",
  "by": "editor",
  "ts": "2026-10-23T15:20:11.120412Z",
  "host_id": "20261023T152005-4466",
  "pid": 4466,
  "seq": 31
}
```

## `session_deleted`

The agent deleted the session (`session/delete`). brnr's transcript stays.

| Field | Type | Meaning |
|---|---|---|
| `by` | string | `control` (`session delete`) or `editor`. |

`session delete` records it in the session's transcript with `host_id` and
`pid` null, and no `seq`: no process has the session open. An editor's
deletion, through its process, goes into every file of the session.

<!-- example: brnr session delete $s && brnr event log $s --json; before: Ended; event: session_deleted -->
```json
{
  "event": "session_deleted",
  "session": "sess-1",
  "by": "control",
  "ts": "2026-10-23T15:15:01.648337Z",
  "host_id": null,
  "pid": null
}
```

## `history`

A load's replay ended (`session/load`): the history the agent sent again.

| Field | Type | Meaning |
|---|---|---|
| `updates` | number | How many updates it replayed. |
| `recorded` | boolean | Whether the replay was taken into the transcript at all, as events with `replayed`. |
| `recorded_updates` | number | How many of the `updates` are in it: all when `recorded`, none when not, fewer when the session closed while its load was unanswered, as what came meanwhile had no file to go to. |
| `loads` | number | How many loads it answers. An editor's loads of a session that overlap share one replay and one `history` event, so this can be more than 1, and a recorded history is then in the transcript once for each load. |

<!-- example: brnr session resume old-1 -- brnr-claude-adapter && brnr event log old-1 --json; before: Nothing; agent: NO_RESUME=1; event: history -->
```json
{
  "event": "history",
  "session": "old-1",
  "updates": 3,
  "recorded": true,
  "recorded_updates": 3,
  "loads": 1,
  "ts": "2026-10-23T15:14:52.389129Z",
  "host_id": "20261023T151452-4466",
  "pid": 4466,
  "seq": 11
}
```

## `line_too_long`

A line past the limit (32 MiB), not read and not recorded. With an editor it
goes on to the other side; headless, it is dropped.

| Field | Type | Meaning |
|---|---|---|
| `from` | string | `agent` or `editor`. |
| `limit` | number | The limit, in bytes. |
| `relayed` | boolean | Whether it went on to the other side. |

<!-- example: brnr prompt send $s --wait --prompt "long 33" && brnr event log $s --json; before: Idle; event: line_too_long -->
```json
{
  "event": "line_too_long",
  "from": "agent",
  "limit": 33554432,
  "relayed": false,
  "ts": "2026-10-23T15:14:47.066039Z",
  "host_id": "20261023T151446-4466",
  "pid": 4466,
  "seq": 22
}
```

## `line_too_deep`

A line nested past the depth brnr reads (128 levels; `[[1]]` is 2), not read
and not recorded. With an editor it goes on to the other side; headless, it
is dropped. brnr tracks nothing of it: a permission request that deep isn't
listed or answerable from outside.

| Field | Type | Meaning |
|---|---|---|
| `from` | string | `agent` or `editor`. |
| `max_depth` | number | The depth brnr reads. |
| `relayed` | boolean | Whether it went on to the other side. |

<!-- example: brnr prompt send $s --wait --prompt "odd deep" && brnr event log $s --json; before: Idle; agent: ASK_AND_END=1; event: line_too_deep -->
```json
{
  "event": "line_too_deep",
  "from": "agent",
  "max_depth": 128,
  "relayed": false,
  "ts": "2026-10-23T15:14:47.046629Z",
  "host_id": "20261023T151446-4466",
  "pid": 4466,
  "seq": 17
}
```

## `peer_cut_off`

A peer (a watcher, a bridge) cut off for falling behind
([ADR 10.2](../adr/0010-bounded-resources.md#102-a-peer-cut-off-catches-up-from-the-transcript)),
told to the other peers.

| Field | Type | Meaning |
|---|---|---|
| `peer` | string | The connection cut off (`socket#7`, a bridge's name). |
| `last_queued_seq` | number | The last event's position it was sent. |

```json
{
  "event": "peer_cut_off",
  "peer": "socket#7",
  "last_queued_seq": 4120,
  "ts": "2026-10-23T15:21:40.501930Z",
  "host_id": "20261023T152005-4466",
  "pid": 4466,
  "seq": 4121
}
```

## `process_exited`

The agent exited, and the process with it.

| Field | Type | Meaning |
|---|---|---|
| `exit_status` | object or null | `{"code": n}`, or `{"signal": n}` for one a signal ended; null where it isn't known. |
| `error` | object | Why, `{code, message}`, when brnr itself failed (`process_panicked`). Not there otherwise. |

<!-- example: brnr session new --stop-when-idle 0 --prompt "slow 1" -- brnr-claude-adapter && brnr event wait $s --for exit --json; before: Nothing -->
```json
{
  "event": "process_exited",
  "exit_status": {
    "code": 0
  },
  "ts": "2026-10-23T15:14:53.637024Z",
  "host_id": "20261023T151452-4466",
  "pid": 4466,
  "seq": 13
}
```

## `acp`

An ACP message, with its direction: every message between the agent and the
editor or brnr. Only to a peer that asks for it by name, and in `event log`
read from the raw ACP file; never in the events file.

| Field | Type | Meaning |
|---|---|---|
| `dir` | string | `editor->agent`, `agent->editor`, `control->agent` (brnr's: a prompt, a cancel, an answer as the client), `control->editor` (a message sent through brnr, shown to the editor), `agent->control` (an answer for brnr), or `agent_stderr`. |
| `session` | string or null | The session it is of; null for one of none. |
| `msg` | object or string | The message, as brnr read it; a line that isn't JSON as a string. |

<!-- example: brnr event log $s --events acp --json; before: Idle; event: acp msg.method=session/prompt -->
```json
{
  "event": "acp",
  "dir": "control->agent",
  "session": "sess-1",
  "msg": {
    "jsonrpc": "2.0",
    "id": "brnr-3",
    "method": "session/prompt",
    "params": {
      "sessionId": "sess-1",
      "prompt": [
        {
          "type": "text",
          "text": "reply hi"
        }
      ]
    }
  },
  "ts": "2026-10-23T15:02:31.807726Z",
  "host_id": "20261023T150231-4466",
  "pid": 4466,
  "seq": 6
}
```

## Notes

Notes are what brnr adds to a stream of events. No process sends one:
`subscribe` and a bridge's `events` refuse their names (`event_unknown`).
`--events` takes them, and shows them whatever it chose.

### `records_skipped`

A gap in a transcript: records brnr couldn't write (a disk too slow or full),
in `event log`.

| Field | Type | Meaning |
|---|---|---|
| `count` | number | Event records skipped. |
| `acp_count` | number | Raw ACP records skipped. |
| `since` | string | When the first was made. |
| `until` | string | When the last was made. |
| `first_seq` | number | The first's position. Not there when none had one. |
| `last_seq` | number | The last's position. Not there when none had one. |
| `error` | object | Why, `{code, message}`. Not there when brnr knows none. |

It has `ts`, `host_id` and `pid`, and `session` in a session's file.

```json
{
  "event": "records_skipped",
  "count": 120,
  "acp_count": 4,
  "since": "2026-10-23T15:30:00.104512Z",
  "until": "2026-10-23T15:30:02.881002Z",
  "first_seq": 981,
  "last_seq": 1104,
  "ts": "2026-10-23T15:30:02.881140Z",
  "host_id": "20261023T152005-4466",
  "pid": 4466,
  "session": "sess-1"
}
```

### `events_missed`

What a command cut off for falling behind couldn't catch up on from the
transcript
([ADR 10.2](../adr/0010-bounded-resources.md#102-a-peer-cut-off-catches-up-from-the-transcript)).
`event watch` prints it on stdout as its last event; `event notify`, `event
wait` and the `--wait`s say it on stderr. They exit 3.

| Field | Type | Meaning |
|---|---|---|
| `peer` | string | The command's connection, as `subscribe` named it. |
| `after_seq` | number | The last event it got. |
| `until_seq` | number or null | Where it subscribed again, or, if that failed each time, where the process said it cut the command off. Null only for a process gone, or one that said no position, as `error` then says. |
| `error` | object | Why it couldn't catch up, `{code, message}`. |

It has `ts`, `host_id` and `pid`.

```json
{
  "event": "events_missed",
  "ts": "2026-10-23T15:31:09.004211Z",
  "host_id": "20261023T152005-4466",
  "pid": 4466,
  "peer": "socket#12",
  "after_seq": 4120,
  "until_seq": 4502,
  "error": {
    "code": "transcript_not_kept",
    "message": "the process keeps no transcript to read (log = \"off\")"
  }
}
```
