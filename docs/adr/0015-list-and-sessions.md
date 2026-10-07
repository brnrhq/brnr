# 15. `list` and `sessions`, and one row shape

Accepted (former decisions 36, 43 and 46); reviewed 2026-10-07. Implemented.

## Context

brnr knows sessions two ways: from its own transcripts and running
processes, in any cwd, with no agent to ask; and from an agent's own list
(`session/list`), which knows sessions started outside brnr, but only that
agent's, for one folder.

## Decision

- `brnr list [--inactive | --all]` is brnr's view of the machine: an index of
  the sessions running in its processes and those it has transcripts of.
- `brnr sessions [--profile <p>] [--cwd <dir>] [-- <agent>]` starts the agent
  just to ask it (`initialize`, `session/list` for the folder, then it
  exits): no brnr process, no target, and the folder is here unless `--cwd`
  says otherwise.
- Every table of sessions has the same columns, in the same order, formatted
  the same way: SESSION, TITLE, STATE, PID, (AGENT,) LAST ACTIVE, CWD;
  `sessions` drops AGENT, which it was given on the command line. STATE is a
  running session's live state (`idle`, `busy`, `waiting` for an approval),
  `inactive` for a transcript, `-` for one only the agent knows. LAST ACTIVE
  is the agent's `updatedAt`, or brnr's own when the agent gives none. Rows
  are ordered most recently active first, everywhere.
- `ps` has no CWD column: sessions have cwds (the process's is in its JSON,
  often `/` for an editor's).

## Considered

- One command: `list` would have to start an agent for every agent and cwd
  to ask it, and not every agent can list.
- A `sessions` request on a running process's control socket: gone, since it
  needed a running process in the right folder.
- Each table with its own columns (`UPDATED` against `LAST ACTIVE`, a `BRNR`
  column folding state and pid into one cell, timestamps formatted two ways)
  and its own order.
