# 22. The host's events are the story

Accepted (former decisions 1, 34 and 35); reviewed 2026-10-07. Implemented.

## Context

The transcript records raw ACP. A readable history needs the interpretation
the host already does (assembling agent messages, pairing permission
requests with their answers, naming a turn's end), and the CLI can't know the
host's names for things (permission handles like `p1`, message ids).

## Decision

- Every event the host emits to bridges (`user_message`, `agent_message`,
  `tool_call`, `permission_request`, `turn_ended`, …) is also a record in the
  session's file. `log`, `watch` and the foreground render the same events,
  and a transcript read back is the stream a bridge saw live (P5).
- `log` reads a record as the event `watch` would have shown: an ACP message
  becomes an `acp` event. `--events` and `--json` then mean the same for
  both, and `log --json` is what a watcher got live. The stored records are
  still in the file (`brnr list --json` names it); `log` doesn't print them
  as stored.
- `exited`, the one event that belongs to the process, is written into every
  session's file too, so `log` keeps to one file. The host log keeps what
  belongs to no session (ACP before a session exists, the agent's stderr).
- Transcripts, like the agents' own, are keyed by project folder:
  `~/.brnr/projects/<folder>/<session id>.jsonl`, one per session, where
  `<folder>` is the cwd with every non-alphanumeric character replaced by `-`
  (as in `~/.claude/projects`), and `~/.brnr/hosts/<run id>.jsonl` for each
  process. Every record carries `host_id`, `host_pid`, `proxy_pid` and
  `agent_pid` for joining. They are readable only by the user (P13).

## Consequences

- Agent text is stored twice: the chunks in the raw ACP, and the assembled
  message.
- Transcripts written before events existed have none; `log` says so.

## Considered

- Rendering raw ACP in the CLI: it works for any transcript, but duplicates
  the host's interpretation in a second place, without the host's names.
- `log` also reading the host log for the session's time span: two files to
  merge and to follow, and the host log has ACP and stderr that aren't the
  session's.
- For `log` (former decision 34), keeping the record dump and adding
  `--events` over the records: `--raw` would have kept two meanings, one for
  `log` and another for `watch`.
- Host-wide events copied into every session's file (former decision 35):
  the case that prompted it, `owner_changed`, went with ADR 2; `exited` is
  what remains.
