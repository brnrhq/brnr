# 7. A start is atomic and commits at ready

Accepted 2026-10-07. Implemented.
Amended by 63: `brnr start` is `brnr session new`, and `start --resume`
`brnr session resume`; each is a start as described here. With `--pid`, in
a running process, the commit is the process's answer on the control
socket, queued while the command is still connected; the command gone
before it, a setting failing, or the start timeout passing closes the
session again (or, for an agent that can't close sessions, the error names
it), and the prompt is never sent. The timeout goes in the socket request
and is the process's, as it is in a start's; the command keeps the same
fallback, 10 s later. The process stopping meanwhile fails the opening too,
and the session ends with the process.
Replaces former decision 4; amends former decisions 21 and 41; resolves
review item 7.
Made exact 2026-10-08 (#67): the process used to queue the report for a
writer thread and send the prompt at once, so a `start` gone between the
two went unnoticed and the session ran unseen. The commit is now the
report's write succeeding, below.

## Context

`brnr start` used to wait for the session, then send the prompt over the
control socket like `brnr send` (former decision 4): `start --wait`,
`--file` and `--image` shared `send`'s code, and `start` subscribed before
sending so it never missed the reply. A start was then two conversations with
the process, the ready pipe until the session opened and the control socket
after it, with a gap between them:

```text
 brnr start                         brnr process                     agent
  launch, ready pipe ──────────────▶ bind socket, spawn agent ─────────▶
                                     initialize, session/new,
                                     set_mode, set_config_option ───────▶
  read ready ◀────────────────────── ready {session}
 ┄┄┄┄┄┄┄┄┄┄┄┄┄┄ start killed here: a session with no prompt ┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄
  connect, subscribe, send ────────▶ session/prompt ────────────────────▶
```

Killed in that gap, `start` left a session with no prompt, and
`--awaiting-prompt` kept its idle clock from starting, so it ran forever,
`--stop-when-idle 0` included. Former decision 4's own reason, "a start that
dies before sending simply sends nothing", was exactly the bug. A `start`
that died while the session was being set up was only noticed when writing
the ready line failed.

## Decision

- `start` reads all of its input before launching the process: `--prompt`
  (from its own stdin with `-`), `--file`, `--image`, and resolves the rest
  of the start (ADR 8).
- It launches the process with a pipe as its stdin and the start channel, a
  socketpair, as fd 3; it writes one start request on the pipe and closes
  it. The process reads its stdin to EOF before doing anything else; cut
  short (`start` died mid-write), it refuses to start.
- The host reads the start channel on a thread like every other source:
  EOF on it before the commit means `start` has gone, and the process stops
  at once, whatever it was doing (`start-abandoned`).
- The commit is the ready report, `{pid, session, message}`, written on the
  start channel once the session is open and its mode and config options are
  set, and before the agent gets the prompt (`message` is the prompt's
  `m<n>`, ADR 17). Until then anything failing (a setup step, the timeout,
  `start` going away) stops the process, and the prompt is never sent. After
  it the process sends the prompt, and the session runs on its own whatever
  `start` does (P14).
- The commit is the report's write succeeding. Nothing else is written on
  the channel before it, so the event loop writes the report itself, in one
  write that doesn't wait; one that fails (`start` has gone, its EOF perhaps
  not read yet) abandons the start as EOF does. A write that succeeds is all
  the process can know: `start` may still go before it reads the report,
  and the session then runs on, as after any commit, in `list` and its
  logs.
- Committed, the start channel becomes a peer, subscribed (with `--wait`) to
  the turn's events before the prompt goes, so it misses none of them.
- Without `--wait`, `start` prints the session and its process and exits
  after `ready` (`--json`: `{session, pid, message}`, `message` null without
  a prompt). With `--wait`, it keeps reading the same channel: the turn's
  events, until the `turn_ended` that carries its message (ADR 21). There is
  no reconnect and no race.
- The start timeout (`BRNR_START_TIMEOUT`, 120 s) goes in the request and is
  the process's: it fails the start when the time passes. `start` keeps only
  a fallback timer, the timeout plus 10 s, for a process stuck too badly to
  report.
- Gone: `--awaiting-prompt`, the idle clock waiting for the prompt (ADR 12),
  `start` stopping the process when its `send` fails (former decision 21),
  and `start` looking the process up again to connect.

```text
 brnr start                         brnr process                     agent
  read prompt, files, images
  launch: stdin = request,
  fd 3 = start channel ────────────▶ read request to EOF
                                     bind socket, spawn agent ─────────▶
                                     initialize, session/new,
                                     set_mode, set_config_option ───────▶
  read ready ◀────────────────────── ready {pid, session, message}  ← commit
  (no --wait: exit)                  session/prompt ────────────────────▶
  --wait: read events ◀───────────── … turn_ended
```

## Consequences

The README's promise now holds for the whole start: if it gives up, or is
interrupted, the process stops too and the prompt is never sent.

## Considered

- Keeping the prompt on the control socket and closing the gap: the start
  deadline running until the prompt arrives (review item 7, A) left a session
  nobody used for up to 120 s; keeping the ready channel open until the
  prompt arrived closed it, but kept two conversations.
- Review item 7 itself rejected sending the prompt with the start (B, the
  option now taken) as rebuilding how the prompt travels for one edge case.
  The review of the principles turned that around: the gap wasn't one edge
  case but a start made of two conversations (P10), and closing it removes
  `--awaiting-prompt`, former decision 21's clean-up and the second
  connection.
- Idle time counting from the session opening (C): it helps only with
  `--stop-when-idle`, and closes at once a session whose prompt is just slow.
- The prompt on the process's command line: Linux caps one argument at
  128 KiB and macOS all of them at 1 MiB, and any user can read arguments
  with `ps` (P13).
- Committing at launch: a killed `start` would leave a running session whose
  id nobody was told, and Ctrl-C couldn't cancel a start.

## Tests

Run `cargo test --release adr_0007_`. Named claims and their assertions:

- [tests/headless.rs](../../tests/headless.rs)
  - `adr_0007_abandoned_start_sends_no_prompt`.
  - `adr_0007_start_gone_as_ready_is_written_sends_no_prompt`.
  - `adr_0007_start_gone_after_ready_leaves_the_session_running`.
  - `adr_0007_start_timeout_stops_the_host`.
  - `adr_0007_empty_prompt_is_refused`.
  - `adr_0007_large_prompt_from_stdin`.
