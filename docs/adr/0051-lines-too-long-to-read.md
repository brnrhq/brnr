# 51. Lines too long to read

Accepted 2026-10-08. Implemented. Amends 6 (what a reader holds beside the
cap), 10 (the agent's stderr goes on in pieces) and 26 (a line too long to
read). Resolves #58.

Amended by 62: the agent's stderr goes on as each read brings it, up to
64 KiB, a line or not; a piece of 64 KiB is what the host log records of a
longer line.

## Context

ADR 6 caps what is on its way through the host at 16 MiB, counted in bytes,
but a line of the agent's stdout was only counted once its newline came:
the reader thread assembled it first, for as long as it took. A newline-free
stream went past the cap without limit. In the review, 64 MiB of `x` with no
newline took the host from 3 MB to 71 MB; 96 MiB to an editor that had
stopped reading, 486 MB. The editor's input had the same gap (what came
after its last newline, `editor_buf`, was counted as done), and the agent's
stderr was read a whole line at a time, uncounted: 96 MiB on stderr with no
newline, 101 MB.

Holding back the pipe can't fix it alone: a line longer than the cap would
hold its own reader back for room only its newline could make, a deadlock
in the middle of a message. So past some length a line can't be kept whole,
and what happens to it then is the decision. The principles pull two ways:
with an editor attached, the bytes must reach it unchanged (P1); headless,
brnr is the client, and refuses what it can't be sure of (P4). Either way
it says so (P3), and memory stays bounded (P6).

## Decision

- The host reads (interprets, and keeps in the raw transcript) a line from
  the agent's stdout or from the editor of up to 32 MiB, its newline
  included. While it waits for its newline it is the reader's, and not
  counted against the cap (ADR 6): counted, a line longer than the cap would
  deadlock its reader. So what one pipe can cost the host is the cap and
  one line's worth, 48 MiB, whatever the lines.
- A longer line isn't kept whole. Its first 32 MiB, then the rest as it
  comes (as each read brings it), go on counted and held back as any output
  is:
  - with an editor attached, to the agent or the editor, byte for byte
    (P1). The host treats it as a line it can't parse (ADR 26): it isn't
    interpreted, nor recorded as raw ACP;
  - headless, from the agent, nowhere: it is dropped (P4).
- Either way, a `line_too_long` event (`from`: `agent` or `editor`; `limit`,
  in bytes; `relayed`: whether it went on) says so once for each such line,
  when it passes the limit: in every session's events file, to watchers and
  bridges, and in the foreground (P3). It belongs to no session: the host
  hasn't read it, so as `exited` it goes to all of them.
- The agent's stderr isn't interpreted, so it isn't kept as lines past
  64 KiB: a longer line goes on in pieces of that, every byte and in order,
  to the editor's stderr through the proxy and to the foreground's (ADR 10),
  unchanged. The host log has a record for each piece, and a failed start's
  tail keeps a long line's first 2000 bytes, as it keeps those of any line.

What is bounded, and what isn't (the issue's audit):

- Bounded: the agent's stdout and the editor's input (16 MiB queued, and
  up to 32 MiB of a line still coming, each); the agent's stderr (the same
  cap, plus a piece of 64 KiB); the link (16 MiB, and a frame is at most a
  line the host read or a piece); each peer's queue (16 MiB, plus the line that takes it past the cap and one more, ADR 6 and 49); the
  logger's queue (64 MiB, ADR 6); the foreground's display (ADR 9). The proxy
  reads the editor 64 KiB at a time.
- Not bounded by a count of bytes: the agent's message or thought being
  assembled from its chunks, for an `agent_message` event (each chunk has
  been through the cap; the message is as long as the agent makes it);
  what a session keeps (held messages and context, the last message,
  config options and commands), whose size the user and the agent choose;
  a line from a peer on the control socket and a started bridge's stderr,
  read as whole lines: they are the user's own processes (P13), not the
  session's pipes.

## Consequences

- A message over 32 MiB, valid JSON or not, isn't seen by the host: no
  events for it, and if it answered a request of the host's, the request
  goes unanswered, as for a line it can't parse (ADR 26). With an editor,
  the editor still gets it.
- An editor's line over 32 MiB goes to the agent as it is: a prompt that
  long isn't counted as a turn, a `session/load` that long isn't checked
  against the session's lock (ADR 3), nor an `initialize` that long
  stripped of `fs` and `terminal` (ADR 2). Editors don't send those that
  long; refusing them would mean reading them.
- The host's memory doesn't grow with the length of a line. A 128 MiB
  line to an editor that stopped reading leaves the host at about 54 MB:
  the line's first 32 MiB, queued for the link and written from where it
  is (a frame's payload isn't copied), and what the host holds anyway.
- The limit is twice the cap so that one message bigger than a peer's
  queue, which ADR 6 lets through to a peer that keeps up (20 MB in its
  tests), is still read.

## Considered

- Counting a line still coming against the cap, and so a limit of 16 MiB:
  one bound for everything, but messages ADR 6 promises to peers (20 MB)
  would no longer be read.
- A larger limit (64 MiB, say): more of the host's memory is the agent's to
  take, for messages no adapter sends.
- Cutting the line in the proxy role, or replacing it with an error: changes
  the editor's bytes (P1).
- Spooling a long line to disk so the host can still read it: memory
  bounded, but a parse of an unbounded line isn't, and disk use becomes the
  agent's to choose.
- Reading a long line leniently for its `id` and `method`: the guess
  ADR 26 rejected.
- An event at the line's end with its length: one that never ends would
  never be told.

## Tests

Run `cargo test --release adr_0051_`. Named claims and their assertions:

- [tests/headless.rs](../../tests/headless.rs)
  - `adr_0051_headless_an_agent_line_past_the_limit_is_dropped_and_said`.
  - `adr_0051_a_long_line_to_an_editor_that_stops_reading_is_held_back`.
  - `adr_0051_an_editor_line_past_the_limit_goes_to_the_agent_unread`.
  - `adr_0051_the_agents_stderr_without_a_newline_is_bounded`.
