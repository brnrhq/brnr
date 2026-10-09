# 48. `log` of a running session waits for its transcript

Proposed 2026-10-08. Implemented. Amends ADR 22 (what `log` shows of a
running session) and ADR 35 (a request, `logged`).

## Context

A process's records are written by a thread of its own (src/log.rs), so the
session never waits for the disk (P1, ADR 6). `brnr log` read the files as
they were, and what a command had just seen happen could be missing:
`brnr start --wait` printed the turn's reply, and `brnr log` right after
didn't have the turn, or failed because the session's events file wasn't
there yet (`…/sess-1.jsonl: No such file or directory`). Nothing said so
(P3). It showed as tests that failed now and then on CI's Linux runners,
where the logger thread can be a whole turn behind when `start --wait`
returns: on a 7-CPU Linux machine running 8 at a time, about 1 in 15
`start --wait`s returned before the turn was written, some before the
session's events file existed.

The same holds for a process that is killed (SIGKILL) just after: what it
hadn't written is lost, `list --all` doesn't have the session, and
`start --resume` of it without an agent fails with `no agent: give one after
-- or set agent in the profile` (ADR 14: an id brnr has no transcript of goes
to the agent given). That can't be helped without writing on the session's
own thread.

A process that exited stopped being listed, and let go of its sessions,
before its logger had written its last records, `exited` among them:
`transcripts_are_two_files` found a transcript without `exited` once the
process was gone, in a Linux container under load.

## Decision

- A request, `logged`, is answered once the process's logger has written
  everything the process recorded before it was asked, with the note of a
  gap of skipped records (ADR 6) that ends there. The answer comes from the
  logger's thread; the event loop doesn't wait for it (P1). With `log =
  false` it is answered at once.
- `log` of a running session sends it first, and reads the files once it is
  answered: what `start --wait`, `send --wait` or `wait` has reported is in
  what it shows. A process that doesn't answer within 5 s (a stalled disk)
  is shown as far as it has written, and `log` says so on its stderr (P3).
  `--follow` waits the same way before its first read.
- A process that exits waits, before brnr stops listing it and it lets go
  of its sessions, until its logger has written what it recorded, `exited`
  last, for 2 s at most: once it is gone, `log`, `list --all` and
  `--resume` read its sessions' transcripts whole, and a resume doesn't
  start appending to one before its `exited`.
- What a process killed with SIGKILL hadn't written is lost, as before.

## Consequences

- `log` of a running session costs a round trip to its process, and up to
  5 s when the disk is stalled.
- Tests that read a transcript, or kill a process whose transcript they
  need, wait for it the same way, by running `brnr log`.

## Considered

- Answering `start --wait` and `send --wait` only once their turn is
  written: every waiting command would wait for the disk, and a stalled one
  would stall them, which ADR 6 rules out for observers.
- Writing records on the event loop: the session would wait for the disk
  (P1, ADR 6).
- Waiting in the tests only (for a record to appear in the file): every
  user's script that runs `log` after `--wait` would have the same race.

## Tests

Run `cargo test --release adr_0048_`. Named claims and their assertions:

- [tests/headless.rs](../../tests/headless.rs)
  - `adr_0048_an_exit_is_written_before_the_process_goes`.
- [tests/cli.rs](../../tests/cli.rs)
  - `adr_0048_log_shows_what_the_process_has_recorded`.
