# 17. Message ids and turns

Accepted (former decision 3); reviewed 2026-10-07. Implemented, with
steering (ADR 18). Amended by 56: a turn's `turn_ended` waits for the
answers to the steers into it.

## Context

`send --wait` must know which turn answers its message, even when the
message is held, steered into a running turn, or sent after an interrupt.

## Decision

Every message the host accepts gets an id (`m<n>`; not `send --context`,
which has no turn), which is in the `send` response (and `start --json`'s,
for the start's prompt), in `user_message` when the message is actually
sent, and in the `turn_ended` of the turn that carried it. A turn can carry
more than one: a steered message has no turn of its own (ADR 18). So
`turn_ended` lists them, `messages: [m1, m3]`, in place of its single
`message` (P9).

## Considered

- Matching on the message's text: fragile.

## Tests

Run `cargo test --release adr_0017_`. Named claims and their assertions:

- [tests/headless.rs](../../tests/headless.rs)
  - `adr_0017_steer_goes_into_the_running_turn`.
- [tests/cli.rs](../../tests/cli.rs)
  - `adr_0017_start_json_gives_the_prompts_message`.
