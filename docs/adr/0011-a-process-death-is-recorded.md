# 11. A process's own death is recorded

Accepted 2026-10-07. Implemented; the cause of the deaths below is still to
be found. What can't be recorded: a panic on the logger's own thread, and an
abort (a stack overflow), which runs no hook. A start that fails as it opens
the start channel (ADR 7) or its bridges records `exited` too, so a host log
without one is a death nothing recorded; `brnr doctor` lists those whose
process isn't running, with when each last wrote and the sessions it had
open.
From the review's "to investigate".

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

- A detached process's stderr goes to its host log, not `/dev/null`.
- A panic hook records the panic in the host log, writes `exited` (with the
  reason) there and in every session's file, sends it to the peers it can
  still reach, and removes the `.sock` and `.json` files (P3).
- The agent's command leaves the process's argv with the start request
  (ADR 8).

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
