# Files and compatibility

## Files

| Path | What it is |
|---|---|
| `$BRNR_HOME/projects/<folder>/<session id>.jsonl` | A session's events file, its transcript. `<folder>` is the session's cwd, every character that isn't a letter or a digit as `-`, and a hash of the cwd. |
| `$BRNR_HOME/projects/<folder>/<session id>.acp.jsonl` | The session's raw ACP, beside it. |
| `$BRNR_HOME/hosts/<run id>.jsonl` | A process's host log: what belongs to no session. |
| `$BRNR_HOME/sessions/<session id>.lock` | A session's lock ([ADR 3.9](../adr/0003-processes-sessions-and-ownership.md#39-ownership-is-a-per-session-lock)). |
| `$BRNR_DIR/<pid>.sock`, `$BRNR_DIR/<pid>.json` | A process's control socket and metadata. |

[Transcripts](../guide/transcripts.md#transcripts) has the paths, the
records' process and session fields, the logging modes, redaction and the
`records_skipped` gap notes. Read history through `event log` where you can.

- Every record starts with its `format`, 1 for all of 1.x. A record in
  another format, an older brnr's without one or a newer one's, isn't read,
  and `event log` and `doctor` say so
  ([ADR 5.10](../adr/0005-events-transcripts-and-observers.md#510-what-a-transcript-promises-and-how-its-format-is-marked)).
- Nothing is appended to a file in the way: a session opened where one is
  sets it aside, renamed `<name>.older.jsonl` (its raw ACP
  `<name>.older.acp.jsonl`), and starts a file of its own. A
  `session_log_moved` host-log record (`path`, `to`) says so.
- A transcript is not a promise of complete history: a disk's backlog or
  errors, logging turned off, and a process that dies at once can lose
  records.

**Covered** from 1.0.0: each session's events and raw ACP files, where they
are under `$BRNR_HOME`, and their records, in format 1; a 1.x brnr reads what
an earlier 1.x wrote (ADR 5.10); and where each other file under
`$BRNR_HOME` is. **Not covered**: what host logs and session lock files
contain, and what is in the runtime directory: sockets and metadata files.

## Output safety

- Text output escapes the agent's control characters. JSON keeps the agent's
  text as it is: escape it before you show it in a terminal or a UI.
- MCP servers' `env` and `headers` values are redacted from the recorded
  ACP, but transcripts can contain prompts and tool output.
- brnr writes a transcript or host log only where the state directory, each
  directory below it and the file are the user's own, with no group or other
  access. What others can reach is made private before the next record (a
  `path_made_private` host-log record says so, or, from `session delete`, a
  line on stderr). A symlink, another user's path, a file that isn't a
  regular file, or one with other hard links is refused, with no override
  ([ADR 9.3](../adr/0009-security-and-privacy.md)). A refused host log fails
  the start; a refused session file is a `session_log_failed` host-log
  record.
- `doctor --report` is for review before you share it; it uploads nothing.

See the [threat model](../threat-model.md) for the trust boundaries and the
documented cleanup limit when a process is killed with SIGKILL.

## Compatibility

Before 1.0.0 every interface here can change outright, with no aliases or
shims (P9). From 1.0.0, what these pages mark **Covered** is kept compatible
within a major version, and anything they don't name is not covered
([ADR 8.4](../adr/0008-the-command-line-and-compatibility.md#84-what-the-compatibility-promise-covers)).
The library on crates.io is covered as part of the command line: its public
API is a facade that mirrors it (#147, ADR 8.5); its internals, behind a
feature that is off by default, are not. [Error codes](errors.md) are
covered.

- A minor release can add a command, flag, JSON field, event, event field,
  bridge request, config key, or an exit code for a situation that had none.
- Only a major release removes or renames any of those, changes a field's
  type or what it means, or changes the exit code of a situation that has
  one.
- Ignore the fields and events you don't know.
- A deprecated form keeps working until the next major version; the
  [changelog](../project/changelog.md) says what to use instead.

Security fixes are made on the latest major version; see
[SECURITY.md](https://github.com/brnrhq/brnr/blob/main/SECURITY.md). Before
1.0.0-rc.1, the names and shapes of `--json` output (#150) are still being
settled.
