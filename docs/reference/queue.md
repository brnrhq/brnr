# queue

What a session holds for later: the messages sent while a turn runs, which
go one at a time as turns end, and the context for the next prompt.

The queue commands print the same object: what is held after the command,
and what it dropped.

| Field | Type | Meaning |
|---|---|---|
| `session` | string | The session. |
| `held` | list of objects | The held messages, in the order they go: interrupts first. |
| `held[].message` | string | The message's id. |
| `held[].text` | string | Its text. |
| `held[].interrupt` | boolean | Whether it is an interrupt, which goes ahead of the rest. |
| `held[].attachments` | number | How many attachments it has. |
| `context` | list of strings | The held context entries' texts, in order. |
| `dropped` | list of objects | The messages the command dropped, each `{message, text, reason}`, `reason` `queue`. |
| `dropped[].reason` | string | `queue`. |

Dropping a held message or context is an event too:
[`message_dropped`](events.md#message_dropped) and
[`context_dropped`](events.md#context_dropped).

## `brnr queue list`

Shows the held messages and context.

    brnr queue list <session> [--json]

<!-- example: brnr prompt send $s --prompt later && brnr queue list $s --json; before: Turn -->
```json
{
  "session": "sess-1",
  "held": [
    {
      "message": "m2",
      "text": "later",
      "interrupt": false,
      "attachments": 0
    }
  ],
  "context": [],
  "dropped": []
}
```

Text output: `m2 (after turn): later` for each, then the context;
`nothing held` when there is none.

**Exit codes:** 0; 1 if it fails.
**Errors:** [those of a running session](errors.md#a-running-session).

## `brnr queue show`

Shows one held message in full: its id, whether it interrupts, its text, and
its attachments as ACP content blocks.

    brnr queue show <session> <message> [--json]

**Output** (`--json`):

| Field | Type | Meaning |
|---|---|---|
| `session` | string | The session. |
| `message` | string | The message's id. |
| `text` | string | Its text. |
| `interrupt` | boolean | Whether it is an interrupt. |
| `attachments` | number | How many attachments it has. |
| `blocks` | list of objects | Its attachments, as the ACP content blocks that go with the prompt: a file's `resource_link` (`uri`, `name`), an image's `image` (`mimeType`, `data`, base64). |

<!-- example: brnr prompt send $s --prompt later --file src/api.rs && brnr queue show $s m2 --json; before: Turn -->
```json
{
  "session": "sess-1",
  "message": "m2",
  "text": "later",
  "interrupt": false,
  "attachments": 1,
  "blocks": [
    {
      "type": "resource_link",
      "uri": "file:///home/me/project/src/api.rs",
      "name": "api.rs"
    }
  ]
}
```

Text output: the message, then `file: <uri>` or `image: <type>, <size>
bytes` for each attachment.

**Exit codes:** 0; 1 if it fails: a message not held fails.
**Errors:** [those of a running session](errors.md#a-running-session),
`message_not_found`.

## `brnr queue drop`

Drops one held message, by its id.

    brnr queue drop <session> <message> [--json]

**Output** (`--json`): the queue object above, with the message in `dropped`
(`reason` `queue`).

<!-- example: brnr prompt send $s --prompt later && brnr queue drop $s m2 --json; before: Turn -->
```json
{
  "session": "sess-1",
  "held": [],
  "context": [],
  "dropped": [
    {
      "message": "m2",
      "text": "later",
      "reason": "queue"
    }
  ]
}
```

Text output: `dropped m2: later`, then what is held.

**Exit codes:** 0; 1 if it fails.
**Errors:** [those of a running session](errors.md#a-running-session),
`message_not_found`.

## `brnr queue clear`

Drops the held messages, the held context, or with neither flag both.

    brnr queue clear <session> [--messages] [--context] [--json]

| Flag | Meaning |
|---|---|
| `--messages` | Drop the held messages. |
| `--context` | Drop the held context. On an editor's session, its profile must enable the experimental `context`. |
| `--json` | Print the result as JSON. |

**Output** (`--json`): the queue object above, with the messages dropped in
`dropped`. Context dropped isn't in it: its `context_dropped` events say so.

<!-- example: brnr prompt send $s --prompt later && brnr queue clear $s --json; before: Turn -->
```json
{
  "session": "sess-1",
  "held": [],
  "context": [],
  "dropped": [
    {
      "message": "m2",
      "text": "later",
      "reason": "queue"
    }
  ]
}
```

Text output: `dropped m2: later` for each, then what is held.

**Exit codes:** 0; 1 if it fails.
**Errors:** [those of a running session](errors.md#a-running-session),
`experimental_disabled`.
