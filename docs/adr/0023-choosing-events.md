# 23. Choosing events, and what text shows

Accepted (former decisions 2, 18, 19 and 34); reviewed 2026-10-07.
Implemented. `usage` has the context window and the cost, not tokens in and
out, so its line is `usage: 12.3k of 200.0k tokens, cost 0.42 USD`.

## Context

`watch` first subscribed to everything, `acp` included: one raw line per
streamed chunk. Then `log --raw` meant something other than for `watch`,
`--thoughts` chose an event that was already chosen, and text dropped events
JSON had (`--events agent_thought` printed nothing without `--thoughts`).
Some still do: `render::event` prints nothing for `usage`, for
`session_changed` about config or commands, or for `tool_call` status
changes between the start and the end, so `brnr watch $s --events usage`
prints nothing while `--json` has them.

## Decision

- `--events` alone chooses, for `watch`, `log` and `notify` alike: event
  names, `all`, and `default`. An event chosen is shown, in text and JSON
  alike (P5). Unknown names fail in the CLI, before connecting, with the same
  message everywhere.
- Without `--events`, `watch` and `log` show every event but the quiet ones,
  `acp`, `agent_thought`, `usage` and `tool_progress` (`QUIET`), in text,
  JSON and the foreground alike. `default` names that set, so
  `--events default,agent_thought` adds thoughts. `--raw` and `--thoughts`
  are gone.
- Bridges and the host's `subscribe` keep their own default: every event but
  `acp`. `notify`'s `default` is its own (ADR 36).
- Tool calls: `tool_call` when a call starts (`started: true`) and when it
  ends (`completed` or `failed`), shown as `tool: … (kind)`,
  `tool done: …`, `tool failed: …`; `tool_progress` for each change of status
  in between (`in_progress`), quiet, shown as `tool: … in_progress` when
  chosen. Progress that doesn't change the status (streamed output) is not
  an event.
- `usage` (one per turn; `status` has the latest) is quiet, and shown as a
  line (`usage: 12k in, 3k out`) when chosen. `session_changed` is shown for
  everything it reports: title, mode, config (`config: model=opus`) and
  commands (`commands: 14 available`).

## Considered

- `watch` showing everything by default, `acp` included: the firehose should
  be one flag away, not the default.
- A text line for every tool status change: noise.
- No event for status changes between start and end: bridges would lose
  `in_progress`.
- Text quietly leaving out events that JSON has: what `--events` chose
  wouldn't be what was shown.
