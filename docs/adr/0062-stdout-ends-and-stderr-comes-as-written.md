# 62. The editor's stdout ends when the agent's does, and its stderr comes as written

Proposed 2026-10-09. Implemented. Amends 2 (the editor's stdout ends with
the agent's, and brnr writes nothing more there), 10 (the agent's stderr
goes on as it is read) and 51 (stderr's 64 KiB pieces are the host log's
records, not what the editor waits for).

## Context

Run directly, an agent's stdout and stderr are two pipes of the editor's.
Each ends when the agent closes it, and each delivers what is written as
soon as it is written. Through `brnr acp` neither did:

- The host saw the agent's stdout end, but used that only to decide when to
  exit (`stdout_open`). The editor's stdout ended when `brnr acp` exited,
  with the agent. To an editor that reads stdout to EOF, an agent that closes
  its stdout and runs on (to finish a log, or wait for a child) still looked
  alive.
- The host read the agent's stderr a line at a time (up to 64 KiB, ADR 51)
  and sent each line on once it ended. A prompt that waits for an answer
  ends no line: `login required: ` showed only once the agent wrote a
  newline or exited.

Both broke P1. They also raise the question of who owns the editor's stdout
once the agent's has ended. brnr writes lines of its own there: the tool
calls that show a message sent from outside (ADR 5), refusals of a load of
a session another process holds (ADR 3), and the `$/cancel_request` that
withdraws a request answered from outside (ADR 4).

## Decision

- The editor's stdout ends when the agent's does, after the last of what
  the agent wrote, whether the agent runs on or not. The host sends the
  proxy `EOF` on the link after the last `DATA` frame (frame.rs), and the
  proxy ends its stdout. Its fd 1 is left open on /dev/null, so no file
  opened later can take its place. The same happens when the editor stopped
  reading and the host closed the agent's stdout for it (ADR 2). The host
  log records `agent-stdout-ended`.
- The editor's stdout is the agent's (ADR 2). Once it has ended, brnr
  writes nothing more there of its own. A line it would have written (a
  message's tool call, a refusal, a withdrawal) isn't sent, and the host log
  says so with `not-sent-to-editor`, its reason, session, method and id
  (P3). What the line was about still happens: a message sent from outside
  still reaches the agent's stdin, which is open. Only the editor's copy is
  lost, as the agent's answer to it would be.
- The agent's stderr goes on as it is read, newline or not. Each read (up
  to 64 KiB) goes to the editor's stderr through the proxy, and in the
  foreground to its stderr (ADR 10), unchanged: every byte once and in
  order, before the exit status.
- What keeps stderr as lines does so alongside, without holding anything
  back. The host log has a record for each line, or for each 64 KiB of a
  longer one (ADR 51). The line still coming at the end (when the agent's
  stderr ends, or the host exits) gets one too. A failed start's tail
  (ADR 10) ends with the line the agent is in the middle of: a prompt's
  text, without its answer.
- The editor's stderr still ends when `brnr acp` exits, not when the agent
  closes its stderr. It is also where `brnr acp` reports its own failures
  (losing its process, a panic's link: ADR 11 and 45), which ending it early
  would lose (P3).

## What the editor still sees differently

Run directly, the agent's stdout and stderr are two pipes. Each has the
kernel's buffer (up to 64 KiB on Linux and macOS), and only its own reader
holds it back. Through brnr both go on one link, in the order the host read them,
under one cap (16 MiB, ADR 6). This decision changes none of that; it
stays:

- An editor that stops reading one stream eventually stops the other. The
  proxy writes frames in order, so stderr waits behind a stdout write that
  blocks, and the other way round. Past the cap the host stops reading both
  of the agent's pipes, and the agent blocks writing to either. Run
  directly, an agent blocked on stdout could still write to stderr. Editors
  read both streams, and an ACP agent waits on its stdout's reader anyway.
- Until the cap is reached, the agent's writes complete without the editor
  having read them: up to 16 MiB, rather than a pipe's 64 KiB. Nothing is
  lost or reordered within a stream.
- An editor that closes its end of stderr gives the agent no EPIPE. The
  proxy's writes there fail and are dropped, and the agent's stderr still
  goes to the host log. (Closing stdout does give EPIPE, ADR 2.)

## Consequences

- An editor that reads stdout to EOF to see the agent go sees EOF when the
  agent closes stdout. It gets the exit status when the agent exits, as it
  would directly.
- The proxy gets a `STDERR` frame for each read rather than each line: 5
  bytes of header each.

## Considered

- Keeping the editor's stdout open for brnr's own lines after the agent's
  has ended: the editor wouldn't see the agent's stdout end, which is what
  needed fixing, and the lines would be answers and updates that no agent
  stands behind.
- Closing fd 1 outright in the proxy: the next file it opened would get
  fd 1, and a stray write to stdout would go there.
- Ending the editor's stderr with the agent's: `brnr acp`'s own failures
  would go nowhere (P3).
- A link of its own for stderr (a third socketpair, with its own cap and a
  thread on each side): independent backpressure, as with two pipes, at a
  cost every editor's process pays, for a difference no editor is known to
  depend on. Still open, if one turns out to.
- Sending stderr a line at a time, with a timeout for a line that doesn't
  end: still late, by a delay brnr chooses, and a timeout of brnr's own,
  which ADR 6 rules out.

## Tests

Run `cargo test --release adr_0062_`. Named claims and their assertions:

- [tests/security.rs](../../tests/security.rs)
  - `adr_0062_the_editors_stdout_ends_with_the_agents`.
  - `adr_0062_stderr_comes_as_it_is_written`.
  - `adr_0062_stdout_and_stderr_end_on_their_own`.
- [tests/headless.rs](../../tests/headless.rs)
  - `adr_0062_a_timed_out_start_shows_the_line_stderr_is_in`.
- [src/host/fuzz.rs](../../src/host/fuzz.rs)
  - `adr_0062_brnrs_own_lines_end_with_the_agents_stdout`.
  - `adr_0062_stderr_goes_on_before_its_line_ends`.
