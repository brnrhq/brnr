# Transcripts

Like the agents' own transcripts, keyed by project folder:

```text
~/.brnr/projects/<folder>/<session id>.jsonl       # a session's events
~/.brnr/projects/<folder>/<session id>.acp.jsonl   # its raw ACP
~/.brnr/hosts/<run id>.jsonl                       # each process's, for what belongs to no session
~/.brnr/sessions/<session id>.lock                 # held by the process serving the session
```

This layout, how `~/.brnr` and the runtime directory are found, and how a cwd
and a session id become names (below) are fixed for 1.x (ADR 3.9); what the
host logs and the locks contain isn't (ADR 8.4).

`<folder>` is the session's cwd with every non-alphanumeric character replaced
by `-`, as in `~/.claude/projects`, then `-` and 8 hex digits, a hash of the
cwd (`-Users-me-src-3db3a304` for `/Users/me/src`), so `/a-b` and `/a/b` have
a folder each. A claude-agent-acp session's events file has the same name as
Claude Code's own transcript, in a folder whose name starts with Claude
Code's. A transcript an earlier brnr wrote in a folder without the hash stays
there, and isn't read (below); a session resumed goes on in its cwd's folder
(ADR 9.2). One resumed in another cwd (`--cwd`) so has a file in each
folder, and they are one transcript: `event log` reads them all, merged by
time, `session list` has one row of it, and `session resume` takes the cwd,
agent and profile of the newest record of them all (ADR 5.1). In
`<session id>`, as in a lock's name, every byte but lowercase ASCII letters,
digits, `-` and `_` is `%` and two hex digits (`a/b` is `a%2fb`), so two
sessions never share a file. The events are the ones bridges get, `process_exited`
included, and are what `brnr event log`, `session list` and `session resume`
read; `event log` reads the raw file too when `acp` events are chosen. A
thread of the process's own writes them, so the session never waits for the
disk; `event log` of a running session first waits until its process has
written what it recorded until then (up to 5 s, then it says so and shows what
is there), so a turn `session new --wait` just reported is in it, and a
process that exits is listed until its transcript has its `process_exited` (2 s at
most). A process killed (SIGKILL) loses what it hadn't written yet, and may
leave its last record cut short: the session is still listed, logged and
resumed as of its last whole record, `event log` says on stderr which lines it
couldn't read, and the next process starts on a new line; brnr never rewrites
a transcript. Every record starts with its `format`, 1, then carries `ts`,
`host_id`, `pid` (the brnr process), `proxy_pid`, `agent_pid` and, in a
session's file, `session`, for joining: `proxy_pid` is null for a headless
process, and the `session_deleted` that `session delete` records is no
process's, its `host_id`, `pid` and `proxy_pid` null. A session's events
file starts with `session_opened`, as the host log records it too: the
`session`, its `cwd`, the process's `profile` and `agent`, its
`events_log`, `acp_log` and `host_log`, and `forked_from` for a fork. That
is what `session resume` resumes a session with: the transcript alone, not
the host log (ADR 3.11).

The format stays 1 for all of 1.x, and every 1.x reads every 1.x's
transcripts (ADR 5.10, ADR 8.4). A record in a format brnr doesn't read,
an older brnr's without `format` or a newer one's, isn't read: `session
list` and `session resume` pass its file over, `event log` says
`<file>: records in format 2; this brnr reads format 1, not shown`, and
`brnr doctor` lists it. Nothing is appended to such a file: a session
opened where one is in the way renames it `<name>.older.jsonl`
(`<name>.older.acp.jsonl` for its raw file), which the host log's
`session_log_moved` says, and starts a file of its own. The events are a summary: an agent message's text,
a tool call's title, kind, status and locations; images, diffs and tool
output are in the raw file only. `log = "events"` leaves out the
raw file, most of the space; `log = "off"` writes nothing but the session
locks.

## How long brnr keeps them

By default, until you delete them: brnr deletes nothing on its own. Two ways
prune by age (ADR 9.5):

```sh
brnr session prune --older-than 2592000 --dry-run   # what was last active more than 30 days ago
brnr session prune --older-than 2592000             # delete it
```

```toml
retention = 2592000   # at the top of config.toml: every process prunes so as it starts
```

- The age is in seconds, as every duration in brnr is. `brnr doctor` says
  the retention in days, so a value meant in days shows.
- A session's age is its last activity, `session list`'s LAST ACTIVE: the
  newest record a process wrote for it, not when it began, and not a
  `session delete`. Its files go as `session delete --purge` deletes them:
  every project folder's events and raw ACP, and those set aside.
- A process's host log goes once the process has gone and its newest record
  is that old. A process that runs longer keeps its host log until it exits.
- Never pruned: a session open in a process, or whose lock is held; the
  session a start resumes; and a file brnr can't read the age of (another
  format, which `doctor` lists, or no record).
- `--older-than 0` prunes every session no process has open. `retention = 0`
  fails to load: leave `retention` out to keep everything, or set a
  profile's `log = "off"` to record nothing.
- The agent's own copy of a session stays: `brnr session delete` asks the
  agent to delete it.
- A process records what it pruned in its host log, `transcripts_pruned`.
  `--json` has the same fields, and `--dry-run` deletes nothing.

## Gaps, secrets and privacy

Records skipped behind a slow disk are counted, and a `records_skipped` note
(`count`, `acp_count` of them raw ACP, `since`, `until`, and the positions of the
events among them, `first_seq` and `last_seq`, with `ts`, `host_id`, `pid`
and `session` as an event has them) marks the gap in the host
log and in each session's events file that lost some. Records a full disk
didn't take are noted likewise, with the `error`, in the file that lost them
(a raw file's in its events file) once it takes records again. How a process
ended (`process_exited`, or `process_panicked`) is never skipped, so `brnr doctor` can tell an
exit from a death. A process that exits waits for its records to be written,
however long the disk takes, once it is no longer listed and has let go of
its sessions; a signal (Ctrl-C, `kill`) gives up on them, and it says on
stderr how much it didn't write. `brnr event log` always shows a gap,
whatever `--events` chose.

The values of MCP servers' `env` and `headers` are recorded as
`"<redacted>"`, in the raw ACP, the host log and `acp` events alike; the agent
gets them as given.

Transcripts hold prompts and tool output, so brnr writes them only where only
you can read them (ADR 9.3): `~/.brnr` (or `$BRNR_HOME`), each directory below
it and the file must be yours, with no group or other access, before a record
goes in, whether brnr made them or found them there. It creates them 0700 and
0600; one restored, copied or chmod'ed so that others can reach it is made
private when a process opens it, and the host log has a `path_made_private`
record (`path`, and the `mode` it had; `session delete`, which has no host
log, says so on stderr). Symlinks are never followed, the
state directory included (set `BRNR_HOME` to where one points instead), and
what isn't your own directory or file, or has another hard link, is refused:
a process whose host log can't be opened doesn't start (`log: <path>: …`),
and a session whose file can't be is served without it, with a
`session_log_failed` record in the host log saying why. On macOS, one with an
access control list (`ls -le`), which can let others in whatever the mode
says, is refused the same way, with the `chmod -N` that removes it; brnr
doesn't remove it. What reads them (`session list`, `event log`, `doctor`, …)
keeps the same rules: nothing is read through a symlink, nor what isn't
yours or has another hard link or an access control list. There is no
override; `log = "off"` writes no transcripts.
