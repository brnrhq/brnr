# 24. `log --last <n>`

Accepted (former decision 20); reviewed 2026-10-07. Implemented.

## Decision

`--last <n>` counts messages sent to the agent (`user_message` events), and
shows from the n-th last one on. `--last 0` shows nothing.

## Considered

- Counting turns (`turn_ended`): it leaves out a turn still running, the one
  most likely wanted.
