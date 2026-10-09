# 21. Waiting, and exit status

Accepted (former decisions 5, 21 and 44); reviewed 2026-10-07. Implemented.

## Decision

- `brnr wait <session> [--for idle|turn|permission|exit] [--timeout <s>]`:
  - `idle` (the default): no turn running and nothing held. It returns at
    once if the session is idle already, so `send` followed by `wait` can't
    race. A closed session counts as idle.
  - `turn`: the next turn that ends.
  - `permission`: an approval is waiting (at once if one already is).
  - `exit`: the process exits.
- Exit status: 0 when the condition is met and, for `idle` and `turn`, the
  last turn ended normally (`end_turn`); 1 if that turn failed or stopped
  for another reason, if its message was dropped, if the agent exited first
  (but for `exit`), or (for `turn` and `permission`) if the session closed
  first; 124 on `--timeout`, as timeout(1) does. The same codes for
  `send --wait` and `start --wait`. `wait` on a session that is idle already
  exits as its last turn ended.
- While `send --wait` and `start --wait` wait, approvals are announced on
  stderr, with how to answer them.
- `start --wait` prints `started …` on stderr, so that stdout is the agent's
  reply and nothing else (`brnr start --wait … > answer.md`).
- With `--json`, `--wait` prints the turn as one object at its end:
  `{session, message, reply, stop_reason, error, dropped}`, and for `start`
  its `pid`.

## Considered

- A command per condition (`wait-idle`, …): one command with `--for` is
  smaller.

## Tests

Run `cargo test --release adr_0021_`. Named claims and their assertions:

- [tests/cli.rs](../../tests/cli.rs)
  - `adr_0021_send_wait_prints_the_reply`.
  - `adr_0021_start_wait_prints_the_reply_and_the_turns_result`.
  - `adr_0021_send_wait_times_out`.
  - `adr_0021_send_wait_reports_a_permission_request`.
  - `adr_0021_wait_behind_exits_as_the_last_turn_ended`.
  - `adr_0021_wait_returns_when_the_session_goes_idle`.
  - `adr_0021_wait_on_an_idle_session_reports_the_last_turn`.
  - `adr_0021_huge_timeouts_are_never`.
  - `adr_0021_wait_for_permission`.
  - `adr_0021_wait_for_exit`.
