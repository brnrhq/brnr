# 34. Same data in text and JSON; `--json` wherever a command prints data

Accepted (former decisions 8, 42 and 44, and the usage part of 36); reviewed
2026-10-07. Implemented.

## Decision

- A command's text and its `--json` carry the same data (P5). `list` is an
  index, one row per session (ADR 15). `status <session>` is one session's
  detail, as a flat object: owner, title, mode, model, whether a turn is
  running and for how long, running tools, plan progress, held messages,
  pending approvals, token usage, the last agent message (at most 4000
  characters of it; the events have all of it), and what the agent says it
  is (`agentInfo`). `ps` follows the same rule. The control socket's
  `status` stays the full report (bridges use it); brnr shapes what it
  prints from it.
- Every command that prints data takes `--json`: one JSON value, or one event
  per line for `log`, `watch` and `start --foreground`. Errors stay on stderr,
  with the same exit status. Commands whose answer is their exit status
  (`stop`, `close`, `notify`) don't take it.
- The usage starts with the commands that start a process (`acp`, `start`),
  then groups the rest by what they are about: processes (`ps`, `stop`),
  chat (`send`, `wait`, `cancel`, `queue`), approvals (`pending`, `show`,
  `approve`, `deny`), sessions (`list`, `status`, `sessions`, `fork`,
  `close`), events (`log`, `watch`, `notify`), settings (`mode`, `model`,
  `config`, `commands`) and brnr (`doctor`, `--version`). It says once, at
  the end, what a `<session>` and a `<pid>` are, and that `--json` is
  everywhere. A usage error prints that command's lines, not all of it.

## Considered

- `list --json` as every process's whole status report, while the text
  showed six columns.
- `status` as a summary in text and the raw report in `--json` (former
  decision 8).
- `--json` on a few commands only (`list`, `status`, `watch`, `log`, as at
  first).

## Tests

Run `cargo test --release adr_0034_`. Named claims and their assertions:

- [tests/cli.rs](../../tests/cli.rs)
  - `adr_0034_status_summarizes_the_session`.
