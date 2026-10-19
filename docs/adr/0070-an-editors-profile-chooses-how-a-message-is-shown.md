# 70. An editor's profile chooses how a message sent through brnr is shown to it

Draft 2026-10-15. Implemented. Amends 5 (a message or context is a tool
call unless the profile says otherwise) and 33 (the editor part has
`echo_as`).

## Context

ADR 5 shows the editor a message sent to its session through the side
channel (`brnr prompt send`, `--context`) as a completed tool call, "Message
via brnr" or "Context via brnr", because editors didn't render a
`user_message_chunk` that arrives out of turn. That holds for some editors
and not for others, and brnr can't tell which editor it serves (P4). For an
editor that renders the chunk, the tool call puts the user's own words in a
block that says a tool ran, where `user_message_chunk` is what ACP has for
them (P1, P2).

## Decision

- `echo_as` in `[profiles.<p>.editor]` chooses the form, by the
  `sessionUpdate` it is sent as:
  - `"tool_call"`, the default: as ADR 5 has it.
  - `"user_message_chunk"`: one `session/update` `user_message_chunk` per
    content block of the message, as the agent sends a user's message when
    it replays a session; no title, no tool call.
- Another value fails to load, naming the two (P7); `brnr doctor` shows the
  profile's when it isn't the default.
- It applies to messages and context only. brnr's notes to the editor
  ("Allowed via brnr", "Session closed via brnr" and the rest, ADR 4) stay
  tool calls: they are brnr's words, and a user chunk would make them the
  user's, as an agent chunk would make them the agent's (ADR 5).
- Only the editor sees the difference. Events, bridges and the transcript's
  story keep `user_message` with `by: control`.
- Both forms are stable ACP, so strict mode doesn't change it; there it is
  moot, as strict mode has no experimental actions (ADR 41).

## Consequences

Which editors render an out-of-turn `user_message_chunk` is checked in real
editors (#216), not here; until then the default is the form every editor
renders. A message shown as a user chunk carries no "via brnr": the editor
shows it as the user's, which it is.

## Considered

- A per-editor default, from the editor's `clientInfo`: brnr would guess
  what an editor renders (P4), and be wrong for the next version of it.
- `user_message_chunk` for brnr's notes too: they would read as the user's
  words.
- An experimental action, named in `experimental`: it doesn't act on the
  session; it chooses how something an action does is shown.

## Tests

Run `cargo test --release adr_0070_`. Named claims and their assertions:

- [tests/headless.rs](../../tests/headless.rs)
  - `adr_0070_messages_can_be_shown_as_user_message_chunks`.
