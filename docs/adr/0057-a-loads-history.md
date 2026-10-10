# 57. A load's history is recorded once, and a `history` event says so

Accepted 2026-10-08 (proposed). Implemented. Amends 14 (what a load does
with the replay) and 22 (brnr never recording a replay). Resolves #60.

Amended by 63: `brnr start --resume` is `brnr session resume`;
`brnr sessions` is `brnr session list` with an agent named, where a session
only the agent knows is `inactive` with SOURCE `agent`; and `status` is
`session status`.

## Context

An agent without `session/resume` resumes a session with `session/load`,
replaying its history as `session/update`s before it answers. A headless
start skipped all of them, raw and interpreted, on the assumption that
brnr's transcript had the history already; only what they said the session
is now (title, mode, config options, commands) was kept. The assumption fails
for a session brnr has never seen (`brnr sessions` lists the agent's own,
and `--resume` takes any of them, ADR 14), and for one whose transcript is
gone: `brnr log` then had the load's request and answer, and none of the
conversation, and nothing said it was missing (P3).

## Decision

- `brnr start --resume` tells the process whether brnr has a transcript of
  the session: one `brnr sessions` lists as `inactive`, or one another
  process is serving (`--take-over`). That is the one thing brnr knows
  about the history it has; it doesn't compare the replay with the
  transcript to guess what is missing (P4).
- With a transcript, the replay is as before: neither recorded, raw or as
  events, nor shown, and what it says the session is now is kept. Loading
  the session again and again adds nothing to the transcript but each
  load's `history` event.
- Without one, the replay is taken in as live updates would be: recorded in
  the raw file, and made into events (`agent_message`, `tool_call`,
  `session_changed`, …), each with `"replayed": true`. The user's messages,
  which live are the prompts brnr sent, come only from the replay here, as
  `user_message` events with `replayed` and without `by` or a message id.
  Text shows them as `(replayed) user: …`. A tool call the history left
  unfinished isn't counted as running (`status`). The next load finds the
  transcript, and doesn't record the history again.
- Either way, once the agent answers the load, a `history` event says how
  many `updates` the agent replayed and whether they were `recorded`:
  `history: 3 updates replayed by the agent, recorded`, or `…, not
  recorded: brnr's transcript has the session`.
- `session/resume` replays nothing, and makes no `history` event. An
  editor's own `session/load` passes through as before.

## Consequences

- The transcript of a session first loaded by brnr starts with its history,
  as the agent replayed it. Those records are as new as the load, and carry
  this process's ids.
- brnr's transcript is complete only for what happened through brnr: turns
  taken in the agent's own client, or while a profile had `log = false`
  (ADR 33), or records a slow disk skipped (ADR 22), aren't in it, and a
  load of a session brnr has a transcript of doesn't add them. The
  `history` event's `recorded: false` says that the replay was left out, so
  that what is missing is known to be missing, not silently gone.
- Deleting a session's transcript makes its next load record the history
  again.

## Considered

- Always recording the replay, marked: every load of a session would add
  its whole history to the transcript again.
- Recording only what the transcript lacks, matching the replay against it:
  ACP's message ids are optional, and brnr's own records of a turn (an
  injected message's echo, a steer) aren't what an agent replays. A match
  would be a guess (P4).
- Recording the raw replay only, without events: `log` reads events, so the
  history still wouldn't be in it.

## Tests

Run `cargo test --release adr_0057_`. Named claims and their assertions:

- [tests/cli.rs](../../tests/cli.rs)
  - `adr_0057_a_first_load_records_the_replayed_history`.
  - `adr_0057_a_load_without_the_transcript_records_the_history`.
  - `adr_0057_a_load_of_an_empty_history_says_so`.

That a session brnr has a transcript of keeps the replay out of it however
often it loads, with a `history` event each time, is ADR 14's
`adr_0014_resume_by_loading_keeps_the_replay_out_of_the_transcript`.
