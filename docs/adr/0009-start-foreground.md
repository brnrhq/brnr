# 9. `start --foreground`

Accepted (former decision 45); reviewed 2026-10-07. Implemented.
Resolves review item 3 with ADR 6.

Amended by 63: `start --foreground` is `session new --foreground` (and
`session resume --foreground`), with the same flags.

## Context

`brnr host` was the way to run a session in the foreground by hand, and its
flags lagged `start`'s. A foreground session is for a supervisor (systemd, a
container) or a terminal.

## Decision

- `start --foreground [--quiet] [--json]` runs the same process `start`
  does, as its child, in a process group of its own, passing it the signals
  it gets (Ctrl-C once stops the session, twice kills the agent). It shows
  the session's events on stdout as they come (`--json`: as JSON lines;
  `--quiet`: not), and exits as the agent did (128 + the signal that killed
  it), but never 0 if the start failed: 1 where the agent then exited 0
  (setting `--mode` failed, say), 127 or 126 where it couldn't be run, as a
  shell has it.
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

## Tests

Run `cargo test --release adr_0009_`. Named claims and their assertions:

- [tests/headless.rs](../../tests/headless.rs)
  - `adr_0009_foreground_start_failure_is_reported`.
  - `adr_0009_foreground_close_of_the_last_session`.
  - `adr_0009_foreground_outlives_its_stdout`.
  - `adr_0009_slow_foreground_reader_is_told_what_it_missed`.
- [tests/cli.rs](../../tests/cli.rs)
  - `adr_0009_foreground_start_shows_the_session`.
