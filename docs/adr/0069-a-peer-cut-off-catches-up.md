# 69. A peer cut off catches up from the transcript

Accepted 2026-10-14. Implemented. Amends 6 (a peer cut off, and what a gap
says), 21 (exit status), 22 (what a record carries), 36 (`notify` cut off),
48 (`logged`) and 60 (the burst past 64 MiB). Resolves #117.

## Context

ADR 60 lets a peer past its 16 MiB queue take lines longer than that, up to
64 MiB of them set aside. A burst of such lines that comes to more still
cuts off a peer that keeps up (#117): a thought and a message each over
16 MiB, to a peer that asked for `acp`, are four lines at once, and the
turn's end after them finds the peer behind unless its writer has written
the first of them by then. The connection closes, and the peer can't tell
that from the process going: `event watch` says `the process closed the
connection`, `prompt send --wait` fails as if the agent had gone, `event
notify` stops its command. Raising the limit moves the boundary; ADR 60
rejected the two ways to tell the peer: a note in its own channel (a peer
that stopped reading has part of a line there, so a note after it couldn't
be read as a line, and waiting to write it holds the queue as long as the
peer doesn't read), and any rule based on time (ADR 49).

What the peer missed is in the transcript (ADR 22): every event the process
sends is recorded there, as sent, and every ACP message, by the logger's
thread; `event log --follow` reads it as it grows. Two things are missing:

- A position. An event has `ts` and `host_id`. `ts` is to the microsecond,
  so two events can share one; an `acp` event's and its raw record's are
  taken at different moments, so the events file and the raw ACP can't be
  merged exactly by it; and the wall clock can step back.
- A way to tell being cut off from the process going.

The current sequence, traced from the code at 7b5a6cb:

1. `emit` (src/host/control.rs) queues the event's line for each peer that
   wants it (`send_to`), and records it (`sink.note`).
2. `Queue::push` finds the peer past 16 MiB, not counting what is set aside
   (ADR 49, 60): `Queued::Full`.
3. `drop_peer` writes `peer-dropped` (`peer`, `reason`) to the host log and
   shuts the connection down. Lines queued for it and not written are
   dropped with the queue.
4. The peer reads what its socket held, part of a line perhaps, then EOF:
   `Conn::read` (src/ctl/talk.rs) says `the process closed the connection`,
   and the command fails with that, or (`event watch`) with `(a watcher that
   falls behind is disconnected)`.

## Decision

Principles: what was sent is never lost silently (P3); the session never
waits for an observer (P1, ADR 6); nothing is guessed (P4).

### Positions

- Every event a process emits, and every ACP message it records, has a
  position, `seq`: one more than the last of the process's. Per process,
  not per session: a peer subscribes to a process, and gets one stream of
  every session's events (a bridge, `--pid`), which one number orders; and
  the events a session watcher gets come from three files (its events, its
  raw ACP, the host log, which has the process's own events such as
  `exited` and ACP of no session), which one number merges exactly. A
  session's file has gaps in it where other sessions' events are; they are
  not losses (a `records-skipped` note says what is lost, below).
- An event carries `seq` as its last key, live and in the transcript; a raw
  ACP record carries it before `dir` (`…,"session_id":"sess-1","seq":12,
  "dir":…`), and `event log` gives it to the `acp` event it makes of it. The
  copy of a `session/new` in the session's raw file, which the host log has
  with its position, has none.
- `subscribe`'s answer says where the subscription starts: `peer`, the
  connection's name as the host log has it (`socket#7`), `host_id`, and
  `seq`, the last position before it. `session new --wait`'s ready report on
  the start channel says the same (`socket#start`).

### Recording the cut-off

- `drop_peer` writes `last_queued` in the `peer-dropped` note: the last
  position when it cut the peer off. Nothing after it was queued for the
  peer; some lines before it may not have been written.
- The process keeps the last 64 cut-offs, the peer's name and `last_queued`,
  for `was_cut_off`.
- It emits a `peer_cut_off` event (`peer`, `last_queued`) to the other
  peers, once the line that cut the peer off has reached them, so that each
  sees positions in order. It is the process's (no session), in the host
  log, and shown by default.

### Asking, and catching up

- A request, `was_cut_off` `{peer}`, is answered `{peer, cut_off,
  last_queued}`: whether the process cut that peer off, and where.
- A command whose connection ends before the end it waits for (`exited`,
  its session's `session_closed`, its turn's end) catches up:
  1. It connects again and asks `was_cut_off` for its peer's name. Not cut
     off, or not one of the last 64: it ends as it did (`the process closed
     the connection`).
  2. It subscribes again to what it subscribed to; the answer's `seq` is
     where the new subscription starts, R. Every event after R comes on the
     new connection.
  3. It sends `logged` (ADR 48): once answered, the transcript has every
     event up to R.
  4. It reads the host log, whose `session-opened` records name each
     session's files, and the files of the session it follows (every
     session's for `--pid`; the raw ACP too when it asked for `acp`). Of the
     process's records, it takes those it subscribed to after its last
     event, L (or where it subscribed), up to R, in order of position, once
     each (`exited` is in every session's transcript and the host log).
  5. They come before what the new connection has sent since, and it goes
     on there.

  A process that has gone can't be asked: its host log's `peer-dropped`
  for the peer says it was cut off, steps 2 and 3 are left out, and its
  transcript, whole once it has gone (ADR 48), is read to its end. So is
  the transcript of one that goes while the command catches up, once it
  has said that it cut the peer off: its socket refuses the next
  connection.
- So do `event watch`, `prompt send --wait`, `session new --wait` and
  `session resume --wait` (on the start channel, or with `--pid`), `event
  wait`, and `event notify` on the socket. A request that only asks
  (`status`, `pending`) and was cut off unanswered is asked again once
  caught up (unable to catch up, the command says what it missed, as
  below); one that acts (`send`) is never sent twice.
- `event notify` cut off while its command runs lets the command finish,
  then catches up, and runs the command for each event it missed. Its
  connection ending otherwise (the process killed), it stops the command as
  ADR 36 has it.
- Cut off again while it catches up (it reads nothing from the new
  connection meanwhile), a command does the same again, up to 3 cut-offs
  in a row with nothing read live in between; cut off a 4th time, it gives
  up, as below. A new subscription that fails, or whose answer has no
  `seq`, gives no R: that is one more cut-off, and the command connects
  and subscribes again (the process has already said it cut the peer
  off, so it isn't asked again).

### When it can't

- What the transcript can't have: none kept (`log = false`); no raw ACP
  (`log = "events"`) for a peer that asked for `acp`; records not written
  where it missed events: a `records-skipped` note (ADR 6) now says the
  positions it lost, `first_seq` and `last_seq`, and one between L and R
  is a hole; and records a full disk didn't take that no note says yet:
  `logged` answers `lost`. The command doesn't guess around a hole (P4).
- It then says what it missed, after which event, up to where, and why, in
  the shape of ADR 6's note for skipped records:

  ```json
  {"event":"events-missed","ts":"…","host_id":"…","peer":"socket#7","after":120,"until":180,"error":"the process keeps no transcript (log = false)"}
  ```

  `until` is R, where the command subscribed again. Given up at the 4th
  cut-off in a row with no R (its last new subscription failed), it is
  where the process said it cut the peer off, `was_cut_off`'s
  `last_queued`. It is null in two cases only: the process has gone (its
  socket refuses a new connection), so its transcript is read to its end;
  or the process, live, cut the peer off but said no `last_queued`, which
  brnr's own process always says, and `error` then says that it gave no
  position. `event watch` shows it as its last event, in text or JSON;
  the others on stderr (`brnr: cut off:
  events after 120 up to 180 missed: …`). It exits 3, which no other
  outcome has.

### Bridges

A started bridge isn't caught up. Its events come on its stdin, a pipe the
process opened; cutting it off closes the pipe and sends SIGTERM (ADR 6),
and there is no connection to open again (a second subscription, which ADR
36 considered and rejected). The host log's `peer-dropped` and the other
peers' `peer_cut_off` say that it was cut off and where. A bridge that must
not miss events subscribes on the socket itself (`BRNR_SOCKET`): `brnr
event notify <session> -- …` rather than `--stdin`, or a client of its own
that catches up as above. `event notify --stdin` cut off says so and exits
1, as before.

### What a peer can learn

`was_cut_off` answers whether and where, never what: no event is in its
answer. What a peer catches up on it reads from the transcript itself, as
the user, which processes of the user's can read anyway (P13), and only the
events it subscribed to. A peer may ask about another peer; it learns where
that one was cut off, which tells it nothing it couldn't get by subscribing,
and the record isn't consumed, so asking can't take it from its owner.

## Consequences

- A peer that keeps up gets every event, once and in order, whatever the
  burst, as long as the transcript has it. #117's limit stays where ADR 60
  set it; what changes is what follows a cut-off.
- Every event line, live and recorded, and every raw ACP record is a few
  bytes longer.
- A cut-off costs a connection and reads of the process's files: its host
  log, and its session's events, and raw ACP for a peer that asked for
  `acp`; every session's for `--pid`. Lines of other processes, and out of
  the range, are passed over without being read whole. What it missed is
  held in the command's memory while it goes out.
- `event watch` no longer ends when it falls behind; it ends with `exited`,
  its session's close, or exit status 3.
- The names this adds (`seq`, `peer_cut_off`, `last_queued`, `first_seq`,
  `last_seq`, `events-missed`, kebab-case as the logger's other notes are)
  depend on #150 and #151, still open, which settle the names and shapes of
  `--json` output and events; they may be renamed there. The change is kept
  to one field on every event, one event, one note and one request.

## Considered

- Positions per session: a bridge or a `--pid` peer follows every session
  in one stream, which numbers per session can't order, nor merge with the
  process's own events in the host log.
- `ts` as the position: not unique, not the same for an `acp` event and its
  raw record, and the wall clock can step back.
- A line count of each file: per file, and a file holds several processes'
  records, appended.
- The process keeping what a cut peer missed until it comes back: memory
  without bound, what ADR 6 cuts peers off to avoid; the transcript has it.
- The process replaying the transcript on the new connection (`subscribe`
  from a position): it would read files on its event loop, or hold the new
  peer's queue while a thread did (P1); the command reading the transcript
  as the user needs nothing from the process but positions.
- A note in the cut peer's own channel, or a rule based on time: rejected in
  ADR 60, for the reasons above.
- Telling a gap from its times (`since`, `until`) against the last event's
  `ts`: a clock that steps back hides one; positions don't.
- Catching up without a bound: a peer that can't keep up even so would go
  round for ever without saying so (P3).
- A started bridge reconnecting on the socket: it would hold two
  subscriptions, its stdin's and the socket's.

## Tests

Run `cargo test --release adr_0069_`. Named claims and their assertions:

- [tests/headless.rs](../../tests/headless.rs)
  - `adr_0069_a_watcher_cut_off_catches_up`.
  - `adr_0069_a_burst_past_64_mib_reaches_a_watcher`.
  - `adr_0069_a_watcher_that_cant_catch_up_says_what_it_missed`.
  - `adr_0069_a_wait_cut_off_catches_up`.
  - `adr_0069_event_wait_cut_off_catches_up`.
- [tests/cli.rs](../../tests/cli.rs)
  - `adr_0069_notify_cut_off_catches_up_once_its_command_is_done`.
- [tests/security.rs](../../tests/security.rs)
  - `adr_0069_was_cut_off_says_where_never_what`.
- [src/ctl/talk.rs](../../src/ctl/talk.rs)
  - `adr_0069_a_cut_off_while_catching_up_is_tried_again_then_given_up`.
  - `adr_0069_event_wait_cut_off_looking_says_what_it_missed`.
  - `adr_0069_a_live_process_that_cant_be_caught_up_with_says_until_where`.
  - `adr_0069_a_new_subscription_that_fails_is_one_more_cut_off`.
  - `adr_0069_a_start_channel_cut_off_catches_up_or_says_what_it_missed`.
- [src/ctl/notify.rs](../../src/ctl/notify.rs)
  - `adr_0069_notify_cut_off_looking_runs_for_what_it_read_then_says_what_it_missed`.
- [src/ctl/catch_up.rs](../../src/ctl/catch_up.rs)
  - `adr_0069_a_records_position_is_read_from_its_end_or_head`.
- [src/log.rs](../../src/log.rs)
  - `adr_0069_what_the_transcript_lost_is_said_by_position`.
