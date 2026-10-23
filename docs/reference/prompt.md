# prompt

What is sent to a session's agent: prompts, steers, interrupts and context,
and the turn's cancellation.

## `brnr prompt send`

Sends a prompt, with attachments, to one session. A prompt sent while a turn
runs is held until it ends: a second prompt never goes while one runs.

    brnr prompt send <session> [--steer | --interrupt | --context [--replace]]
                     [--prompt <text> | -] [--file <path>]... [--image <path>]...
                     [--wait [--timeout <s>]] [--json]

| Flag | Meaning |
|---|---|
| `--steer` | Put the text into the running turn (`_session/steering`), where the agent advertises it. With no turn running, it is sent as a prompt. Not in strict mode, nor into an editor's turn. |
| `--interrupt` | Cancel the running turn, and send this ahead of the held prompts. |
| `--context` | Add the text to the next prompt, as context. Text only. |
| `--replace` | With `--context`: replace the last context entry. |
| `--prompt <text>`, `--prompt -`, `--file <path>`, `--image <path>` | The prompt and its attachments, as for [`session new`](session.md#prompt-flags). One of them is needed. |
| `--wait` | Wait for the turn to end, and print the agent's reply on stdout. Not with `--context`, which starts no turn. |
| `--timeout <s>` | With `--wait`: give up waiting after this many seconds, exit 124. It doesn't cancel the turn. |
| `--json` | Print the result as JSON. |

`--steer`, `--interrupt` and `--context` are modes, not flags to combine:
two of them are a usage error, and so is `--replace` without `--context`.
With `--wait`, approvals and elicitations on the way are said on stderr,
with how to answer them. An accepted send is not a turn that has ended:
match the message id with [`turn_ended`](events.md#turn_ended)'s `messages`
or [`message_dropped`](events.md#message_dropped)'s `message`.

**Output** (`--json`, without `--wait`):

| Field | Type | Meaning |
|---|---|---|
| `status` | string | `delivered` (sent now), `held` (until the running turn ends; context is always `held`), `steered` (into the running turn) or `interrupting` (the running turn is being cancelled; this goes next). |
| `session` | string | The session. |
| `message` | string | The message's id (`m1`), which its turn's `user_message` and `turn_ended` carry. Not there for `--context`, which has none. |

<!-- example: brnr prompt send $s --json --prompt "also update the changelog"; before: Turn -->
```json
{
  "status": "held",
  "session": "sess-1",
  "message": "m2"
}
```

**Output** (`--wait --json`): one object, once the turn has ended. A command
can fail before there is one.

| Field | Type | Meaning |
|---|---|---|
| `session` | string | The session. |
| `message` | string | The message's id. |
| `reply` | string | The agent's messages in the turn, joined by a blank line. |
| `stop_reason` | string or null | ACP's stop reason; null if the turn failed or the message was dropped. |
| `error` | object or null | The turn's failure, `{code, message}`, `agent_failed` with the agent's as `rpc_error`; null if it didn't fail. |
| `tokens` | object or null | What the agent reported the turn used, as [`turn_ended`](events.md#turn_ended) has it; null if it reported nothing. |
| `dropped` | list of objects | Empty, or the message, dropped before it was sent, `{message, text, reason}`. |

<!-- example: brnr prompt send $s --wait --json --prompt "reply done"; before: Idle -->
```json
{
  "session": "sess-1",
  "message": "m2",
  "reply": "done",
  "stop_reason": "end_turn",
  "error": null,
  "tokens": null,
  "dropped": []
}
```

Text output: `delivered (message m2)`, `held (message m2)`, …; with `--wait`,
the agent's reply.

**Exit codes:** 0 once sent (with `--wait`: once its turn has ended with
`end_turn`); with `--wait`, 1 if the turn failed, stopped otherwise or its
message was dropped, 124 when `--timeout` passes, 3 when cut off and unable
to catch up; 1 if it fails; 2 for a usage error.
**Errors:** [those of a running session](errors.md#a-running-session),
`input_invalid`, `capability_missing` (steering, images),
`strict_mode_refused`, `turn_owned_by_editor`, `experimental_disabled`,
`process_stopping`, `session_closed` and `agent_exited` (with `--wait`).

## `brnr prompt cancel`

Cancels the running turn (`session/cancel`). The held messages are dropped,
unless `--keep-held`: they would otherwise start the next turn. The held
context stays.

    brnr prompt cancel <session> [--keep-held] [--json]

| Flag | Meaning |
|---|---|
| `--keep-held` | Keep the held messages. |
| `--json` | Print the result as JSON. |

**Output** (`--json`):

| Field | Type | Meaning |
|---|---|---|
| `status` | string | `cancelling` (a turn runs, and is cancelled) or `idle` (none runs). |
| `session` | string | The session. |
| `dropped` | list of objects | The messages dropped, each `{message, text, reason}`, `reason` `cancel`. |
| `dropped[].message` | string | The message's id. |
| `dropped[].text` | string | Its text. |
| `dropped[].reason` | string | `cancel`. |

<!-- example: brnr prompt send $s --prompt later && brnr prompt cancel $s --json; before: Turn -->
```json
{
  "status": "cancelling",
  "session": "sess-1",
  "dropped": [
    {
      "message": "m2",
      "text": "later",
      "reason": "cancel"
    }
  ]
}
```

Text output: the status, then `dropped m2: later` for each message dropped.

**Exit codes:** 0; 1 if it fails.
**Errors:** [those of a running session](errors.md#a-running-session),
`experimental_disabled`.

## `brnr prompt commands`

Lists the slash commands the agent has advertised (ACP's
`available_commands_update`). Invoke one by sending its text as a prompt.

    brnr prompt commands <session> [--json]

**Output** (`--json`):

| Field | Type | Meaning |
|---|---|---|
| `session` | string | The session. |
| `commands` | list of objects | One for each command. |
| `commands[].command` | string | Its name, without the `/`. |
| `commands[].hint` | string or null | What its input is, as the agent says (`input.hint`); null if it takes none. |
| `commands[].description` | string | What it does, as the agent says. |

<!-- example: brnr prompt commands $s --json; before: Idle -->
```json
{
  "session": "sess-1",
  "commands": [
    {
      "command": "compact",
      "hint": null,
      "description": "Compact the conversation"
    }
  ]
}
```

Text output: `/compact  Compact the conversation`, one line for each.

**Exit codes:** 0; 1 if it fails.
**Errors:** [those of a running session](errors.md#a-running-session).
