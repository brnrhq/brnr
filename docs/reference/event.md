# event

A session's [events](events.md): read back from its transcript, watched
live, turned into notifications, or waited for.

`--events` chooses the events, in `event log`, `event watch` and `event
notify` alike: event names, `all` for every event, and `default` for the
command's own default, comma-separated (`--events default,agent_thought`).
Without it, `event log` and `event watch` show every event but the quiet
ones, `acp`, `agent_thought`, `usage` and `tool_progress`, in text and JSON
alike. It takes the names of brnr's two notes too, `records_skipped` and
`events_missed`, which are shown whatever it chose: `--events
records_skipped` alone shows only a transcript's gaps. A name that is
neither is a usage error.

## `brnr event log`

Reads a session's saved events, running or not, from its files in every
project folder, merged by time. A running session's are read once its
process has written what it recorded until then.

    brnr event log <session> [--last <n>] [--follow] [--events <default|all|event>,...] [--json]

| Flag | Meaning |
|---|---|
| `--last <n>` | Start at the `n`th last message sent to the agent (`user_message`), counted in what is merged; `0`: only what comes next. |
| `--follow` | Go on printing as the session's process writes, until the session closes or its process exits. |
| `--events <…>` | The events to show. With `acp`, the raw ACP files are read too, each message an `acp` event, merged in time order. |
| `--json` | Print one event per line, as JSON. |

**Output** (`--json`): one [event](events.md) per line, as the transcript
has it. A gap in the transcript is a [`records_skipped`](events.md#records_skipped)
note, shown whatever `--events` chose. A session whose process keeps no
transcript (`log = "off"`), or no raw ACP for `acp` (`log = "events"`), says
so.

Text output: the events, one or more lines each, with their times.

**Exit codes:** 0; 1 if it fails.
**Errors:** `session_not_found`, `session_opening`, `process_unreachable`,
`transcript_not_kept`, `transcript_unsupported`.

## `brnr event watch`

Shows a session's events, or a process's (`--pid`), live, until the session
closes or the process exits. A session's events are its own, and its
process's (the agent exiting).

    brnr event watch (<session> | --pid <pid>) [--events <default|all|event>,...] [--json]

| Flag | Meaning |
|---|---|
| `--pid <pid>` | Every session's events in this process, in place of `<session>`. |
| `--events <…>` | The events to show. |
| `--json` | Print one event per line, as JSON. |

**Output** (`--json`): one [event](events.md) per line. Cut off for falling
behind, it catches up from the transcript and goes on. Unable to, it prints
what it missed, an [`events_missed`](events.md#events_missed) note, as its
last line, and exits 3.

Text output: the events as `event log` shows them.

**Exit codes:** 0 when the session closes or the process exits; 3 when cut
off and unable to catch up; 1 if it fails.
**Errors:** [those of a running session](errors.md#a-running-session),
`process_not_found` (`--pid`).

## `brnr event notify`

Runs a command for each event of a session, or of every session in a process
(`--pid`), for notifications. With `--stdin` it reads the events, one per
line, from its stdin instead of connecting: a started bridge's events
([bridges](bridge.md)).

    brnr event notify (<session> | --pid <pid> | --stdin) [--events <default|all|event>,...] -- <command> [args...]

| Flag | Meaning |
|---|---|
| `--pid <pid>` | Every session's events in this process. |
| `--stdin` | Read the events from stdin, as a bridge started from a profile. |
| `--events <…>` | The events to run the command for. `default`, and the default without it: `permission_request`, `elicitation_request`, `turn_ended` and `process_exited`. |
| `-- <command> [args...]` | The command to run, as an argument vector. |

It has no `--json`: its stdout is its command's. The command gets the event,
whole, as JSON on its stdin, and these in its environment; nothing is put on
its command line, so what the agent writes can't become arguments:

| Variable | Meaning |
|---|---|
| `BRNR_EVENT` | The event's name. |
| `BRNR_TEXT` | The event as `event watch` shows it. |
| `BRNR_TITLE` | The session's title, else its id. |
| `BRNR_MESSAGE` | The agent's last message in the session. |
| `BRNR_SESSION` | The session's id. |
| `BRNR_REQUEST` | An approval's or an elicitation's handle; empty for other events. |
| `BRNR_PID` | The process. |

`BRNR_TEXT`, `BRNR_TITLE` and `BRNR_MESSAGE` are escaped, as `event watch`
shows them, and cut at 32 KiB; the JSON on stdin is whole. Commands run one
at a time, in order, each in a process group of its own.

- It exits when the process does, or the session closes.
- As a bridge, its `events`, if the profile limits them, must include
  `agent_message`, `session_changed` and `process_exited`, which it needs
  for its environment and to know when to stop.
- Cut off for falling behind, on the socket, it lets the command finish,
  then catches up, running the command for each event it missed, as `event
  watch` does. Unable to, it says what it missed on stderr and exits 3. A
  started bridge doesn't catch up: its stdin is closed, and it is sent
  SIGTERM.
- Its connection ending otherwise first, or stopped by a signal (HUP, INT,
  QUIT, TERM), it stops the command it is running (SIGTERM to its group,
  SIGKILL 2 s later), says on stderr that no more notifications come, and
  exits non-zero.

See [notifications](../guide/events.md#notifications).

**Exit codes:** 0 when the process exits or the session closes; 3 when cut
off and unable to catch up; non-zero when its connection ends otherwise, or
a signal stops it.
**Errors:** [those of a running session](errors.md#a-running-session),
`process_not_found` (`--pid`).

## `brnr event wait`

Waits for a condition of a session. It returns at once if the condition holds
already, so `prompt send` followed by `event wait` can't race.

    brnr event wait <session> [--for idle|turn|permission|elicitation|exit] [--timeout <s>] [--json]

| Flag | Meaning |
|---|---|
| `--for idle` | The default: no turn running, nothing held, and no approval or elicitation waiting. A session that closes counts as idle. |
| `--for turn` | The next turn that ends. |
| `--for permission` | An approval is waiting. |
| `--for elicitation` | An elicitation is waiting. |
| `--for exit` | The process exits. |
| `--timeout <s>` | Give up after this many seconds, exit 124. It doesn't cancel the turn. |
| `--json` | Print what met the condition as JSON. |

**Output** (`--json`): one shape for each `--for`, whatever ended the wait,
and whether it held when the wait began:

- `idle`: the session's state, the table below.
- `turn`: the [`turn_ended`](events.md#turn_ended) event, as `event watch
  --json` prints it.
- `permission`: the approval waiting, as
  [`permission show --json`](permission.md#brnr-permission-show) prints it.
  The event of one answered before brnr asks for it doesn't end the wait:
  none is waiting.
- `elicitation`: the elicitation waiting, as
  [`elicitation show --json`](elicitation.md#brnr-elicitation-show) prints
  it, the same way.
- `exit`: the [`process_exited`](events.md#process_exited) event. A process
  gone without sending it (killed) gets the same event from the command, as
  far as it knows it: `exit_status` and `seq` null, `ts` when it saw the
  process go, and `error` saying so. Only once brnr has seen the process
  exit: a connection that fails while it lives is a failure, 1, saying brnr
  can't tell that it exited.

| Field | Type | Meaning |
|---|---|---|
| `session` | string | The session. |
| `state` | string | `idle`, or `inactive` once it has closed, as `session list` has it. |
| `last_turn` | object or null | How its last turn ended, as `session status` has it; null before the first, and for one that has closed. |
| `last_turn.stop_reason` | string or null | As [`turn_ended`](events.md#turn_ended) has it. |
| `last_turn.error` | object or null | As `turn_ended` has it. |
| `last_turn.tokens` | object or null | As `turn_ended` has it. |

<!-- example: brnr event wait $s --json; before: Idle -->
```json
{
  "session": "sess-1",
  "state": "idle",
  "last_turn": {
    "stop_reason": "end_turn",
    "error": null,
    "tokens": null
  }
}
```

Text output: `idle: end_turn`, `inactive: session closed (idle)`, `turn
ended: end_turn`, `approval p1: Edit src/lib.rs`, `elicitation e1: Tell me
about you`, `exited`.

**Exit codes:**

- 0 when the condition is met and, for `idle` and `turn`, the last turn
  ended normally (`end_turn`). On a session that is idle already, it exits
  as its last turn ended.
- 1 if that turn failed or stopped for another reason, if its message was
  dropped, if the agent exited first (but for `exit`), or (for `turn`,
  `permission` and `elicitation`) if the session closed first.
- 124 when `--timeout` passes.
- 3 when cut off and unable to catch up, what it missed said on stderr.

A process whose status lacks a count this needs (an older brnr's) fails the
wait, saying which: a count that isn't there isn't 0.
**Errors:** [those of a running session](errors.md#a-running-session),
`session_closed`, `agent_exited`, `command_unknown`.
