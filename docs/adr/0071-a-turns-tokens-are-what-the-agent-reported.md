# 71. A turn's tokens are what the agent reported

Draft 2026-10-16. Implemented. Amends 21 (`--wait --json` has
`tokens`), 23 (`turn_ended` has `tokens`, and its line shows them), 34
(`status` has `last_turn`) and 43 (a prompt answer's unstable `usage` is read
as a `Value`). Resolves #228.

## Context

An agent can say in its answer to `session/prompt` how many tokens the turn
used. ACP's schema 1.10.2 has this as `PromptResponse.usage`, only behind
its unstable feature `unstable_end_turn_token_usage`: `totalTokens`,
`inputTokens` and `outputTokens`, required; `thoughtTokens`,
`cachedReadTokens` and `cachedWriteTokens`, optional; and `_meta`. brnr
builds the schema crate without its unstable features (ADR 43), so the
counts are in the raw ACP (ADR 22) and nowhere else.

brnr already has a `usage` event and a `usage` field in the status (ADR
23). They are another thing: what `usage_update` says, the context window
(`used` of `size`) and the cost. They are not a turn's count.

What a count covers is the agent's to say. Research of October 2026
(research#7) found that the Codex adapter reports its last model call, and
that Claude's ACP counts leave out subagents. Neither is in the protocol.

The current sequence, traced from the code at 39d304e (src/host/acp.rs,
`agent_response`):

1. The agent answers a `session/prompt`. The host reads its copy of the
   line.
2. The host records the line in the raw ACP, as it came (`record`).
3. If the editor sent the prompt, the host sends the line to the editor,
   with the editor's id put back and nothing else changed (ADR 61).
4. The host sets the session's `last_turn` to `{stop_reason, error}` and
   makes `turn_ended` with `stop_reason` and `error`. `result.usage` is not
   read.
5. `turn_ended` goes to the peers, or waits for the steers into the turn
   (ADR 56).

## Decision

Principles: what brnr shows is what the agent said, read once (P5), and
nothing is worked out or guessed (P4); ACP is followed as it is (P1, P2);
what brnr keeps from an event, it says (P3).

- The host reads `result.usage` of a `session/prompt` answer as a
  `serde_json::Value`, as ADR 43 already does for the unstable `fork`
  capability. The crate's unstable feature stays off.
- `turn_ended` and the session's `last_turn` get a field, `tokens`, after
  `error`:
  - `null` when the answer has no `usage`, or its `usage` isn't an object:
    the tokens are unknown.
  - Otherwise `{"total", "input", "output", "thought", "cached_read",
    "cached_write"}`, from `totalTokens`, `inputTokens`, `outputTokens`,
    `thoughtTokens`, `cachedReadTokens` and `cachedWriteTokens`. Each is
    the agent's number when it is a whole number that isn't negative (a
    `u64`), and `null` otherwise: left out, null, a string, negative, a
    fraction or written with an exponent.
- No count is calculated. There is no total from the parts, nothing from
  `usage_update`'s `used` or `size`, and no sum over turns.
- brnr says nothing of what the counts cover. They are the counts the agent
  reported for the turn, as the agent counts them: brnr doesn't say whether
  subagents or earlier model calls are in them.
- The answer goes to the editor as it came, `usage` included (P1). Only
  brnr's copy is read.
- `session status` has `last_turn` too, in `--json` and in text (it didn't
  before). `prompt send --wait --json` and `session new --wait --json` have
  `tokens` beside `stop_reason` and `error`, from the turn's `turn_ended`.
- Text (P5): `turn_ended` is `turn ended: end_turn (control); tokens: 1.2k
  total, 1.0k input, 234 output, 50 thought, 500 cached read, 20 cached
  write`, and the status has `last turn ended: end_turn; tokens: …`. Each
  count the agent gave is shown, and one it didn't give is left out, as
  `usage`'s line leaves out a cost it doesn't have (ADR 23). With none
  given, the line ends after `(control)`. The numbers are shown as the
  `usage` line shows them (`950`, `12.3k`, `1.2M`); `--json` has them
  exact.
- Strict mode doesn't change it. To read what the agent said is to observe,
  which affects no party (P1, ADR 41). An agent that keeps to stable ACP
  sends no `usage`, and its turns have `tokens: null`.

## Consequences

- Every `turn_ended` and `last_turn` has one field more, `null` for agents
  that don't send `usage`.
- If ACP changes the field (a name, a type, where it is) or stabilizes it,
  brnr sees the changed form as unknown counts or no `usage` until it is
  read again here; it doesn't fail. A stabilized `usage` can then be read
  with the schema's type.
- `tokens` is one of the names #150 and #151 settle, as ADR 69's are.

## Considered

- Enabling `unstable_end_turn_token_usage`: against ADR 43. The feature can
  change or go before it is stable, and with it the crate's types would no
  longer be what strict mode means by stable ACP (ADR 41). A `Value` read
  shows the counts and leaves the stable types as they are.
- Calling the field `usage`: it would clash with the `usage` event and the
  status's `usage`, which are the context window and the cost (ADR 23).
- Taking a turn's tokens from `usage_update`, or a total from its `used`:
  that is how full the context window is, not what a turn used.
- Passing the agent's `usage` object on as it is, camelCase: brnr's events
  are snake_case, and a copy would pass on any value, of any type. Each
  count read on its own is the agent's number or unknown.
- A total from `input` and `output` where the agent gives no `total`: a
  guess (P4). What a total includes is the agent's to say.

## Tests

Run `cargo test --release adr_0071_`. Named claims and their assertions:

- [tests/cli.rs](../../tests/cli.rs)
  - `adr_0071_a_turns_tokens_are_what_the_agent_reported`.
  - `adr_0071_a_count_the_agent_didnt_give_is_null`.
  - `adr_0071_a_turn_without_usage_has_its_tokens_unknown`.
- [tests/security.rs](../../tests/security.rs)
  - `adr_0071_the_editor_gets_the_answers_usage_as_the_agent_sent_it`.
- [src/render.rs](../../src/render.rs)
  - `adr_0071_text_shows_the_counts_the_agent_gave`.
