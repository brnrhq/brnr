# 6. How far behind a reader may fall

Accepted 2026-10-07, from former decision 25. Implemented. While the
agent's stdin holds the link back, what the proxy sends behind the editor's
input (a signal, stdin's EOF) waits with it; the proxy going away doesn't.
Resolves review item 1, and with ADR 9 review item 3.

## Context

Every reader of a session can fall behind: a bridge or a watcher, the
foreground's stdout, the editor through the proxy, the agent reading its
stdin. In the review an editor that stopped reading cost about 315 MiB
before anything happened: frames for it wait in an unbounded queue (`link`
in src/host/mod.rs), as do frames for the agent's stdin (`agent_in`) and
everything the reader threads hand the event loop (its `mpsc::channel`),
while the foreground `println!`s on the event loop and blocks the whole
host. Only peers' queues were bounded. #15
then made a write to the editor that blocks for 30 s (`LINK_WRITE_TIMEOUT`)
count as the editor gone, which kills the agent.

## Decision

Who gives way depends on who the reader is (P6).

- Observers give way. A peer (a bridge, `watch`, `notify`, any control
  connection) may have 16 MiB queued that its writer hasn't written yet,
  counted from when a line is queued until it is written. A peer past that is
  behind and is cut off: a connection is shut down, a started bridge gets
  SIGTERM. Until then it takes the next line, however big. The foreground's
  display gets a bounded buffer, then skips events, saying how many
  (`… 120 events not shown`); its stdout closing ends the display, not the
  session (ADR 9). None of them ever slows the session.
- The session's own pipes get backpressure. The editor link's queue is capped
  (16 MiB, as peers'); past it the host stops reading the agent's stdout, and
  the agent blocks on its stdout as it would writing to the editor directly.
  The agent's stdin queue likewise: past the cap the host stops reading the
  link, the proxy blocks, and the editor's write blocks. No timeout of
  brnr's own: an editor that hangs stalls its agent for as long as it hangs,
  as a direct pipe would (P1). `LINK_WRITE_TIMEOUT` goes. An editor that is
  gone needs no timeout: its pipe closes, the proxy's write fails, and the
  host hears the editor has gone.
- The host's own channels are bounded too: a reader thread (the agent's
  stdout, the link) blocks when the event loop is that far behind, so
  "stops reading" really holds the bytes in the pipe, not in memory.
- While the agent's stdout is held back, observers see nothing of the agent
  either (no answers to injected prompts, no approvals); P1 ranks above them.

## Considered

- A count of lines (first 4096 per peer, then more of them): still a count,
  and lines vary from a few bytes to megabytes. On a slow CI runner a raw
  `watch` that was reading the whole time fell more than 4096 lines behind an
  agent streaming 20,000 chunks at once, and was cut off as if it had
  stopped.
- Dropping `acp` events for a slow peer instead of cutting it off: lossy
  where `--events all` promises everything.
- "Behind" as "the next line would take it past the limit": it cut off peers
  that were keeping up when one big agent message (10 MB, at the end of a
  burst) came while they were a few MB behind (seen on the macOS CI runner);
  before that, one message over the limit cut off every peer, `brnr status`
  included. The status report quotes at most 4000 characters of the last
  message; the events have all of it.
- The 30 s timeout alone (review item 1, A, as #15 left it): memory bounded
  only by how much the agent writes in 30 s, and the agent never feels the
  editor's pace.
- Backpressure plus the 30 s timeout (review item 1, B): memory is bounded
  without it, and it kills the agent of an editor that only paused.
- An unbounded buffer for the foreground (more of the memory problem), or
  blocking the host on it (it stops the control socket, the agent's traffic
  and signals).
