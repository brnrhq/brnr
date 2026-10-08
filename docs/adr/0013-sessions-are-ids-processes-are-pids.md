# 13. Sessions are the agent's ids, processes are pids

Accepted (former decisions 37, 38 and 39); reviewed 2026-10-07. Implemented.

## Context

A `<target>` used to be a host id (the process's pid), a host's `--name`, a
session id or a unique prefix of one, and `--session` then picked a session
inside the host, or brnr picked the only one. Too many meanings, and the
picking hid that settings and messages belong to a session until a `fork`
broke it. Sessions also had names for a while: `start --name`,
`fork --name`, `brnr name`, at most one running session per name, a name
meaning the latest session that had it, the name coming back on `--resume`
unless in use, and an index of it all under `~/.brnr`.

## Decision

- `<session>` is a session's exact id, as the agent gave it, running or not,
  for everything about a session. `--pid <pid>` (or a `<pid>` argument) is a
  brnr process as `brnr ps` shows it, for what is about the process. No
  prefixes, no `--session`, no picking (P4). `watch` and `notify` take one or
  the other and have no default; `notify --stdin`, as a bridge, takes the
  events its process gives it instead (ADR 36).
- No names. `start --json` prints the session for a script to keep
  (`s=$(brnr start --json … | jq -r .session)`); anyone who wants shorter
  handles can keep them in their shell. The agent's own title for a session
  (`session_info_update`) is shown in `list` and `status` to tell sessions
  apart, and is never something to type.
- `brnr ps` lists brnr's processes: pid, agent, owner (`editor` or
  `headless`, or `unreachable` for one that doesn't answer, whose sessions
  are those it holds the locks of, ADR 3), uptime and their sessions.
  `brnr stop <pid>` stops a process and its agent: the agent's stdin
  closed, then SIGTERM to its process group after 5 s, then SIGKILL after
  5 more. Ending one session is `brnr close <session>` (ADR 16). `list` is
  about sessions, `ps` about processes.
- "Host" isn't in the CLI or its help; it stays the name of the process
  inside brnr.

## Considered

- Keeping `<target>` and documenting it better.
- Names: brnr's own layer over ACP's sessions, and a policy to learn (which
  session a name means, when one is taken, what resuming does to it) rather
  than a description of the agent's sessions (P2).
