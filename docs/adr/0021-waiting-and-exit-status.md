# 21. Waiting, and exit status

Accepted (former decisions 5, 21 and 44); reviewed 2026-10-07. Implemented,
except ending on `session_closed` and `message_dropped` (ADR 20), `dropped`
in the JSON, and `start --wait` reading the start channel (ADR 7).

## Decision

- `brnr wait <session> [--for idle|turn|permission|exit] [--timeout <s>]`:
  - `idle` (the default): no turn running and nothing held. It returns at
    once if the session is idle already, so `send` followed by `wait` can't
    race. A closed session counts as idle.
  - `turn`: the next turn that ends.
  - `permission`: an approval is waiting (at once if one already is).
  - `exit`: the process exits.
- Exit status: 0 when the condition is met and the last turn ended normally
  (`end_turn`); 1 if that turn failed or stopped for another reason, if its
  message was dropped, or (for `turn` and `permission`) if the session
  closed first; 124 on `--timeout`, as timeout(1) does. The same codes for
  `send --wait` and `start --wait`. `wait` on a session that is idle already
  exits as its last turn ended.
- While they wait, approvals are announced on stderr.
- `start --wait` prints `started …` on stderr, so that stdout is the agent's
  reply and nothing else (`brnr start --wait … > answer.md`).
- With `--json`, `--wait` prints the turn as one object at its end:
  `{session, message, reply, stop_reason, error, dropped}`, and for `start`
  its `pid`.

## Considered

- A command per condition (`wait-idle`, …): one command with `--for` is
  smaller.
