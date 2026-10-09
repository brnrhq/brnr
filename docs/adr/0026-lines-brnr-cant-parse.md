# 26. Lines brnr can't parse

Accepted 2026-10-07. Implemented. The editor is told in the session that
its late answer was dropped, and who answered first (ADR 4).
Resolves review item 2.

Amended by 51: a line past 32 MiB isn't read either; with an editor it goes
on as it comes, headless it is dropped, and a `line_too_long` event says so.

Amended by 61: an editor's request the host reads goes with an id of the
host's, so an answer the host can't read reaches the editor with that id.

## Context

`agent_line` (src/host/acp.rs) forwarded a line serde_json rejected as it
was, without tracking it. If it was a request, the editor's answer was
dropped as unknown (`editor-response-dropped` in `editor_line`) and the turn
hung; an answer to one of the host's own requests reached the editor raw;
an editor prompt that didn't parse wasn't counted as a turn.

The review's two cases aren't broken JSON. A lone surrogate (`"\ud83d"`,
what `JSON.stringify` makes of a title cut in the middle of an emoji) is
allowed by RFC 8259's grammar, which only calls its meaning unpredictable;
nesting deeper than 128 is serde_json's limit, not JSON's.

## Decision

- The host reads every RFC 8259 text. In the copy it interprets, a lone
  surrogate becomes U+FFFD, and there is no depth limit (serde_json's
  `unbounded_depth`, with the stack grown as needed). The line itself is
  forwarded byte for byte, as always. This stays brnr's own work with the
  official schema types too (ADR 43): they parse with serde_json.
- For a line that isn't JSON at all: the host keeps the ids of agent
  requests it answered itself (permission requests cancelled, or answered
  from outside, ADR 4) and drops only the editor's late answers to those,
  telling the editor (ADR 4); any other unknown id goes to the agent, which
  ignores an answer to something it never asked.
- Lines aren't scanned for ids (P4), and nothing is withheld from the editor
  (P1). An editor ignores a response to a request it never sent. A host
  request whose answer can't be read goes unanswered: a start fails at its
  timeout (ADR 7), and for a bridge's request `brnr` stops waiting after
  120 s (ADR 29, which notes that the host itself never gives up).
- An editor prompt that isn't JSON stays uncounted; nothing hangs on it.

## Considered

- Pulling the `id` and `method` out of a broken line leniently (review item
  2, B): a guess that would have to be right about every way JSON can break.
- Also keeping agent lines that contain `"id":"brnr-` from the editor: a
  heuristic, which withholds the editor's bytes.
- Leaving serde_json's limits and only no longer dropping unknown ids: the
  lone surrogate, the case that happens, would still go untracked.

## Tests

Run `cargo test --release adr_0026_`. Named claims and their assertions:

- [tests/headless.rs](../../tests/headless.rs)
  - `adr_0026_a_lone_surrogate_is_read`.
  - `adr_0026_deep_nesting_is_read`.
  - `adr_0026_an_answer_the_host_cant_place_reaches_the_agent`.
  - `adr_0026_a_late_answer_to_a_cancelled_request_is_dropped`.
