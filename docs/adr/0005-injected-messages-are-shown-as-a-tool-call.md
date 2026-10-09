# 5. Injected messages are shown to the editor as a tool call

Accepted (former decision 48); reviewed 2026-10-07. Implemented, and the
form of ADR 4's notes to the editor: an approval answered elsewhere ("Approved
via brnr", "Denied via brnr"), the editor's late answer to one, a session
closed or taken over, a close that failed. Applies where ADR 4's actions are
enabled.

## Context

The editor was shown an injected message (`brnr send`, attached context) as a
synthesized `user_message_chunk`. Editors don't render one that arrives out
of turn: in ACP's model user messages are what the client sends, so an
unsolicited chunk, mid-turn or between turns, has no place in their
transcript and is dropped or misplaced.

## Decision

One `session/update` with `sessionUpdate: tool_call`,
`toolCallId: brnr-echo-<n>`, kind `other`, status `completed`, titled
`Message via brnr` (or `Context via brnr`), the message's content blocks as
its content: the one update editors render as a block of its own at any point
in a turn. Only the editor sees it: events, bridges and the transcript's own
story keep `user_message` with `by: control`. It is sent when the message is
sent; when the agent takes it up is the agent's business. The same form is
the candidate for ADR 4's other notes to the editor (an approval answered
elsewhere, a session taken over).

## Consequences

How each editor shows it is part of ADR 4's trials.

## Considered

- `user_message_chunk`: honest about what it is, but not rendered.
- `agent_message_chunk` or `agent_thought_chunk`: rendered, but the text
  appears as the agent's own words.

## Tests

Run `cargo test --release adr_0005_`. Named claims and their assertions:

- [tests/headless.rs](../../tests/headless.rs)
  - `adr_0005_send_to_an_editors_session_waits_for_no_turn`.
  - `adr_0005_context_joins_the_editors_next_prompt`.
