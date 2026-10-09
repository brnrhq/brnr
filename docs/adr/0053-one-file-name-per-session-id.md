# 53. One file name per session id

Accepted 2026-10-08. Implemented. Amends 1 (P8), 3 and 22; resolves #59.

## Context

A session id is the agent's text, and becomes the name of the session's
transcript (ADR 22) and of its lock (ADR 3). P8 had it sanitized: anything
but ASCII letters, digits, `-`, `_` and `.` became `_`, and an events file
named `x.acp.jsonl` had its `.` turned into `_` so it wasn't taken for
session `x`'s raw ACP. That kept the name inside its folder, but not the
session's identity: `a/b` and `a_b` were one file, as were `x.acp` and
`x_acp`. Two sessions wrote one transcript, `list --inactive` showed only the
one that wrote last, and two processes serving them contended for one lock:
the second couldn't start, as if the first session were its own. On macOS,
whose file systems ignore case and Unicode normalization by default, `Sess-1`
and `sess-1`, or `é` composed and decomposed, were one file too.

## Decision

- A session id becomes a file name by escaping, not replacing: lowercase
  ASCII letters, digits, `-` and `_` stay as they are, and every other byte
  of the id's UTF-8 is `%` and two lowercase hex digits (`a/b` is `a%2fb`,
  `x.acp` is `x%2eacp`, `Sess-1` is `%53ess-1`). The empty id is `%`. The
  escaping is one-to-one, also where the file system folds case or
  normalization, so distinct ids never share a name: `paths::file_name`.
- The name has no `/` and no `.`: it can't leave its folder, is never `.` or
  `..`, and no events file can be named like another session's raw ACP.
- The namespace, the same for lookup, locks and logs: a session is its
  agent's id, exact (ADR 13, P4). Its lock is `sessions/<name>.lock`, one per
  id whatever the agent or folder. Its transcript is
  `projects/<folder>/<name>.jsonl` and `<name>.acp.jsonl`, one per id and
  folder, as a session resumed in another folder writes in that one.
  `list`, `log` and `--resume` find a transcript by the id its records
  carry, not by its file name. The original id is in every record and in the
  lock file.
- An id whose name doesn't fit in a file name (255 bytes with the
  `.acp.jsonl`; an escaped byte takes three) is not supported: its
  transcript fails to open and the host log says so, and its lock can't be
  taken (ADR 50).
- Existing files are neither migrated nor renamed (P9). Ids of lowercase
  letters, digits, `-` and `_` only, such as the UUIDs Claude Code's and
  Codex's adapters use, have the names they had. A transcript under a former
  name is still listed and read, by the ids its records carry; a session
  with such an id resumed now writes a new file under its new name. A file
  two sessions already share stays as it is.

## Consequences

- A claude-agent-acp session's events file still has the same folder and
  name as Claude Code's own transcript.
- Names of ids with capitals or other characters are longer and less
  readable; the id itself is in the records.
- The project folder stays as `~/.claude/projects` has it, which can put two
  cwds in one folder (`/a-b` and `/a/b`): their sessions are still apart,
  as ids are, and each record names its cwd.

## Considered

- Replacing, with a suffix (a hash of the id) only for ids that change: the
  names of ids that don't change stay readable, but a name then depends on
  a hash that can collide, and the rule is two rules.
- A hash of the id for every name: no longer the agent's transcript's name.
- Keeping capitals: one name per id on Linux, two ids in one file on macOS.
- Escaping the project folder too: it would no longer be the agent's own
  folder name, which is what joining with its transcripts relies on.
