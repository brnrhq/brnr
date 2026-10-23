# Observing sessions

Watching a session is always allowed, a session an editor owns included:
`session status`, `event log`, `event watch` and `event notify` change
nothing. They all show the same
events, live or read back from the session's transcript.

## Status, and the story so far

```sh
brnr session status $s --json
brnr event log $s --json
brnr event log $s --last 1 --json
```

`session status` is one object: `state` (`idle`, `busy`, `waiting` for an
approval or an elicitation), `turn_elapsed`, `owner` (`headless` or
`editor`), `title`, `mode`, `model`, `tools` running, `plan`, `pending`
approvals, `elicitations` waiting, `held` messages, `context`,
`usage`, `last_turn` (`stop_reason`, `error`, `tokens`), `last_message`
(cut at 4000 characters), `cwd`, `forked_from` (the session a fork came
from, or null), `pid`, `stop_when_idle`. Use it to answer "what is it doing?", not to wait (use
`event wait` for that).

`event log` prints the session's events, one JSON object per line, and works on a
session that has ended too. A session resumed in another cwd has files in
each folder, and `event log` reads them all, in time order. `--last <n>`
starts at the n-th last message sent to the agent; `--follow` keeps printing
until the session closes.

## Live

```sh
brnr event watch $s --json
brnr event watch --pid 4466 --json
```

`event watch` prints events as they come, until the session closes (or,
with `--pid`, until the process exits). It runs until then: start it in the
background, or prefer `event wait` when what you want is the end of a turn.

## Choosing events

`--events` takes event names, `all`, and `default`, comma-separated, for
`event log`, `event watch` and `event notify` alike. Without it, `event log`
and `event watch` leave out the quiet ones: `acp`, `agent_thought`, `usage` and `tool_progress`.
It takes brnr's two notes too, which are shown whatever it chose:
`records_skipped`, a gap in the transcript, and `events_missed`, what a
command cut off couldn't catch up on. `--events records_skipped` alone lists
a transcript's gaps.

```sh
brnr event log $s --events default,agent_thought --json
brnr event watch $s --events turn_ended,permission_request --json
```

## The events

Every event has `event`, `ts`, `host_id` (the process's run) and `pid` (the
process); all but `process_exited`, `line_too_long` and `line_too_deep` have
`session`. Every
name is snake_case. Who acted is `by` (`editor`, `agent` or `control`), the
connection that asked `peer`, and why `reason`. As brnr prints them (`ts`,
`host_id`, `pid` and `seq` left out here):

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
{"event": "permission_resolved", "session": "0f6c…", "request": "p1", "answer": "allowed", "option_kind": "allow_once", "outcome": {"outcome": "selected", "optionId": "allow"}, "by": "control", "peer": "socket#18", "reason": null}
{"event": "elicitation_request", "session": "0f6c…", "request": "e1", "owner": "headless", "mode": "form", "text": "Which branch?", "scope": "session", "tool_call_id": null, "scope_request_id": null, "schema": {}, "url": null, "elicitation_id": null}
{"event": "elicitation_resolved", "session": "0f6c…", "request": "e1", "action": "accept", "content": {"branch": "main"}, "by": "control", "peer": "socket#18", "reason": null}
{"event": "elicitation_completed", "session": "0f6c…", "request": "e2", "elicitation_id": "login-1"}
{"event": "turn_ended", "session": "0f6c…", "by": "control", "messages": ["m1"], "stop_reason": "end_turn", "error": null, "tokens": {"total": 1234, "input": 1000, "output": 234, "thought": null, "cached_read": 500, "cached_write": null}}
{"event": "message_dropped", "session": "0f6c…", "message": "m4", "text": "also update the changelog", "reason": "cancel"}
{"event": "context_dropped", "session": "0f6c…", "text": "the API key is in .env.local", "reason": "close"}
{"event": "session_closed", "session": "0f6c…", "by": "control", "reason": "idle"}
{"event": "session_deleted", "session": "0f6c…", "by": "control"}
{"event": "history", "session": "0f6c…", "updates": 42, "recorded": true, "recorded_updates": 42, "loads": 1}
{"event": "process_exited", "exit_status": {"code": 0}}
```

- `user_message`: a message sent to the agent, `by` the `editor` or
  `control` (brnr's commands and bridges), with its id.
- `tool_call`: `started: true` when it starts; again when it ends, with
  `status` `completed` or `failed`.
- `session_changed`: `what` is `title`, `mode`, `config` or `commands`; for
  config options and commands `value` is a JSON merge patch by id or name.
- `permission_resolved`: `answer` is `allowed`, `rejected` or `cancelled`
  (null for an editor's answer of a kind brnr doesn't know), `option_kind`
  the chosen option's kind, `by` who answered (`editor` or `control`, or
  `agent` when the agent gave the request up itself, `$/cancel_request`),
  `peer` the connection that answered (`socket#18`, a bridge's name), and
  `reason` `cancel` or `timeout` when brnr answered for its turn's cancel or
  its time.
- `turn_ended`: `stop_reason` (`end_turn`, `cancelled`, …) or an `error`
  (`{code, message, rpc_error}`: `agent_failed`, and the agent's JSON-RPC
  error), the `messages` the turn carried, and `tokens`: the
  counts the agent reported for the turn (`total`, `input`, `output`,
  `thought`, `cached_read`, `cached_write`), each null where it gave none that
  is a count (a JSON integer that fits a `u64`; not `1e3`), and `tokens` null
  when the answer has no `usage` object. brnr adds nothing up; what the counts
  cover (subagents, earlier model calls) is the agent's to say. `usage` is the
  context window, not a turn's tokens.
- `message_dropped`: its `reason` `cancel`, `queue`, `close`, `exit` or
  `steer`; `context_dropped`: `queue`, `close` or `exit`. Each is something
  you sent that the agent never got.
- `session_closed`: `by` `control`, `reason` `close` or `idle`
  (`--stop-when-idle`); or `by` `editor`.
- `session_reopened`: `by` `editor`: the agent answered an editor's load
  of the session with success after it answered the session's close, so the
  session is open again (unless another process took it meanwhile: then
  it isn't, and there is none). A watcher has ended at the `session_closed`; watch
  it again to follow it.
- `session_deleted`: the agent deleted its copy of the session, `by`
  `control` (`brnr session delete`) or the `editor`. brnr's transcript stays,
  and `event log` still reads it.
- `history`: a `session/load` replayed the session's history (`updates`).
  `recorded: true` when brnr had no transcript of it: the history is in the
  log as events with `replayed: true` (`(replayed) user: …` in text);
  `false` when brnr's transcript had it already, which has only what
  happened through brnr. `loads` is how many loads it covers: more than 1
  when an editor loaded the session again before the agent answered, and
  then a recorded history is in the log once per load, each copy marked.
  `recorded_updates` is how many of the `updates` are in the log: all when
  `recorded`, none when not, fewer when the session was closed while it
  loaded (`history: 4 updates replayed by the agent, 3 recorded`); its
  `history` then follows a `session_reopened`.
- `line_too_long`: a line from the agent (or the editor) over 32 MiB, which
  brnr didn't read. Headless it was dropped (`relayed: false`): whatever it
  said, such as the answer to a turn, is lost. It is the process's.
- `line_too_deep`: a line from the agent (or the editor) nested past 128
  levels (`max_depth`), which brnr didn't read, as `line_too_long`.
  Headless it was dropped: a permission request in it reaches no one, and
  the turn waits until it is cancelled. It is the process's.
- `process_exited`: the agent's process ended, and brnr's with it
  (`exit_status`), with an `error` when brnr itself crashed. It is the
  process's, not one session's.
- `acp`: every ACP message, raw; only with `--events all` or by name.

## Notifications

`event notify` runs a command for each event: by default `permission_request`,
`turn_ended` and `process_exited`. The event is in the command's environment
(`BRNR_EVENT`, `BRNR_TEXT`, `BRNR_TITLE`, `BRNR_MESSAGE`, `BRNR_SESSION`,
`BRNR_REQUEST`, `BRNR_PID`) and, as JSON, on its stdin, never on its command
line. It runs until the session closes.

```sh
brnr event notify $s -- sh -c 'curl -s -d "$BRNR_TEXT" ntfy.sh/my-agents'
```

`--events` chooses others, as for `event watch` (`default` is the three
above).

Set one up only when the user asks for notifications; for your own waiting,
`event wait` is simpler. Bridges (programs that speak brnr's protocol on the
control socket) are in brnr's documentation:
[Bridges](https://brnrhq.github.io/brnr/docs/guide/profiles.html#bridges).
