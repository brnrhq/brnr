# 11. A process's own death is recorded

Amended by 45: a panic also prints a link to report it, in the foreground
from the hook, and from `brnr start` or `brnr acp` when it fails a start.

Accepted 2026-10-07. Implemented; the cause of the deaths below is still to
be found. What can't be recorded: a panic on the logger's own thread, and an
abort (a stack overflow), which runs no hook. A start that fails as it opens
the start channel (ADR 7) or its bridges records `exited` too, and so does
one that panics once the agent is spawned (with `panic`, the agent killed);
the logger never skips either (ADR 6), so a host log without `exited` is a
death nothing recorded; `brnr doctor` lists those whose process isn't
running (how many, and the latest three), with when each last wrote and the
sessions it had open. Whether metadata in the runtime directory is a
process's that is gone is told by its socket, not its pid, which may be
another process's by then (a reboot, pids wrapping around): one whose pid
isn't a live process of the user's, or whose socket refuses a connection
twice (nobody listens, or it isn't there), is gone. macOS also refuses a
stopped process's socket once its backlog is full, so there a refusal counts
only if the process with that pid started after the file was written.
`doctor --fix` removes what processes that are gone left behind; `list`,
`ps`, `status`, `start --resume` and every other command that looks
processes up drop them, removing their metadata and socket.
From the review's "to investigate".

Amended by 63: `brnr start` is `brnr session new` (and `start --resume`
`session resume`), and `list`, `ps` and `status` are `session list`,
`process list` and `session status`.

## Context

Early in the review's editor-mode run, the host, its agent and the proxy
died together, without a word, twice. Host logs on the maintainer's machine
(brnr 0.6.0 from Homebrew, IntelliJ IDEA 2026.2.3's AIR plugin as the editor)
show three editor processes whose log ends with no `exited` record, no
`editor-disconnected` note (0.6.0's `link_gone` writes one when the proxy
goes) and no `signal` note. The host never saw the proxy go and got no
signal it could catch: it was SIGKILLed, or it panicked. A detached host's
stderr is `/dev/null`, so a panic says nothing anywhere.

| Host log (`~/.brnr/hosts/`) | Last record (UTC) | At that moment |
|---|---|---|
| `20261007T011255-91876` | 01:15:41, the agent's stderr: "Claude Code process exited with code 143" | the review's test runs |
| `20261007T023306-61979` | 14:42:14, after 12 hours of `session/list` polls | 14:42:15: IntelliJ started three new agent processes |
| `20261007T143333-89992` | 15:50:41 | 15:50:40: IntelliJ started a new agent process for the same project |

One explanation fits all three, unconfirmed: something killing processes by
command line, such as `pkill -9 -f brnr-claude-adapter` from IntelliJ
cleaning up its agents, or from the review's clean-up. The host's argv had
the adapter's path in it (`brnr host --link-fd 3 … --
/opt/homebrew/bin/brnr-claude-adapter`), so the proxy, the host and the
agent would all die in the same instant, with no record.

## Decision

- On any agent exit, including a spontaneous crash during setup or a turn,
  the host SIGKILLs what remains in the agent's process group before reaping
  it (P14). Until then its pid and group id cannot be reused. Cleanup must
  not depend on a stop request having arrived first.

- A detached process's stderr goes to its host log (`host-stderr`), not
  `/dev/null` (with `log = false` there is no host log, and it stays
  `/dev/null`).
- A panic, on any thread, is recorded: the hook notes it and wakes the event
  loop, which records the panic in the host log, writes `exited` (with the
  reason) there and in every session's file, sends it to the peers it can
  still reach, removes the `.sock` and `.json` files (P3), and SIGKILLs the
  agent's process group (P14). Bridges get the time to act on `exited` they
  get on any exit (ADR 35).
- A panic as the start is under way, once the agent is spawned and before
  the event loop runs, is recorded and ends the agent the same way, as far
  as what the start has made allows (its log, bridges, the display; no
  sessions yet), and whoever started the process is told on fd 3, as of
  any start that fails: `brnr start` fails with the reason, an editor's
  `brnr acp` says it on stderr and exits 101.
- The agent's command leaves the process's argv with the start request
  (ADR 8).

## Failure testing

`tests/chaos.rs` injects seeded agent failures and kills the host, proxy or
bridge during a turn. A killed host cannot write `exited`: doctor reports
its incomplete log and repairs stale runtime files, and the session lock
can be taken again (ADR 3). A killed proxy leaves the host alive to record
its death and stop even a blocked agent. A killed bridge leaves the session
and its ownership intact (ADR 35). Setup faults never commit the prompt
(ADR 7).

The host is the only supervisor. SIGKILL closes its pipes but cannot make
it signal its children: a blocked agent, an EOF-ignoring bridge or their
descendants can outlive it. The host-SIGKILL test checks a flooding agent
that exits on its broken pipe, not unconditional child cleanup. A separate
supervisor and a decision about its own lifetime are needed for that
stronger promise; doctor does not kill processes inferred from stale pids.

## To find out

- Reproduce with IntelliJ restarting its agents, the first case for ADR 4's
  editor trials: watch with `ps` whether the old proxy, host and agent get
  SIGKILL, and from whom. Whether the user did something in IntelliJ at
  14:42 and 15:50 UTC (changed the agent's settings, reopened the AI chat,
  opened a project, updated the plugin) would say what to repeat.
- The proxy's exit status: killed by a signal, or "lost the connection to
  its process" (exit 1), which would mean the host went first.
- Whether it happens without another run alongside (the review's two cases
  had one).

## Tests

Run `cargo test --release adr_0011_`. Named claims and their assertions:

- [tests/headless.rs](../../tests/headless.rs)
  - `adr_0011_a_panic_is_recorded`.
  - `adr_0011_a_panic_while_starting_is_recorded`.
- [src/log.rs](../../src/log.rs)
  - `adr_0011_how_the_process_ended_is_never_skipped`.
