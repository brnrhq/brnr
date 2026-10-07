# To resolve

Open questions from the code review of `brnr acp` and `brnr start`
(October 2026; the fixes that needed no decision are in [#15]). Each entry says
what the review found, what is already fixed, the options and a proposal.
Once one is decided, it moves to the decision log ([decisions.md]) with what
was chosen, and comes out of here.

[decisions.md]: decisions.md
[#15]: https://github.com/brnrhq/brnr/pull/15

## 1. Flow control on the editor link

**Found** (review finding 1, high): frames for the editor wait in an
unbounded queue (`link` in src/host/mod.rs) while the proxy can't take them.
In the review's test an editor that stopped reading cost about 315 MiB before
anything happened. [#15] made a write that blocks for `LINK_WRITE_TIMEOUT`
(30 s) count as the editor gone, so the agent no longer outlives it, but the
queue still grows without limit until then, and the agent never feels the
editor's pace. It is one case of a wider theme: the foreground's stdout (3)
and the internal channels aren't bounded either; only peers' queues are
(`QUEUE_BYTES`, 16 MiB).

- **A. The timeout alone** (as now). Memory is bounded only by how much the
  agent writes in 30 s.
- **B. Backpressure, and the timeout.** The link queue is capped (16 MiB, as
  peers'). Past it, `read_agent_stdout` stops reading, so the agent blocks
  on its stdout as it would writing to the editor directly. The timeout stays
  as the sign the editor is gone.
- **C. Backpressure without the timeout.** Exactly a direct pipe: an editor
  that stops reading stalls the agent for as long as it doesn't read.

**Proposed: B.** While the reader is blocked the host doesn't see the
agent's other output either (answers to injected prompts, permission
requests); that is what a direct pipe would do too.

## 2. Agent lines that don't parse

**Found** (review finding 4, medium): `agent_line` (src/host/acp.rs)
forwards a line serde_json rejects as it is, without tracking it: a lone
surrogate such as `"\ud83d"` (what `JSON.stringify` makes of a title cut in
the middle of an emoji), or nesting deeper than 128. If that line was a
request, the editor's answer is dropped as unknown (`editor-response-dropped`
in `editor_line`) and the turn hangs. Two more symptoms of the same cause:
such an answer to one of the host's own requests reaches the editor raw, and
an editor prompt that doesn't parse isn't counted as a turn.

- **A. Drop only answers the host already gave.** The host keeps the ids it
  answered itself (cancelled permission requests) and drops the editor's late
  answers to those; any other unknown id goes to the agent. An agent ignores
  an answer to something it never asked.
- **B. Pull the id out leniently.** Scan a line that doesn't parse for its
  `id` and `method`, and track it as usual.
- **C. Both.**

**Proposed: A**, plus keeping lines with a host id (`"id":"brnr-`) from the
editor. A guesses at nothing; B would have to be right about every way JSON
can break. An editor prompt that doesn't parse stays uncounted (nothing hangs
on it).

## 3. Foreground output

**Found** (review finding 5, medium): `emit` (src/host/control.rs) prints
the session with `println!` on the host's event loop.

- `brnr start --foreground … | head -1`: the host panics on the broken pipe,
  leaving no `exited` record and stale `.sock` and `.json` files.
- A slow reader blocks the whole host: the control socket, the agent's
  traffic and signals.

When stdout is closed:

- **A. Stop showing and keep running**, with a note in the host log.
- **B. Stop the session**, as SIGPIPE ends the writer of a pipeline.

When the reader is slow (the output on a thread of its own, as the editor
link is):

- **C. A bounded buffer, then drop**, saying so (`… 120 events not shown`).
- **D. An unbounded buffer.**
- **E. Block the host** (as now).

**Proposed: A and C.** The transcript has everything; what fails is only the
display, which shouldn't cost the session. B is the case for a supervisor
that means "stop" by closing the pipe; it can stop the process instead.

## 4. What ends a bridge, and `notify`'s stdin

**Found** (review finding 7, medium):

- A started bridge is dropped when its stdout closes (`read_requests` in
  `start_bridge`, then `PeerClosed`), though it is still running and reading.
  A bridge `exec cat > file` closes its stdout: it got no events and was gone
  in 4 ms.
- The README's `notify` bridge never reads its stdin, where the host writes
  its events. After 16 MiB the host drops it and sends it SIGTERM, and the
  notifications stop without a word.

Fixed in [#15]: a line that isn't UTF-8 no longer drops a peer, and `notify`
fails when it is cut off.

What ends a bridge:

- **A. Its stdout closing** (as now): a bridge that only listens has to keep
  its stdout open.
- **B. Its process exiting**: its stdout closing only means no more
  requests.

`notify` as a bridge:

- **C. It drains its stdin.**
- **D. It reads its events from its stdin** instead of connecting to the
  socket.
- **E. The host stops writing events to a started bridge's stdin** unless it
  subscribes.

**Proposed: B and C.** C changes no protocol. D makes `notify` behave one way
as a bridge and another way by hand, and E changes the bridge protocol
(started bridges are subscribed from the start, src/host/control.rs).

## 5. An event for a dropped held message

**Found** (review finding 8, medium): held messages dropped by `cancel`, by
`queue --drop` or `--clear`, or by closing their session go without an event.
`cancel` writes a transcript note and `queue` writes nothing. A
`send --wait` for such a message waits until the process exits.

- **A. `message_dropped`**, `{session, message, text, by}` with `by` one of
  `cancel`, `queue`, `close`; `send --wait` exits 1 on it.
- **B. `turn_ended` with a `dropped` stop reason**: misleading, since no
  turn ran.
- **C. Only the response says** (as now).

**Proposed: A.** It is a new public event: `EVENTS`, and the README's list
for bridges.

## 6. `model` and `config` while an editor is attached

**Found** (review finding 9, medium): decision 9 allows changing mode, config
options and model while an editor is attached because "the agent tells the
editor about the change". claude-agent-acp doesn't, for these: the new
options come back only in the response to `session/set_config_option` (or
`session/set_model`), which is the host's, so the editor never sees it, and
its model selector shows the old one.

- **A. Tell the editor.** The host sends it a `config_option_update` made
  from the response's `configOptions`.
- **B. Refuse them while an editor is attached**, as `fork` and `close` are.
- **C. Leave it** (as now).

**Proposed: A** for config options, model included where it is one;
**B** for `session/set_model` (codex's older API), which has no update to
send. Either way, decision 9's reason needs correcting.

## 7. `brnr start` killed before it sends the prompt

**Found** (review, med-low): `brnr start` sends the prompt over the control
socket once the session is open (`start` in src/ctl/talk.rs). Killed in
between, it leaves a session with no prompt. `started_ok` stays false
(`finish_start`), so `--stop-when-idle` never counts (`fire_idle_timers`) and
the process idles forever, `--stop-when-idle 0` included. The ready pipe
can't say so: the host is done with it.

- **A. The start deadline runs until the prompt arrives** (with
  `--awaiting-prompt`), and its passing fails the start as a timeout does.
- **B. The prompt comes with the start**: on a pipe to the host's stdin (as
  `brnr host --prompt -` reads it), with the attachments.
- **C. Idle time counts from the session opening**, prompt or not.

**Proposed: A.** It reuses the start deadline (`BRNR_START_TIMEOUT`). B
rebuilds how the prompt travels for one edge case, and C would close a
session whose prompt is only slow (at once, with `--stop-when-idle 0`).

## 8. `--resume` while the process serving it doesn't answer

**Found** (review, low-med): decision 10 refuses `--resume` of a session a
running process serves. A process that doesn't answer can't say which
sessions it has, so `find_session` fails with "no session …, and process N
is not answering". `start` ignores that error (`Err(_) => {}`) and resumes,
and two processes append to one transcript.

- **A. Refuse on that error**, saying how to get past it (`brnr stop N`).
- **B. Refuse only if the session's transcript was last written by the
  silent process**; resume a session only the agent knows (which brnr allows
  by design), whatever else is silent.
- **C. Resume** (as now).

**Proposed: B.** A would also refuse a session only the agent knows whenever
any process is silent, though nothing of brnr's can be serving it.

## 9. The agent's stderr

**Found** (review, low-med): the agent's stderr goes to the host log
(`Ev::AgentStderr`, src/host/mod.rs), and to the editor through the proxy,
but headless nowhere else. "claude CLI not found" is in neither the start's
error nor the foreground output; the start only fails with "the agent exited
before the session started".

- **A. A failed start's error ends with the agent's last stderr lines** (say
  20), in the background and the foreground.
- **B. The foreground shows the agent's stderr as it comes.**
- **C. B, behind a flag.**

**Proposed: A.** Adapters write a lot to stderr; it matters when the start
fails. A ring buffer of the last lines is enough.

## 10. MCP secrets in transcripts

**Found** (review, low): a profile's MCP servers go to the agent with their
`env` and `headers` (`GITHUB_TOKEN = "…"`, `Authorization`) in the
`session/new`, `load`, `resume` and `fork` requests the host sends. Those are
recorded in the host log and the session's transcript (`host_request`,
src/host/requests.rs), and sent to `acp` subscribers. An editor's own
`session/new` with MCP servers is recorded the same way. Transcripts are
readable only by the user, but the tokens are on disk once per session.

- **A. Redact the values** of `env` and `headers` in what is recorded and in
  the `acp` event; the agent gets them as before. An exception to decision
  1's raw ACP, for these requests only.
- **B. Keep them** (as now).
- **C. Don't record these requests.**

**Proposed: A.**

## To investigate: the host, agent and proxy dying together

Early in the review's editor-mode run, the host, its agent and the proxy
died together, without a word, twice. The reviewer thought an outside SIGKILL
most likely (cleanup from the other run), but couldn't pin it down. To find
out: whether the host log has an `exited` record, the proxy's exit status
(a signal, or "lost the connection to its process"), and whether it happens
without another run alongside.
