# 14. Resuming

Accepted (former decisions 10 and 36); reviewed 2026-10-07. Implemented,
except the ownership lock and `--take-over` (ADR 3).

## Decision

- `brnr start --resume <session>`, with the session's exact id (ADR 13).
- `session/resume` when the agent offers it (no history replay), else
  `session/load`. While a load replays history, the host neither records nor
  emits the replayed updates: they are in the transcript already. What they
  say the session is now (its title, mode, config options and commands) is
  kept.
- An id brnr has a transcript of brings its cwd, agent and profile from it,
  the session's own record (P4); `--cwd` or `-- <agent>` override. An id
  brnr doesn't know (one `brnr sessions` lists) goes to the agent as given,
  in `--cwd` (or here), with the agent after `--` (or the profile's).
- A session another process holds is refused, unless `--take-over` (ADR 3).
- It appends to the same transcript.

## Considered

- Refusing an id brnr has no transcript of: `brnr sessions` listed sessions
  that `--resume` then refused.
- Prefixes of ids: gone with ADR 13.
