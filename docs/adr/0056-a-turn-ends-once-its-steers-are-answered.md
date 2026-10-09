# 56. A turn ends once its steers are answered

Accepted 2026-10-08. Implemented. Amends 17 (which messages a turn carried)
and 18 (what `send --steer` does when the agent's answer comes late).
Resolves #63.

## Context

A steer (`send --steer`, ADR 18) is a request of its own,
`_session/steering`, answered apart from the `session/prompt` of the turn
it goes into. The two answers can come in either order. When the turn's
came first, the host emitted `turn_ended` with the messages it knew of, and
a later `injected` found no turn to put the message in: it went into the
host log as `steer-after-turn` and nowhere else. The message had been
accepted (`steered (message m2)`), but no `turn_ended` listed it and no
`message_dropped` said it went unsent, so `send --steer --wait` waited until
its timeout (124) on a session that was idle (P3).

## Decision

- A turn that ends while steers sent into it are unanswered has its
  `turn_ended` held back until the agent has answered them all, or they are
  dropped: until then which messages it carried isn't known (ADR 17).
- An `injected` answer puts the message in that turn: its `user_message`
  names the turn's prompt, and the `turn_ended`, when it goes, lists it. A
  `promptRequired` answer holds it as ADR 18 says, ahead of what is held;
  an error, or any other answer, is a `message_dropped`, `by` `steer`
  (ADR 20). Then the held `turn_ended` goes, and the next held message.
- `cancel`, closing the session and the agent exiting drop unanswered
  steers as before (`message_dropped`, `by` `cancel`, `close` or `exit`),
  and then the held `turn_ended` goes: every accepted message ends in one
  `turn_ended` that lists it or one `message_dropped`, whatever the order of
  the answers.
- `send --steer --wait` for a message answered `injected` after its turn's
  own answer exits as that turn does. What the agent said before it answered
  isn't shown as the message's reply (`--wait` prints what follows the
  message's `user_message`); the transcript has it.

## Consequences

- An agent that never answers a steer keeps the turn's `turn_ended`, and
  what is held behind it, waiting, as it already kept held messages
  waiting; `cancel` or `close` ends it.
- `steer-after-turn` in the host log is left for an `injected` answer with
  no turn at all, which a steer sent only while a turn runs or waits can't
  get; it is dropped then, `by` `steer`, never left without an end. The
  editor's own steers (ADR 4), which have no message id, keep it as before.

## Considered

- Emitting `turn_ended` at once, and a second event for the late steer: two
  endings for one turn, or a new event for waiters and bridges to learn,
  for what is one turn's list of messages.
- Dropping a late `injected` steer as `message_dropped`: it would say a
  message the agent took never went (P3 the other way).

## Tests

Run `cargo test --release adr_0056_`. Named claims and their assertions:

- [tests/headless.rs](../../tests/headless.rs)
  - `adr_0056_steer_answered_after_its_turn_is_in_that_turn`: an `injected`
    answer after the turn's own is in that turn's `turn_ended`, and
    `send --steer --wait` exits 0.
  - `adr_0056_steer_refused_after_its_turn_is_dropped`: an error answer
    after the turn's is a `message_dropped`, `by` `steer`, then the turn's
    `turn_ended` without it; `--wait` exits 1.
  - `adr_0056_steer_never_answered_is_dropped_by_close`: the `turn_ended`
    waits for the unanswered steer; closing the session drops it, `by`
    `close`, then the `turn_ended` goes; `--wait` exits 1.
