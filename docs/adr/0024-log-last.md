# 24. `log --last <n>`

Accepted (former decision 20); reviewed 2026-10-07. Implemented.

## Decision

`--last <n>` counts messages sent to the agent (`user_message` events), and
shows from the n-th last one on. `--last 0` shows nothing (with
`--follow`, only what comes next).

## Considered

- Counting turns (`turn_ended`): it leaves out a turn still running, the one
  most likely wanted.

## Tests

Run `cargo test --release adr_0024_`. Named claims and their assertions:

- [tests/cli.rs](../../tests/cli.rs)
  - `adr_0024_last_counts_user_messages_including_an_unfinished_turn`.
