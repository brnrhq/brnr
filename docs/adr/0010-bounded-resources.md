# 10. Bounded resources

Draft 2026-10-19. Implemented. Factor: N2 (ADR 1). Brings together former
ADRs 6, 49, 51, 60 and 69 (docs/adr/README.md maps them).

How much memory a reader that falls behind, or a long line, can cost the
process, and what a peer that is cut off does next. Memory stays bounded
whatever a peer, the agent or the editor sends (N2, P6): observers give way
and are told (P3), and the session's own pipes get backpressure (P1).

## 10.1 How far behind a reader may fall

Who gives way depends on who the reader is (P6).

Observers give way, and never slow the session:

- A peer (a bridge, `event watch`, `event notify`, any control connection)
  may have 16 MiB queued that its writer hasn't written yet, counted from
  when a line is queued until it is written.
- Some lines are set aside and don't count toward the 16 MiB:
  - the line that takes the peer past 16 MiB;
  - while the peer is past it, each line longer than 16 MiB on its own,
    up to 64 MiB of lines set aside (two of the longest lines the host
    reads, ADR 10.3: a message's ACP line and its event).

  They stop being set aside once the queue is back under 16 MiB.
- A peer is behind once what is queued for it, not counting what is set
  aside, is past 16 MiB. Until then it takes the next line, however big. So
  at most 16 MiB, the lines set aside (64 MiB of them, or the first however
  long), and one more line are queued for a peer.
- A peer that is behind is cut off: a connection is shut down, a started
  bridge gets SIGTERM. The host log has `peer_dropped`, and the other peers
  get `peer_cut_off` (ADR 10.2).
- The foreground's display has a bounded buffer, then skips events, saying
  how many (`… 120 events not shown`). Its stdout closing ends the display,
  not the session (ADR 3.5).

The session's own pipes get backpressure, as a direct pipe would give them
(P1):

- The editor link's queue is capped at 16 MiB, as peers' are. Past it the
  host stops reading the agent's stdout and stderr, and the agent blocks on
  its stdout as it would writing to the editor directly.
- The agent's stdin queue is capped the same way. Past it the host stops
  reading the link, the proxy blocks, and the editor's write blocks.
- Stdin's EOF waits behind what the editor wrote, as on a pipe. A signal to
  the proxy and its stdout failing don't wait: they go on a socket of their
  own, the signal link, which the host never stops reading, and on which it
  hears the proxy go (ADR 3.1).
- brnr has no timeout of its own. An editor that hangs stalls its agent for
  as long as it hangs, as a direct pipe would (P1). An editor that is gone
  needs no timeout: its pipe closes, the proxy's write fails, and the host
  hears the editor has gone.
- The host's own channels are bounded too. A reader thread (the agent's
  stdout, the link) blocks when the event loop is that far behind, so
  "stops reading" holds the bytes in the pipe, not in memory.
- While the agent's stdout is held back, observers see nothing of the agent
  either (no answers to injected prompts, no approvals): P1 ranks above
  them.

The logger's queue is bounded, and gives way as an observer does:

- Past 64 MiB not yet written (a slow or stalled disk), records are skipped
  and counted. A session's opening is never skipped, nor `process_exited` or
  `process_panicked`, which say how the process ended (ADR 3.7).
- Once the queue is down to half that, or the logger has caught up, a
  `records_skipped` note (`count`, `acp_count`, `since`, `until`,
  `first_seq`, `last_seq`, and `ts`, `host_id`, `pid` and in a session's
  file `session`, as an event has them, ADR 5.9) says what was lost, in the host log and in each session's
  events file that lost some (P3). `first_seq` and `last_seq` are the
  positions lost (ADR 10.2).
- Records a write fails to put in a file (a full disk) are noted the same
  way, with the `error`, once the file takes records again.
- `event log` always shows these notes, whatever `--events` chose.

**Consequences.**

- A reply of up to about 30 MB reaches every kind of peer that keeps up,
  whatever events it asked for: `session new --wait`, `prompt send --wait`,
  `event watch`, `event notify` and bridges.
- A peer that stopped reading is cut off soon after 16 MiB when the lines
  are short, and after 64 MiB of them when each is longer than 16 MiB.
- A burst of more such lines than fit in 64 MiB (a turn's thought and
  message, each over 16 MiB, to a peer that asked for `acp`) can still cut
  off a peer that keeps up. It then catches up from the transcript
  (ADR 10.2).
- A long message costs its size again in memory for each peer that hasn't
  written it yet.
- `session status` quotes at most 4000 characters of the last message; the
  events have all of it.

**Why.** Every reader of a session can fall behind: a bridge or a watcher,
the foreground's stdout, the editor through the proxy, the agent reading
its stdin. With unbounded queues, an editor that stopped reading cost about
315 MiB before anything happened, and a foreground that printed on the
event loop blocked the whole host. A timeout on writes to the editor
bounded memory only by what the agent writes in that time, and killed the
agent of an editor that only paused. A long agent message (10 MB at the end
of a burst; 20 MB in #82) is followed at once by its `turn_ended`, and a
peer that asked for `acp` gets it twice at once, as the ACP line and as
`agent_message`: no reader writes 20 MB in the time the host takes between
them, so counting every line cut off peers that kept up (N2, P6, P3).

**Considered.**

- A count of lines (first 4096 per peer): lines vary from a few bytes to
  megabytes. On a slow CI runner a raw `event watch` that was reading the
  whole time fell more than 4096 lines behind an agent streaming 20,000
  chunks at once, and was cut off as if it had stopped.
- Dropping `acp` events for a slow peer instead of cutting it off: lossy
  where `--events all` promises everything.
- "Behind" as "the next line would take it past the limit": it cuts off
  peers that keep up when one big message comes while they are a few MB
  behind, and one message over the limit cuts off every peer, `brnr session
  status` included.
- A 30 s timeout on writes to the editor, counted as the editor gone:
  memory bounded only by how much the agent writes in 30 s, and the agent
  never feels the editor's pace.
- Backpressure plus that timeout: memory is bounded without it, and it
  kills the agent of an editor that only paused.
- An unbounded buffer for the foreground (more of the memory problem), or
  blocking the host on it (it stops the control socket, the agent's traffic
  and signals).
- Raising the limit: a longer message would do the same.
- A time instead (behind for so long): observers have no times, and a
  stopped peer would hold memory for that long.
- Setting aside every line that takes the peer past the limit, however
  short: a stream of short lines to a stopped peer would each take it past,
  and it would be cut off only at 80 MiB.
- Setting aside every line longer than the limit, without a cap: a stopped
  peer of a session sending long messages would hold them all.
- Counting the line the peer's writer is writing as written: which line
  that is when the host queues the next one is a race.
- Telling the peer it was cut off, in its own channel, before closing it: a
  peer that stopped reading has part of a line in its socket buffer, so a
  note after it couldn't be read as a line; and keeping the connection until
  the note is written holds the peer's queue for as long as it doesn't read.

## 10.2 A peer cut off catches up from the transcript

Principles: what was sent is never lost silently (P3); the session never
waits for an observer (P1, ADR 10.1); nothing is guessed (P4).

### Positions

- Every event a process emits, and every ACP message it records, has a
  position, `seq`: one more than the process's last. It is per process,
  not per session. A peer subscribes to a process and gets one stream of
  every session's events (a bridge, `--pid`), which one number orders. The
  events a session watcher gets come from three files (its events, its raw
  ACP, and the host log, which has the process's own events such as
  `process_exited` and ACP of no session), which one number merges exactly.
- A session's file has gaps where other sessions' events are. They are not
  losses: a `records_skipped` note says what is lost (ADR 10.1).
- An event carries `seq` as its last key, live and in the transcript. A raw
  ACP record carries it before `dir` (`…,"session":"sess-1","seq":12,
  "dir":…`), and `event log` gives it to the `acp` event it makes of the
  record. The copy of a `session/new` in the session's raw file has none;
  the host log has it with its position.
- `subscribe`'s answer says where the subscription starts: `peer`, the
  connection's name as the host log has it (`socket#7`), `host_id`, `pid`,
  and `seq`, the last position before it. `session new --wait`'s ready report
  on the start channel says the same (`socket#start`).

### Recording the cut-off

- A peer cut off (ADR 10.1) gets a `peer_dropped` record (`peer`, `reason`
  `fell_behind`, `last_queued_seq`) in the host log, and its connection is shut down. Lines
  queued for it and not written are dropped with the queue. `last_queued_seq`
  is the last position when the peer was cut off: nothing after it was
  queued for the peer; some lines before it may not have been written.
- The process keeps the last 64 cut-offs, the peer's name and
  `last_queued_seq`, for `was_cut_off`.
- It emits a `peer_cut_off` event (`peer`, `last_queued_seq`) to the other
  peers, once the line that cut the peer off has reached them, so that each
  sees positions in order. The event is the process's (no session), in the
  host log, and shown by default.

### Asking, and catching up

- A request, `was_cut_off` `{peer}`, is answered `{peer, cut_off,
  last_queued_seq}`: whether the process cut that peer off, and where.
- A command whose connection ends before the end it waits for (`process_exited`,
  its session's `session_closed`, its turn's end) catches up:
  1. It connects again and asks `was_cut_off` for its peer's name. Not cut
     off, or not one of the last 64: it ends as it did (`the process closed
     the connection`).
  2. It subscribes again to what it subscribed to. The answer's `seq` is
     where the new subscription starts, R. Every event after R comes on the
     new connection.
  3. It sends `logged` (ADR 5.5). Once answered, the transcript has every
     event up to R.
  4. It reads the host log, whose `session_opened` records name each
     session's files, and the files of the session it follows (every
     session's for `--pid`; the raw ACP too when it asked for `acp`). Of
     the process's records, it takes those it subscribed to after its last
     event, L (or where it subscribed), up to R, in order of position, once
     each (`process_exited` is in every session's transcript and the host log).
  5. They come before what the new connection has sent since, and it goes
     on there. Its last event is then the last the new connection sent, or
     R if none. Cut off again, it catches up after that, so nothing comes
     twice.
- A process that has gone can't be asked. Its host log's `peer_dropped` for
  the peer says it was cut off, steps 2 and 3 are left out, and its
  transcript, whole once it has gone (ADR 5.5), is read to its end. So is
  the transcript of a process that goes while the command catches up, once
  it has said that it cut the peer off: its socket refuses the next
  connection.
- A process that is stopping still accepts until its socket goes, and hangs
  up on what it is asked (#275). One that hangs up on step 1, or whose
  socket refuses, is waited for, up to 10 s, until it has gone: its pid,
  which names the socket, has exited, or is a zombie nothing has reaped yet.
  Its socket isn't asked: a process removes it before its last records are
  written, and a full backlog refuses on macOS and blocks on Linux. Gone, it
  is caught up with as above if its host log has the peer's `peer_dropped`.
  Otherwise the command fails, as it does for a peer that wasn't cut off.
  One that hangs up on step 3 has already said it cut the peer off: it is
  waited for the same way and, gone, its transcript is read to its end;
  still there after 10 s, the command says what it missed up to R, as
  below.
- These commands catch up: `event watch`, `prompt send --wait`, `session
  new --wait` and `session resume --wait` (on the start channel, or with
  `--pid`), `event wait`, and `event notify` on the socket.
- A request that only asks (`status`, `pending`) and was cut off
  unanswered is asked again once caught up. Unable to catch up, the command
  says what it missed, as below. A request that acts (`send`) is never sent
  twice.
- `event notify` cut off while its command runs lets the command finish,
  then catches up, and runs the command for each event it missed. If its
  connection ends otherwise (the process killed), it stops the command
  (ADR 5.8).
- Cut off again while it catches up (it reads nothing from the new
  connection meanwhile), a command does the same again, up to 3 cut-offs
  in a row with nothing read live in between. Cut off a 4th time, it gives
  up, as below. A new subscription that fails, or whose answer has no
  `seq`, gives no R: that is one more cut-off, and the command connects and
  subscribes again. The process has already said it cut the peer off, so it
  isn't asked again.

### When it can't

- The command can't catch up where the transcript doesn't have what it
  missed:
  - none is kept (`log = "off"`);
  - no raw ACP is kept (`log = "events"`), for a peer that asked for `acp`;
  - records weren't written where it missed events: a `records_skipped`
    note's `first_seq` and `last_seq` between L and R is a hole;
  - a full disk didn't take records that no note says yet: `logged`
    answers `lost`.

  The command doesn't guess around a hole (P4).
- It then says what it missed, after which event, up to where, and why, in
  the shape of the logger's `records_skipped` note:

  ```json
  {"event":"events_missed","ts":"…","host_id":"…","pid":4466,"peer":"socket#7","after_seq":120,"until_seq":180,"error":"the process keeps no transcript (log = \"off\")"}
  ```

  `until_seq` is R, where the command subscribed again. Given up at the 4th
  cut-off in a row with no R (its last new subscription failed), it is
  where the process said it cut the peer off, `was_cut_off`'s
  `last_queued_seq`. It is null in two cases only: the process has gone (its
  socket refuses a new connection), so its transcript is read to its end;
  or the process, live, cut the peer off but said no `last_queued_seq`, which
  brnr's own process always says, and `error` then says that it gave no
  position.
- `event watch` shows it as its last event, in text or JSON; the others on
  stderr (`brnr: cut off: events after 120 up to 180 missed: …`). The
  command exits 3, which no other outcome has (ADR 8.3).

### Bridges

- A started bridge isn't caught up. Its events come on its stdin, a pipe
  the process opened; cutting it off closes the pipe and sends SIGTERM
  (ADR 10.1), and there is no connection to open again. The host log's
  `peer_dropped` and the other peers' `peer_cut_off` say that it was cut
  off and where.
- A bridge that must not miss events subscribes on the socket itself
  (`BRNR_SOCKET`): `brnr event notify <session> -- …` rather than
  `--stdin`, or a client of its own that catches up as above.
- `event notify --stdin` cut off says so and exits 1.

### What a peer can learn

`was_cut_off` answers whether and where, never what: no event is in its
answer. What a peer catches up on it reads from the transcript itself, as
the user, which processes of the user's can read anyway (P13), and only
the events it subscribed to. A peer may ask about another peer. It learns
where that one was cut off, which tells it nothing it couldn't get by
subscribing, and the record isn't consumed, so asking can't take it from
its owner.

**Consequences.**

- A peer that keeps up gets every event, once and in order, whatever the
  burst, as long as the transcript has it and it catches up within 3
  cut-offs in a row. Otherwise it says what it missed and exits 3. The
  limits of ADR 10.1 stay; this is what follows a cut-off.
- Every event line, live and recorded, and every raw ACP record is a few
  bytes longer.
- A cut-off costs a connection and reads of the process's files: its host
  log, and its session's events, and raw ACP for a peer that asked for
  `acp`; every session's for `--pid`. Lines of other processes, and out of
  the range, are passed over without being read whole. What it missed is
  held in the command's memory while it goes out.
- `event watch` doesn't end when it falls behind. It ends with `process_exited`,
  its session's close, or exit status 3.
- The names this adds (`seq`, `peer_cut_off`, `last_queued_seq`,
  `first_seq`, `last_seq`, `events_missed` with `after_seq` and
  `until_seq`) follow ADR 5.9, and their shapes in `--json` ADR 8.2. The change is kept to one field on every event, one event, one
  note and one request.

**Why.** A burst of lines longer than 16 MiB that comes to more than the
64 MiB ADR 10.1 sets aside still cuts off a peer that keeps up (#117): a
thought and a message each over 16 MiB, to a peer that asked for `acp`,
are four lines at once. When the connection closed, the peer couldn't tell
that from the process going: `event watch` said `the process closed the
connection`, `prompt send --wait` failed as if the agent had gone, `event
notify` stopped its command. Raising the limit only moves the boundary, and
ADR 10.1 rejects telling the peer in its own channel and rules based on
time. What the peer missed is in the transcript (ADR 5.1); what it needs is
a position, which `ts` can't be, and a way to tell being cut off from the
process going.

**Considered.**

- Positions per session: a bridge or a `--pid` peer follows every session
  in one stream, which numbers per session can't order, nor merge with the
  process's own events in the host log.
- `ts` as the position: it is to the microsecond, so not unique; an `acp`
  event's and its raw record's are taken at different moments; and the
  wall clock can step back.
- A line count of each file: per file, and a file holds several processes'
  records, appended.
- The process keeping what a cut peer missed until it comes back: memory
  without bound, what ADR 10.1 cuts peers off to avoid; the transcript has
  it.
- The process replaying the transcript on the new connection (`subscribe`
  from a position): it would read files on its event loop, or hold the new
  peer's queue while a thread did (P1). The command reading the transcript
  as the user needs nothing from the process but positions.
- A note in the cut peer's own channel, or a rule based on time: rejected
  in ADR 10.1.
- Telling a gap from its times (`since`, `until`) against the last event's
  `ts`: a clock that steps back hides one; positions don't.
- Catching up without a bound: a peer that can't keep up even so would go
  round for ever without saying so (P3).
- A started bridge reconnecting on the socket: it would hold two
  subscriptions, its stdin's and the socket's (ADR 5.8).

## 10.3 Lines too long to read

- The host reads (interprets, and keeps in the raw transcript) a line from
  the agent's stdout or from the editor of up to 32 MiB, its newline
  included. While it waits for its newline the line is its reader's, and
  isn't counted against the 16 MiB cap (ADR 10.1): counted, a line longer
  than the cap would deadlock its reader. So one pipe can cost the host the
  cap and one line's worth, 48 MiB, whatever the lines.
- A longer line isn't kept whole. Its first 32 MiB, then the rest as each
  read brings it, go on counted and held back as any output is:
  - with an editor attached, to the agent or the editor, byte for byte
    (P1). The host treats it as a line it can't parse (ADR 2.5): it isn't
    interpreted, nor recorded as raw ACP;
  - headless, from the agent, nowhere: it is dropped (P4).
- Either way, a `line_too_long` event (`from`: `agent` or `editor`;
  `limit`, in bytes; `relayed`: whether it went on) says so once for each
  such line, when it passes the limit: in every session's events file, to
  watchers and bridges, and in the foreground (P3). It belongs to no
  session: the host hasn't read it, so, as `process_exited`, it goes to all of
  them.
- The agent's stderr isn't interpreted. It goes on as each read brings it,
  up to 64 KiB, a line or not, every byte and in order, to the editor's
  stderr through the proxy and to the foreground's, unchanged (ADR 2.6,
  ADR 3.6). The host log has a record for each line, or for each piece of
  64 KiB of a longer one. A character that a piece's end would cut starts
  the next piece. A line of exactly 64 KiB is one record, not one and an
  empty one.
- A failed start's tail keeps a long line's first 2000 bytes, cut between
  characters, as it keeps those of any line (ADR 3.6).
- A started bridge's stderr and the process's own stderr, detached
  (`process_stderr`, ADR 3.7), go into the host log the same way: a line or a
  piece of 64 KiB at a time, a character a piece's end would cut starting
  the next piece. A bridge's note's decoded text is at most 64 KiB too, the
  rest in the next notes.

What is bounded, and what isn't:

- Bounded:
  - the agent's stdout and the editor's input: 16 MiB queued, and up to
    32 MiB of a line still coming, each;
  - the agent's stderr: the same cap, plus a piece of 64 KiB;
  - a started bridge's stderr, and the process's own stderr, detached: a
    line or a piece of 64 KiB at a time;
  - the link: 16 MiB, and a frame is at most a line the host read or a
    piece;
  - each peer's queue: 16 MiB, plus the lines set aside, up to 64 MiB of
    them, and one more line (ADR 10.1);
  - the logger's queue: 64 MiB (ADR 10.1);
  - requests the host answered that the editor was shown, kept for its
    late answer (ADR 4.5, 7.1): 1024, the oldest going first, none past
    their session's close;
  - requests the agent cancelled that the editor was shown, kept to
    record the editor's answer in their session (ADR 4.5, 7.1): 1024, the
    oldest going first, none past their session's close;
  - sessions brnr closed under the editor, kept to answer the editor's
    requests for them (ADR 7.1): 1024, the oldest going first;
  - the foreground's display (ADR 3.5);
  - the proxy reads the editor 64 KiB at a time;
  - how deep a line may nest for brnr to read it: 128 levels for what the
    agent, the editor or a peer sends, 256 for what brnr wrote, and so the
    stack any value takes (ADR 2.5). No thread is sized to a line.
- Not bounded by a count of bytes:
  - the agent's message or thought being assembled from its chunks, for an
    `agent_message` event: each chunk has been through the cap, and the
    message is as long as the agent makes it;
  - what a session keeps (held messages and context, the last message,
    config options and commands), whose size the user and the agent
    choose;
  - a line from a peer on the control socket, read as a whole line: it is
    the user's own process (P13), not the session's pipes.

What isn't bounded is a gap measured against N2 (ADR 1).

**Consequences.**

- An editor's late answer to a request no longer kept goes to the agent,
  which ignores it (ADR 2.5), and the editor isn't told who answered first.
  An editor's request for a session closed under it that is no longer kept
  reaches the agent, which answers it with its own error.
- An editor's answer to a request the agent cancelled that is no longer
  kept still goes to the agent as it is (ADR 4.5), but with no session: it
  is recorded in the host log, not the session's transcript, and its `acp`
  event has `session: null`.
- A message over 32 MiB, valid JSON or not, isn't seen by the host: no
  events for it. If it answered a request of the host's, the request goes
  unanswered, as for a line it can't parse (ADR 2.5). With an editor, the
  editor still gets it.
- An editor's line over 32 MiB goes to the agent as it is. A prompt that
  long isn't counted as a turn, a `session/load` that long isn't checked
  against the session's lock (ADR 3.9), and an `initialize` that long isn't
  stripped of `fs` and `terminal` (ADR 2.7). Editors don't send those that
  long; refusing them would mean reading them.
- The host's memory doesn't grow with the length of a line. A 128 MiB line
  to an editor that stopped reading leaves the host at about 54 MB: the
  line's first 32 MiB, queued for the link and written from where it is (a
  frame's payload isn't copied), and what the host holds anyway.
- The limit is twice the cap so that one message bigger than a peer's
  queue, which ADR 10.1 lets through to a peer that keeps up, is still
  read.

**Why.** A line counted only once its newline came let a newline-free
stream go past the cap without limit: 64 MiB of `x` took the host from
3 MB to 71 MB, and 96 MiB to an editor that had stopped reading, 486 MB.
The editor's input and the agent's stderr had the same gap. Holding back
the pipe can't fix it alone: a line longer than the cap would hold its own
reader back for room only its newline could make, a deadlock in the middle
of a message. So past some length a line can't be kept whole. With an
editor attached the bytes must reach it unchanged (P1); headless, brnr is
the client, and refuses what it can't be sure of (P4). Either way it says
so (P3), and memory stays bounded (P6).

**Considered.**

- Keeping the requests the host answered, those the agent cancelled and
  the sessions closed under the editor until the editor follows up, without
  a cap: an editor that never answers a withdrawn or cancelled request, or
  never loads a closed session again, grows them for the process's life
  (#332, #364).
- Counting a line still coming against the cap, and so a limit of 16 MiB:
  one bound for everything, but messages ADR 10.1 promises to peers (20 MB)
  would not be read.
- A larger limit (64 MiB, say): more of the host's memory is the agent's to
  take, for messages no adapter sends.
- Cutting the line in the proxy role, or replacing it with an error: it
  changes the editor's bytes (P1).
- Spooling a long line to disk so the host can still read it: memory
  bounded, but a parse of an unbounded line isn't, and disk use becomes the
  agent's to choose.
- Reading a long line leniently for its `id` and `method`: the guess
  ADR 2.5 rejects.
- An event at the line's end with its length: one that never ends would
  never be told.

## Tests

Run `cargo test --release adr_0010_`. Named claims, by section:

- 10.1:
  - [tests/headless.rs](../../tests/headless.rs):
    `adr_0010_slow_watcher_is_disconnected`,
    `adr_0010_reading_watcher_stays_connected`,
    `adr_0010_huge_message_reaches_watchers`,
    `adr_0010_big_message_to_a_lagging_watcher`,
    `adr_0010_every_watcher_sees_the_exit`,
    `adr_0010_a_stalled_disk_skips_records_and_says_so`,
    `adr_0010_editor_that_stops_reading_holds_the_agent_back`,
    `adr_0010_a_stalled_agent_holds_the_editor_back`,
    `adr_0010_a_long_message_doesnt_put_a_watcher_behind`,
    `adr_0010_a_reply_bigger_than_the_queue_reaches_every_peer`.
  - [src/log.rs](../../src/log.rs):
    `adr_0010_past_the_cap_records_are_skipped_and_noted_before_the_next`,
    `adr_0010_a_gap_is_noted_in_the_host_log_and_each_session_that_lost_records`,
    `adr_0010_a_failed_write_is_noted_once_the_file_takes_records_again`.
  - [src/host/control.rs](../../src/host/control.rs):
    `adr_0010_a_message_and_its_acp_line_fit_at_once`,
    `adr_0010_a_peer_that_stopped_reading_is_cut_off_soon_after_the_limit`.
- 10.2:
  - [tests/headless.rs](../../tests/headless.rs):
    `adr_0010_a_watcher_cut_off_catches_up`,
    `adr_0010_a_burst_past_64_mib_reaches_a_watcher`,
    `adr_0010_a_watcher_that_cant_catch_up_says_what_it_missed`,
    `adr_0010_a_wait_cut_off_catches_up`,
    `adr_0010_event_wait_cut_off_catches_up`.
  - [tests/cli.rs](../../tests/cli.rs):
    `adr_0010_notify_cut_off_catches_up_once_its_command_is_done`.
  - [tests/security.rs](../../tests/security.rs):
    `adr_0010_was_cut_off_says_where_never_what`.
  - [src/ctl/client.rs](../../src/ctl/client.rs):
    `adr_0010_a_cut_off_while_catching_up_is_tried_again_then_given_up`,
    `adr_0010_a_live_process_that_cant_be_caught_up_with_says_until_where`,
    `adr_0010_a_new_subscription_that_fails_is_one_more_cut_off`,
    `adr_0010_a_peer_cut_off_by_a_stopping_process_catches_up`,
    `adr_0010_a_process_has_gone_once_it_has_exited`,
    `adr_0010_an_event_live_while_subscribing_again_comes_once`.
  - [src/ctl/talk.rs](../../src/ctl/talk.rs):
    `adr_0010_event_wait_cut_off_looking_says_what_it_missed`,
    `adr_0010_a_start_channel_cut_off_catches_up_or_says_what_it_missed`.
  - [src/ctl/notify.rs](../../src/ctl/notify.rs):
    `adr_0010_notify_cut_off_looking_runs_for_what_it_read_then_says_what_it_missed`.
  - [src/transcript.rs](../../src/transcript.rs):
    `adr_0010_a_records_position_is_read_from_its_end_or_head`.
  - [src/log.rs](../../src/log.rs):
    `adr_0010_what_the_transcript_lost_is_said_by_position`.
- 10.3:
  - [src/host/approvals.rs](../../src/host/approvals.rs):
    `adr_0010_answered_requests_past_the_cap_lose_the_oldest`,
    `adr_0010_a_sessions_answered_requests_go_with_its_close`,
    `adr_0010_given_up_requests_past_the_cap_lose_the_oldest`,
    `adr_0010_a_sessions_given_up_requests_go_with_its_close`.
  - [src/host/sessions.rs](../../src/host/sessions.rs):
    `adr_0010_sessions_closed_under_the_editor_past_the_cap_lose_the_oldest`,
    `adr_0010_a_closed_sessions_answered_requests_go`,
    `adr_0010_a_request_cancelled_after_its_close_is_not_kept`.
  - [tests/headless.rs](../../tests/headless.rs):
    `adr_0010_headless_an_agent_line_past_the_limit_is_dropped_and_said`,
    `adr_0010_a_long_line_to_an_editor_that_stops_reading_is_held_back`,
    `adr_0010_an_editor_line_past_the_limit_goes_to_the_agent_unread`,
    `adr_0010_the_agents_stderr_without_a_newline_is_bounded`,
    `adr_0010_a_bridges_long_stderr_line_goes_to_the_host_log_in_pieces`,
    `adr_0010_a_bridges_stderr_line_that_fills_a_piece_is_one_note`,
    `adr_0010_a_bridges_stderr_keeps_a_carriage_return_inside_a_line`,
    `adr_0010_a_bridges_stderr_keeps_a_character_a_piece_would_cut`,
    `adr_0010_an_agents_stderr_line_that_fills_a_piece_is_one_record`,
    `adr_0010_an_agents_stderr_keeps_a_character_a_piece_would_cut`,
    `adr_0010_the_processs_own_stderr_goes_to_the_host_log_in_pieces`.
  - [src/host/mod.rs](../../src/host/mod.rs):
    `adr_0010_stderr_read_as_lines_ends_with_what_a_piece_left`,
    `adr_0010_a_stderr_note_is_at_most_a_piece_once_decoded`,
    `adr_0010_a_tails_line_is_cut_between_characters`.
  - [src/host/lines.rs](../../src/host/lines.rs):
    `adr_0010_lines_are_read_whole_however_the_reads_split_them`,
    `adr_0010_a_line_that_ends_at_the_limit_is_read_whole`,
    `adr_0010_a_relayed_line_past_the_limit_goes_on_as_it_comes`,
    `adr_0010_a_cut_line_is_in_parts_of_the_limit`,
    `adr_0010_the_newline_after_a_cut_only_ends_its_line`,
    `adr_0010_a_character_a_cut_would_split_starts_the_next_part`,
    `adr_0010_a_cut_is_between_characters`,
    `adr_0010_a_byte_at_a_time_reads_as_all_at_once`.
