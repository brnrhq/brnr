# Observing sessions

Watching a session is always allowed, a session an editor owns included:
`status`, `log`, `watch` and `notify` change nothing. They all show the same
events, live or read back from the session's transcript.

## Status, and the story so far

```sh
brnr status $s --json
brnr log $s --json
brnr log $s --last 1 --json
```

`status` is one object: `state` (`idle`, `busy`, `waiting` for an approval),
`turn_seconds`, `owner` (`headless` or `editor`), `title`, `mode`, `model`,
`tools` running, `plan`, `pending` approvals, `held` messages, `context`,
`usage`, `last_message` (cut at 4000 characters), `cwd`, `pid`,
`stop_when_idle`. Use it to answer "what is it doing?", not to wait (use
`wait` for that).

`log` prints the session's events, one JSON object per line, and works on a
session that has ended too. `--last <n>` starts at the n-th last message sent
to the agent; `--follow` keeps printing until the session closes.

## Live

```sh
brnr watch $s --json
brnr watch --pid 4466 --json
```

`watch` prints events as they come, until the session closes (or, with
`--pid`, until the process exits). It runs until then: start it in the
background, or prefer `wait` when what you want is the end of a turn.

## Choosing events

`--events` takes event names, `all`, and `default`, comma-separated, for
`log`, `watch` and `notify` alike. Without it, `log` and `watch` leave out
the quiet ones: `acp`, `agent_thought`, `usage` and `tool_progress`.

```sh
brnr log $s --events default,agent_thought --json
brnr watch $s --events turn_ended,permission_request --json
```

## The events

Every event has `event`, `ts` and `host_id`; all but `exited` have
`session`. As brnr prints them:

```json
{"event": "user_message", "session": "0f6c…", "by": "control", "message": "m1", "text": "fix the failing tests"}
{"event": "agent_message", "session": "0f6c…", "text": "All tests pass now."}
{"event": "agent_thought", "session": "0f6c…", "text": "…"}
{"event": "tool_call", "session": "0f6c…", "tool_call_id": "t1", "title": "Run the tests", "kind": "execute", "status": "pending", "locations": [], "started": true}
{"event": "tool_progress", "session": "0f6c…", "tool_call_id": "t1", "title": "Run the tests", "kind": "execute", "status": "in_progress", "locations": []}
{"event": "plan", "session": "0f6c…", "entries": [{"content": "Run the tests", "status": "in_progress", "priority": "high"}]}
{"event": "usage", "session": "0f6c…", "usage": {"used": 12345, "size": 200000, "cost": {"amount": 0.42, "currency": "USD"}}}
{"event": "session_changed", "session": "0f6c…", "what": "title", "value": "Fix the failing tests"}
{"event": "permission_request", "session": "0f6c…", "request": "p1", "owner": "headless", "title": "Edit src/lib.rs", "kind": "edit", "tool_call": {}, "options": []}
{"event": "permission_resolved", "session": "0f6c…", "request": "p1", "outcome": {"outcome": "selected", "optionId": "allow"}, "by": "socket#18"}
{"event": "turn_ended", "session": "0f6c…", "by": "control", "messages": ["m1"], "stop_reason": "end_turn", "error": null}
{"event": "message_dropped", "session": "0f6c…", "message": "m4", "text": "also update the changelog", "by": "cancel"}
{"event": "context_dropped", "session": "0f6c…", "text": "the API key is in .env.local", "by": "close"}
{"event": "session_closed", "session": "0f6c…", "by": "idle"}
{"event": "exited", "status": {"code": 0}}
```

- `user_message`: a message sent to the agent, `by` the `editor` or
  `control` (brnr's commands and bridges), with its id.
- `tool_call`: `started: true` when it starts; again when it ends, with
  `status` `completed` or `failed`.
- `session_changed`: `what` is `title`, `mode`, `config` or `commands`; for
  config options and commands `value` is a JSON merge patch by id or name.
- `turn_ended`: `stop_reason` (`end_turn`, `cancelled`, …) or an `error`
  (`{code, message}`), and the `messages` the turn carried.
- `message_dropped`: `by` `cancel`, `queue`, `close`, `exit` or `steer`;
  `context_dropped`: `by` `queue`, `close` or `exit`. Each is something you
  sent that the agent never got.
- `session_closed`: `by` `close`, `idle` (`--stop-when-idle`) or `editor`.
- `exited`: the agent's process ended (`status`), with a `reason` when brnr
  itself crashed. It is the process's, not one session's.
- `acp`: every ACP message, raw; only with `--events all` or by name.

## Notifications

`notify` runs a command for each event: by default `permission_request`,
`turn_ended` and `exited`. The event is in the command's environment
(`BRNR_EVENT`, `BRNR_TEXT`, `BRNR_TITLE`, `BRNR_MESSAGE`, `BRNR_SESSION_ID`,
`BRNR_REQUEST`, `BRNR_PID`) and, as JSON, on its stdin, never on its command
line. It runs until the session closes.

```sh
brnr notify $s -- sh -c 'curl -s -d "$BRNR_TEXT" ntfy.sh/my-agents'
```

`--events` chooses others, as for `watch` (`default` is the three above).

Set one up only when the user asks for notifications; for your own waiting,
`wait` is simpler. Bridges (programs that speak brnr's protocol on the
control socket) are in brnr's README.
