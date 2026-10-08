# 16. Fork and close

Accepted (former decision 11); reviewed 2026-10-07. Implemented; on an
editor's session `close` is ADR 4's experimental action.

## Decision

- `brnr fork <session>` forks the session (`session/fork`) into a new one in
  the same process and prints its id. Refused on an editor's process
  (ADR 4). `session/fork` is unstable in ACP v1: a convention both adapters
  implement, used by default and refused in strict mode (ADR 41). A plain
  "new session in this process" isn't offered: `brnr start` is the way to get
  one.
- `brnr close <session>` cancels a running turn and closes that one session
  (`session/close`); sessions beside it in the process keep running. A
  headless process with no session left stops. On an editor's session it is
  the experimental `close` (ADR 4). `start --resume --take-over` closes the
  same way (ADR 3).
- Messages still held in a closed session are reported dropped, then
  `session_closed` (ADR 20).
- Both are refused when the agent can't (no `fork` or `close` session
  capability) (P7), and `fork` when `stop_when_idle` is set and the agent
  can't close sessions (ADR 12).

## Considered

- `fork` and `close` refused outright while an editor is attached (former
  decision 11): `close` is possible with compensation (ADR 4); `fork` stays
  refused.
- Closing by stopping the whole process: it would take sibling sessions with
  it.

## Tests

Run `cargo test --release adr_0016_`. Named claims and their assertions:

- [tests/cli.rs](../../tests/cli.rs)
  - `adr_0016_fork_and_close`.
  - `adr_0016_close_cancels_the_turn_first`.
