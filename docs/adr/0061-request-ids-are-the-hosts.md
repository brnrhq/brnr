# 61. Every request to the agent has an id of the host's

Proposed 2026-10-09. Implemented. Amends ADR 2 (the editor's requests reach
the agent with another id) and ADR 26 (an answer the host can't read).
Resolves GHSA-84pw-hh9c-w2m8.

## Context

The host sends the agent requests of its own: a headless start's, a
bridge's (`mode`, `model`, `config`, `fork`, `close`), a message sent as a
prompt and a steer. It gave them ids `brnr-<n>`, and passed the editor's
requests on with the editor's ids. The two shared one namespace, and the
editor may choose any id, `brnr-1` included: the report sent an editor
prompt with id `brnr-1` and then ran `brnr mode`, whose request had the
same id. The answer to the mode was taken for the answer to the prompt as
well: the editor's turn ended (`status` idle, a `turn_ended`) while the
agent was still running it, and the mode's answer was kept from the editor.
The other way round, the editor's request with the id of a prompt `send`
had injected ended brnr's turn, and the editor never got its answer. The
editor's `$/cancel_request` naming such an id went to the agent as it was,
and cancelled the host's request. A prefix the editor is asked not to use is
no namespace: the protocol lets it use any id.

The agent's answer carries only the id. To know whose it is, the id must
say so.

## Decision

- Every request to the agent has an id the host gives it, `brnr-<n>`, from
  one counter: the editor's too. Ids on the agent's side are the host's
  alone, so no two requests waiting for an answer have the same one,
  whatever ids the editor writes.
- The host keeps, for each request waiting for its answer, who sent it: the
  editor, the host itself (a start's or a bridge's request), or a message it
  sent as a prompt. The answer goes to that one only. The editor's goes back
  to it with the id as the editor wrote it, byte for byte: `7` and `"7"`
  are two requests, and `1e3` comes back as `1e3`.
- In a line, only the id changes: the host finds the request's top-level
  `id` members in the line (an escaped name, and one given twice, too) and
  writes its own id there, and the editor's back into the answer. The rest
  goes on byte for byte, as ADR 2 has it.
- The editor's `$/cancel_request` names one of its own requests by its id:
  it goes to the agent with that request's id there. One naming an id the
  host gave a request, which the editor was never told, doesn't go, and the
  host log says so (`editor-cancel-dropped`): it can't be the editor's to
  cancel. One naming neither goes as it came, for a request in a line the
  host couldn't read (ADR 26).
- What brnr records of a request and its answer is as the agent had them,
  with the host's id; the `prompt` of `user_message` and `turn_ended` is
  that id too.
- The agent's own requests to the editor keep the agent's ids: only the
  agent sends those, and the editor answers them.

## Consequences

- A request in a line the host doesn't read (one that isn't JSON, or past
  32 MiB, ADR 26 and 51, or what the editor wrote after its last newline)
  reaches the agent with the editor's id, untracked as before. Should that
  id be one the host has given a request still waiting, the answer is taken
  for that request's: the editor can only bring that about with an id it
  wasn't told, and with a line nobody can read.
- An answer the host can't read reaches the editor as the agent wrote it,
  with the host's id, so the editor can't place it, as a host request's
  answer couldn't be placed before (ADR 26).
- Only the agent's ids are on the editor's side, so an agent that reuses an
  id after the host answered a request with it (a cancel, an approve) can
  have the editor's late answer to the first taken for the second's. No
  agent brnr knows reuses ids; the editor's side isn't rewritten for it.

## Considered

- A prefix of the host's (`brnr-`), with the editor asked not to use it, or
  its ids that have it refused or rewritten: the editor may use any id, and
  any prefix is one it may choose.
- Rewriting only the editor's ids that look like the host's: whether two
  ids collide then turns on how each side reads them (`1` and `1.0`, an
  escaped string), which a separate namespace doesn't.
- Re-encoding the whole line with the new id: simpler, but every other byte
  of the editor's requests and the agent's answers would change too (number
  forms, escapes, key order and spacing), which ADR 2 promises they don't.
- Random host ids: no better than a prefix for the lines the host reads,
  and unneeded for them.

## Tests

Run `cargo test --release adr_0061_`. Named claims and their assertions:

- [tests/headless.rs](../../tests/headless.rs)
  - `adr_0061_an_editors_prompt_ends_only_with_its_own_answer`.
  - `adr_0061_an_editors_request_with_a_host_requests_id_is_the_editors`.
  - `adr_0061_ids_come_back_as_the_editor_wrote_them`.
  - `adr_0061_the_editors_cancel_request_is_for_its_own_request`.
- [src/json.rs](../../src/json.rs)
  - `adr_0061_an_id_is_found_where_it_is_and_nowhere_else`.
- [tests/security.rs](../../tests/security.rs)
  - `adr_0002_acp_passes_bytes_unchanged`, which checks that only the ids
    change.
