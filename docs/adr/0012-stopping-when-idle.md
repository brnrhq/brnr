# 12. Stopping when idle

Accepted (former decisions 14 and 41); reviewed 2026-10-07. Implemented.

## Context

A headless session started for one task should end when the task is done,
without anyone having to stop it.

## Decision

- `stop_when_idle = <seconds>` in a profile's headless part, or
  `start --stop-when-idle <s>` (also with `--foreground`): a session idle that
  long closes (`session/close`), and the process stops with its last session:
  the last one isn't closed first, the process just stops.
- A session is idle while no turn is running, nothing is held and no approval
  is waiting. A message or prompt starts the count again. It counts from the
  start too, so a start without a prompt doesn't run forever; a start with a
  prompt sends it at the commit (ADR 7), so the session is busy at once. `0`
  is as soon as it is idle.
- Never while an editor is attached.
- Closing emits `session_closed` with `by: idle` (ADR 20). The last session,
  stopping the process, gets `exited` instead, which isn't also
  `session_closed`.
- With `stop_when_idle` set and an agent that can't close sessions, `fork` is
  refused (P7): a second session could never close when idle, and the
  process would never stop.

## Considered

- A switch (`stop_when_idle = true`, `--stop-when-idle`; former decision 14):
  seconds replaced it, and `0` is what the switch did.
- Not counting before the first turn (former decision 14): a start whose
  prompt never came ran forever. Later, counting only once the prompt
  arrived (`--awaiting-prompt`, former decision 41) did the same when
  `start` died before sending it; ADR 7 removes that gap.
- Keeping, silently, a session that ran out while the process has others,
  when the agent can't close sessions (as was): lost without a word (P3).

## Tests

Run `cargo test --release adr_0012_`. Named claims and their assertions:

- [tests/cli.rs](../../tests/cli.rs)
  - `adr_0012_fork_is_refused_when_it_could_never_close`.
  - `adr_0012_stop_when_idle`.
