# 9. `start --foreground`

Accepted (former decision 45); reviewed 2026-10-07. Implemented, except: a
closed or slow stdout (below), and the agent's stderr (ADR 10).
Resolves review item 3 with ADR 6.

## Context

`brnr host` was the way to run a session in the foreground by hand, and its
flags lagged `start`'s. A foreground session is for a supervisor (systemd, a
container) or a terminal.

## Decision

- `start --foreground [--quiet] [--json]` runs the same process `start`
  does, as its child, in a process group of its own, passing it the signals
  it gets (Ctrl-C once stops the session, twice kills the agent). It shows
  the session's events on stdout as they come (`--json`: as JSON lines;
  `--quiet`: not), and exits as the agent did, or 1 if the start failed
  (setting `--mode`, say).
- `--wait` and `--foreground` don't go together: the foreground shows the
  whole session.
- stdout closed (`brnr start --foreground … | head -1`): the display stops,
  with a note in the host log, and the session carries on. The transcript
  has everything; what failed is only the display, which shouldn't cost the
  session. A supervisor that means "stop" stops the process.
- A slow reader: the display runs on a thread of its own, with a bounded
  buffer, then skips events, saying how many (ADR 6). It never blocks the
  host.
- stderr carries brnr's own messages and the agent's stderr (ADR 10).

## Considered

- stdout closing stopping the session, as SIGPIPE ends the writer of a
  pipeline.
- An unbounded buffer, or blocking the host (as was: `println!` on the event
  loop, where a broken pipe panicked the host and left no `exited` record and
  stale `.sock` and `.json` files).
