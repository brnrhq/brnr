# 49. The line that takes a peer past the limit doesn't count toward it

Proposed 2026-10-08. Implemented. Amends ADR 6 (when a peer is behind).

## Context

ADR 6 cuts off a peer once it has more than 16 MiB queued that its writer
hasn't written, and lets it take the next line however big while it is
under that, so that one long agent message (seen: 10 MB, at the end of a
burst) doesn't cut off a peer that keeps up but is a few MB behind. It
still did, one line later. A turn's `agent_message` is followed at once by
its `turn_ended`: the long message takes the peer past 16 MiB, and the
`turn_ended` finds it past and cuts it off, before its writer can have
written the message.

`reading_watcher_stays_connected` failed that way now and then on CI's
macOS runner, and under load locally: a `watch --events all` of a burst of
20,000 chunks (about 25 MB of events, then the 10 MB message) was cut off at
`turn_ended` with 17 to 20 MB queued, in 5 runs of 48, 16 at a time.

## Decision

- A peer is behind once what is queued for it, not counting the line that
  took it past 16 MiB, is past 16 MiB. That line stops being set aside once
  the queue is back under the limit.
- So at most 16 MiB, the line that took the queue past it, and one more
  line are queued for a peer, where ADR 6 had 16 MiB and a line.

## Consequences

- A peer that stopped reading is still cut off soon after 16 MiB: the line
  that took it past is set aside once, and every line after it counts.
- A long message costs its size again in memory for each peer that hasn't
  written it yet, as it did.

## Considered

- Raising the limit: a longer message would do the same.
- A time instead (behind for so long): ADR 6 left times out for observers,
  and a stopped peer would hold memory for that long.

## Tests

Run `cargo test --release adr_0049_`. Named claims and their assertions:

- [tests/headless.rs](../../tests/headless.rs)
  - `adr_0049_a_long_message_doesnt_put_a_watcher_behind`.
