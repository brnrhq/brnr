# 29. Requests the agent answers

Accepted (former decision 22); reviewed 2026-10-07. Implemented (with
`set_model` still among them, until ADR 28).

## Decision

Setting the mode or a config option, forking and closing are answered when
the agent answers, on the same connection (matched by `req_id`); the host
doesn't block meanwhile. `brnr` waits up to 120 seconds for them, since
switching model or forking can take a while.

## Consequences

The host itself never gives up on these: a bridge whose request the agent
never answers (or answers in a line brnr can't read, ADR 26) waits as long
as it keeps waiting, and so does the `session/close` of a `--take-over`
(ADR 3) once `brnr` has stopped waiting. Whether the host should answer such
requests with an error after a time of its own (P3) is still open.
