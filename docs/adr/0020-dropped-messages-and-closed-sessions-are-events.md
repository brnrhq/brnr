# 20. Dropped messages and closed sessions are events

Accepted 2026-10-07. Implemented.
Resolves review item 5.

## Context

Held messages dropped by `cancel`, by `queue --drop` or `--clear`, or by
closing their session went without an event: `cancel` wrote a transcript
note, `queue` nothing, so a `send --wait` for such a message waited until the
process exited. Closing a session (`brnr close`, the idle timeout with other
sessions open) emitted nothing at all: `watch`, `notify` and `wait` on it
waited for the whole process. Messages still held when the agent exited were
an `undelivered` list in `exited`.

## Decision

- `message_dropped {session, message, text, by}`, `by` one of `cancel`,
  `queue`, `close`, `exit`, and `steer` (a steer the agent neither took nor
  sent back, ADR 18): every way a message brnr accepted can go unsent (P3).
  Unanswered steers are dropped with held messages by `cancel`, a close and
  the exit. `exited` loses `undelivered` (P5, P9).
- `context_dropped {session, text, by}`, `by` one of `queue`
  (`queue --clear-context`), `close` and `exit`: context held for a next
  prompt that won't come, one event per item (context has no message id,
  ADR 17). `cancel` keeps context: it is for the next prompt, whenever that
  comes. Added 2026-10-08: context used to go without a word when its session
  closed (`stop_when_idle` included) or the process exited.
- `session_closed {session, by}`, `by` one of `close` (`brnr close`,
  `--take-over`), `idle` (`stop_when_idle`) and `editor` (the editor's own
  `session/close`). Messages and context still held in the session are
  dropped first. A
  process exiting is `exited`, which is already in every session's file, and
  isn't also `session_closed`.
- `send --wait` and `start --wait` whose message is dropped exit 1, saying
  `m3 was dropped (cancel)` on stderr and `dropped: "cancel"` in `--json`.
  `wait` ends as ADR 21 says; `watch` and `notify` on a session end when it
  closes.
- All three are in `EVENTS` and the README's list for bridges, and in the default
  events of `watch`, `log` and bridges (ADR 23). `notify`'s default stays its
  three (ADR 36).

## Considered

- `turn_ended` with a `dropped` stop reason: misleading, since no turn ran.
- Only the response to the command that dropped them saying so (as was).
- A distinct exit status for "never ran": one more code for scripts to
  learn; the difference is in stderr and `--json`.
- Keeping `undelivered` in `exited`: two kinds of record for one thing.
