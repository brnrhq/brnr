# 22. The host's events are the story, and transcripts are two files

Amended by 48: `log` of a running session first waits until its process has
written what it recorded until then. Amended by 55: what reads a transcript
passes over a line it can't read, cut short or not, and `log` says so.

Amended by 53: the session id in the file names is escaped, one name per
id, so distinct sessions never share a transcript.

Accepted (former decisions 1, 34 and 35); reviewed 2026-10-07. Implemented.
Of the config options and the commands, `session_changed` has a JSON merge
patch (RFC 7396) by id and name: an option's new value (`{"model":
"opus"}`), a command added (`{"review": {…}}`), `null` for what is gone. An
answer to `set_config_option` that changes a value makes one too, as an
agent needn't send an update for it. Records the logger skips behind a slow
disk (ADR 6) leave a gap that a `records-skipped` record counts, in the
session's events file and the host log; so do records a write fails to put
in a file, in the file that lost them once it takes records again (the raw
file's in its events file). `log` shows such a gap whatever `--events`
chose.

## Context

The transcript records raw ACP. A readable history needs the interpretation
the host already does (assembling agent messages, pairing permission
requests with their answers, naming a turn's end), and the CLI can't know the
host's names for things (permission handles like `p1`, message ids).

What the transcript is for, as brnr uses it: `brnr log` for running and
ended sessions; `list --inactive` and `--all`, the index of ended sessions;
`--resume` taking a session's cwd, agent and profile from it (ADR 14); and
the raw ACP, for `log --events acp` and for checking the interpretation.
brnr has never replayed it: when an agent replays a session
(`session/load`), the host doesn't record the replay, and `session/resume`
replays nothing. The agents keep transcripts of their own (Claude Code's in
`~/.claude/projects`, under the same folder names).

On the maintainer's machine in October 2026, 44 session files held 31 MB:
74% raw ACP, almost all of it the agent's `session/update`s to the editor;
26% events, of which `session_changed` was 5.5 MB in 176 records, each
repeating whole lists of config options or commands.

## Decision

- Every event the host emits to bridges (`user_message`, `agent_message`,
  `tool_call`, `permission_request`, `turn_ended`, …) is also a record in the
  session's events file. `log`, `watch` and the foreground render the same
  events, and a transcript read back is the stream a bridge saw live (P5).
- Each session has two files, keyed by project folder like the agents' own:
  `~/.brnr/projects/<folder>/<session id>.jsonl`, its events, and
  `<session id>.acp.jsonl` beside it, the raw ACP. `<folder>` is the cwd with
  every non-alphanumeric character replaced by `-` (as in
  `~/.claude/projects`). Each process also has `~/.brnr/hosts/<run id>.jsonl`
  for what belongs to no session (ACP before a session exists, the agent's
  stderr).
- The events file is always written: it is what `log`, `list` and `--resume`
  read. The raw file is written by default; a profile's `log = "events"`
  leaves it out, and `log = false` writes neither (ADR 33).
- `log` reads a record as the event `watch` would have shown. With `acp`
  events chosen it reads the raw file too, merged in order, each ACP message
  an `acp` event, and says so when there is none. `--events` and `--json`
  mean the same as for `watch`, and `log --json` is what a watcher got live.
- `exited`, the one event that belongs to the process, is written into every
  session's events file too, so `log` keeps to one session's files.
- `session_changed` records what changed, not the whole list again.
- Every record carries `host_id`, `host_pid`, `proxy_pid` and `agent_pid` for
  joining. All of it is readable only by the user (P13).

## Consequences

- A process holds a session's two files only while it serves the session:
  closing it closes them once its last record, `session_closed`, is
  written, and a session opened again appends to them, after another
  `session-opened`. A process that forks and closes session after session
  keeps as many files open as it has sessions.
- Turning the raw file off saves most of the space, and leaves secrets
  (ADR 25) nothing to be redacted from but the host log.
- The raw file keeps replay possible: the agent's `session/update`s to the
  editor are what a replay to an editor that resumes a session without
  history (ACP v2 has no `session/load`) would resend. Whether brnr offers
  that is undecided; it would be an experimental editor action (ADR 4).
- Transcripts written before events existed have none; `log` says so.

## Considered

- One file with raw ACP and events together (as was).
- Events only, raw ACP only on request: a quarter of the space, but checking
  the interpretation, and any replay, would need it turned on in advance.
- Raw ACP only, the events derived when read: a second interpretation in the
  CLI, without the host's names.
- `log` also reading the host log for the session's time span: two files to
  merge and to follow, and the host log has ACP and stderr that aren't the
  session's.
- For `log` (former decision 34), keeping the record dump and adding
  `--events` over the records: `--raw` would have kept two meanings, one for
  `log` and another for `watch`.
- Host-wide events copied into every session's file (former decision 35):
  the case that prompted it, `owner_changed`, went with ADR 2; `exited` is
  what remains.

## Tests

Run `cargo test --release adr_0022_`. Named claims and their assertions:

- [tests/headless.rs](../../tests/headless.rs)
  - `adr_0022_transcripts_are_two_files`.
  - `adr_0022_log_events_leaves_out_the_raw_acp`.
- [tests/cli.rs](../../tests/cli.rs)
  - `adr_0022_log_shows_the_conversation`.
  - `adr_0022_log_merges_the_raw_acp_in_time`.
  - `adr_0022_log_reads_an_inactive_session`.
  - `adr_0022_log_follows_until_the_host_exits`.
  - `adr_0022_log_follows_the_raw_acp_too`.
  - `adr_0022_session_changed_says_what_changed`.
  - `adr_0022_closing_sessions_closes_their_files`.
- [src/log.rs](../../src/log.rs)
  - `adr_0022_a_closed_session_has_its_records_and_then_no_file`.
