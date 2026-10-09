# 60. A message bigger than a peer's queue reaches every peer that keeps up

Proposed 2026-10-08. Implemented. Amends ADR 6 and 49 (when a peer is
behind). Resolves #82.

## Context

ADR 6 promises that one message bigger than a peer's 16 MiB queue (20 MB in
its tests) still reaches a peer that keeps up. #82 saw `brnr send --wait
big 20000000` end with `the process closed the connection` instead of the
reply: the 20 MB `agent_message` took the connection past 16 MiB and the
`turn_ended` right after it found it past and cut it off. ADR 49, which
landed after the issue was opened, sets that line aside, and `send --wait`,
`start --wait`, watchers and bridges with the default events all got the
reply since.

A peer that asked for `acp` too (`watch --events all`, a bridge whose
`events` include it) was still cut off: the reply comes to it twice at
once, as the ACP message that carried it and as its `agent_message`. The
first takes the peer past the limit and is set aside; the second, as long,
then counts, and the `turn_ended` after it finds the peer behind. No reader
writes 20 MB in the time the host takes between them.

## Decision

- While a peer is past 16 MiB, a line longer than 16 MiB on its own
  (a message bigger than the queue) doesn't count toward it either, as the
  line that took it past doesn't (ADR 49), up to 64 MiB of lines set aside
  (two of the longest lines the host reads, ADR 51: a message's ACP line and
  its event). They stop being set aside once the queue is back under the
  limit.
- So at most 16 MiB, the lines set aside (64 MiB of them, or the first
  however long) and one more line are queued for a peer.

## Consequences

- A reply of up to about 30 MB reaches every kind of peer that keeps up,
  whatever events it asked for: `start --wait`, `send --wait`, `watch`,
  `notify` and bridges.
- A peer that stopped reading is cut off as soon as before when the lines
  are short; when they are each longer than 16 MiB, after 64 MiB of them.
- A burst of more lines that long than fit in 64 MiB (a turn's thought and
  message, each over 16 MiB, to a peer that asked for `acp`) can still cut
  off a peer that keeps up, as before, with the connection closing.

## Considered

- Setting aside every line that takes the peer past the limit, however
  short: a stream of short lines to a stopped peer would each take it past,
  and it would be cut off only at 80 MiB.
- Setting aside every line longer than the limit, without a cap: a stopped
  peer of a session sending long messages would hold them all.
- Counting the line the peer's writer is writing as written: which line
  that is when the host queues the next one is a race.
- A time (behind for so long), or raising the limit: rejected in ADR 49.
- Telling the peer it was cut off, in its own channel, before closing it: a
  peer that stopped reading has part of a line in its socket buffer, so a
  note after it couldn't be read as a line; and keeping the connection until
  the note is written holds the peer's queue for as long as it doesn't read.

## Tests

Run `cargo test --release adr_0060_`. Named claims and their assertions:

- [tests/headless.rs](../../tests/headless.rs)
  - `adr_0060_a_reply_bigger_than_the_queue_reaches_every_peer`.
- [src/host/control.rs](../../src/host/control.rs)
  - `adr_0060_a_message_and_its_acp_line_fit_at_once`.
  - `adr_0060_a_peer_that_stopped_reading_is_cut_off_soon_after_the_limit`.
