# 19. What `cancel` does with held messages

Accepted (former decision 6); reviewed 2026-10-07. Implemented, except the
`message_dropped` events (ADR 20).

## Decision

`cancel` cancels the running turn (`session/cancel`, answering the agent's
pending permission requests `cancelled`, as ACP requires) and drops the
session's held messages, each with a `message_dropped` event (`by: cancel`)
and listed in its response. `--keep-held` keeps them, to go out as usual.

## Considered

- Cancelling the turn and letting held messages go out as usual: surprising,
  since "stop" would start the next queued message.
- Cancelling the turn and pausing the queue: a paused state to explain.
