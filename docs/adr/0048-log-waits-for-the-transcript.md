# 48. `log` of a running session waits for its transcript

Draft 2026-10-08. Implemented. Amends ADR 22 (what `log` shows of a
running session) and ADR 35 (a request, `logged`).
Amended 2026-10-12 (#161): a session closed in a process is let go of,
and its close answered, once its records are written, as on exit, for 2 s
at most; meanwhile the process's `status` lists it as `closing`, `log`
waits for its records, and a resume is told it is closing there.
Amended 2026-10-14 (#193): a session whose open failed after the process
recorded something of it (a load's replay) is let go of, and the open
answered, the same way.
Amended 2026-10-16 (#165): a process that exits then waits for the rest
of its records without a time limit, as it does for the display's reader;
a signal gives up on them, and the process says on stderr how much it
didn't write. A panic lets go of the process's sessions as an exit does,
before it waits.

Amended by 69: `logged` says `lost` when a file has lost records it
hasn't noted yet.

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
  start appending to one before its `exited`. A panic does the same.
  Then, unlisted and holding no session, it waits for the rest without a
  time limit, as for the display's reader in the foreground (ADR 9), and
  exits once it is written. A signal to it (Ctrl-C, `kill`) gives up on
  it: the process says on its stderr how much it didn't write (`brnr:
  process 4466: 1234 bytes of records not written: the disk didn't take
  them`), and exits as it would have.
- A process that closes a session (`session close`, `session resume
  --take-over`, its idle time running out, the editor) holds its lock, and
  answers the close, until its logger has written what it recorded of the
  session, `session_closed` last, for 2 s at most, as on exit: a resume in
  another process appends after it, and a load there finds its transcript
  (ADR 57). The event loop doesn't wait for it (P1). Past 2 s (a stalled
  disk) the session is let go of all the same, and its last records can
  come after the next process's, as they can after an exit. Opened in the
  same process again meanwhile, it keeps its lock there, and the close is
  still answered once its records are written.
- A session whose open the agent fails in a process is held the same way,
  until what the process recorded of it is written: a load's replay (ADR
  57), or the close's records, when the open took back the lock of a
  session closing there. A bridge's open (`session resume --pid`) is
  answered with its error once they are; a start that fails lets go of the
  session with its exit's records, at the latest. A fork into a session the
  process couldn't lock has nothing to let go of: it took no lock, and
  opened no transcript.
- Until then the process's `status` lists the session in `closing`, so
  that brnr can tell it from one the process is opening: `log` of it sends
  `logged` to that process and reads the transcript once it is answered,
  `session_closed` in it, as for a running session; `session resume` and
  `session delete` of it fail, saying it is closing in that process and
  free once its records are written, 2 s at most.
- What a process killed with SIGKILL hadn't written is lost, as before.

## Consequences

- `log` of a running session costs a round trip to its process, and up to
  5 s when the disk is stalled.
- Tests that read a transcript, or kill a process whose transcript they
  need, wait for it the same way, by running `brnr log`.
- A headless process whose disk stalls stays after it stopped being
  listed, holding no session and no agent, only its queue (64 MiB at most,
  ADR 6), until the disk takes it or a signal ends it (`kill`). Ended so,
  its host log can lack `exited`, and `brnr doctor` lists it as a death
  nothing recorded (ADR 11): the disk has no record of the exit.

## Considered

- Answering `start --wait` and `send --wait` only once their turn is
  written: every waiting command would wait for the disk, and a stalled one
  would stall them, which ADR 6 rules out for observers.
- Writing records on the event loop: the session would wait for the disk
  (P1, ADR 6).
- Waiting in the tests only (for a record to appear in the file): every
  user's script that runs `log` after `--wait` would have the same race.
- A time limit on exit (10 s, 30 s): a headless process would drop records
  with nobody told, its stderr unwatched and its host log, the record, on
  the stalled disk (P3), to free a process that by then holds nothing. A
  limit the user can't see is a guess (P4); the user's signal is the
  explicit choice.

## Tests

Run `cargo test --release adr_0048_`. Named claims and their assertions:

- [tests/headless.rs](../../tests/headless.rs)
  - `adr_0048_an_exit_is_written_before_the_process_goes`.
  - `adr_0048_a_signal_gives_up_on_a_stalled_disk_at_exit`.
  - `adr_0048_a_foreground_exit_given_up_on_says_what_it_didnt_write`.
  - `adr_0048_a_panic_lets_go_of_its_sessions_before_it_waits`.
  - `adr_0048_a_close_is_written_before_the_session_is_resumed`.
  - `adr_0048_a_take_over_is_written_before_the_session_is_resumed`.
  - `adr_0048_a_last_session_closed_is_written_before_it_is_resumed`.
  - `adr_0048_a_take_over_finds_the_transcript_it_continues`.
  - `adr_0048_a_close_waits_for_a_stalled_disk_2_s_at_most`.
  - `adr_0048_a_close_resumed_in_its_process_is_answered_once_written`.
  - `adr_0048_a_closing_session_is_read_once_written_and_said_to_be_closing`.
  - `adr_0048_a_failed_load_is_written_before_the_session_is_resumed`.
  - `adr_0048_a_start_whose_load_failed_is_written_before_the_session_is_resumed`.
- [tests/cli.rs](../../tests/cli.rs)
  - `adr_0048_log_shows_what_the_process_has_recorded`.
