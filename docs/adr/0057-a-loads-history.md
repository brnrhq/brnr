# 57. A load's history is recorded once, and a `history` event says so

Accepted 2026-10-09. Implemented. Amends 14 (what a load does
with the replay) and 22 (brnr never recording a replay). Resolves #60.
Amended 2026-10-12 (#118): an editor's `session/load` through `brnr acp`
follows the same rule, where it passed through as before; its loads that
overlap share one replay and one `history` event, which says how many
`loads` it covers; and a load the agent answers with success after the
session's close is answered opens it again, with a `session_reopened`.
Amended 2026-10-14 (#118): a load's replay is counted across its session's
close, and its `history` follows the `session_reopened`; `recorded_updates`
says how many of the `updates` the transcript has.
Amended 2026-10-16 (#190): a bridge's load (`session resume --pid`) looks
for the transcript in any folder too, and a session the process has served
has its transcript there, written or not.
Amended 2026-10-16 (#191): an events file holding only `session-opened`
records, which a load that failed leaves, isn't a transcript of the
session, so the next load records the history; and a process counts a
session a load opened as served there only once the load succeeded.

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
- A transcript is an events file holding a record of the session beyond
  its openings. One holding only `session-opened` records, left by a load
  that failed, isn't one: the next load, a start's, a bridge's or an
  editor's, records the history. brnr reads its own file for this when the
  load is asked for, and deletes nothing; `brnr sessions` still lists the
  file, which names the session's cwd and agent.
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
  recorded: brnr's transcript has the session`. Its `loads` is 1, but for
  an editor's loads that overlap (below). `recorded` says whether the
  replay was taken into the transcript at all, as brnr had no transcript of
  the session; `recorded_updates`, always there, how many of the `updates`
  are in it: all of them when `recorded`, none when not, but for a session
  closed while its load was unanswered (below), where text says
  `history: 4 updates replayed by the agent, 3 recorded`.
- `session/resume` replays nothing, and makes no `history` event.
- An editor's `session/load` through `brnr acp` follows the same rule: its
  replay is recorded only when brnr has no transcript of the session, in
  any folder, and a `history` event says either way. A session open in the
  editor's process has its transcript, the process's, whether or not the
  disk has it yet (unless it logs nothing, `log = false`), so loading it
  again there records nothing of the replay again. The editor gets the
  replay as the agent sent it (ADR 2): only what brnr records and emits
  follows the rule.
- A bridge's load (`session resume --pid`, ADR 63) follows it too: in any
  folder, whatever the cwd it is resumed in.
- A session a process has served, and closed since, has its transcript in
  that process, for an editor's load and a bridge's alike, whether or not
  the logger, which writes behind (ADR 48), has written it yet: the process
  remembers each session it has started a transcript of. One a load
  opened, it remembers only once the load succeeded: a load that failed
  before the agent replayed left only the opening, and the next load, in
  that process too, records the history.
- The editor's loads of a session that overlap (one sent before the agent
  has answered the one before) all reach the agent, and share one replay,
  as which load an update is for can't be told (P4). Until the last of them
  is answered, every update either replays is taken as the replay is:
  recorded, marked `replayed`, or not, as the first load found the
  transcript. Then one `history` event says how many `updates` they
  replayed between them, whether they were `recorded`, and how many `loads`
  it covers: `history: 10 updates replayed by the agent in 2 loads,
  recorded`. A recorded replay can so hold the history once per load, each
  copy marked; nothing is left out (P3). A load that fails counts as
  answered; if none of them succeeds, there is no `history` event, as for a
  single load that fails, and a session they opened closes again, unless a
  `session/resume` of it succeeded meanwhile.
- The editor's close of a session (`session/close`) while its load is
  unanswered closes it once the agent answers the close, with its
  `session_closed`, as any close does. If the agent then answers the load
  with success, its last answer wins: the session is open in the editor's
  process again, and a `session_reopened`, `by` `editor`, says so (`session
  reopened (editor): its load was answered after its close`). brnr takes
  the agent's last answer as how the session is, and doesn't guess
  otherwise (P4). The load's replay is kept through the close: what the
  agent replays meanwhile is counted, and goes to the host log, as the
  session's files are closed; once the load succeeds, the replay goes on in
  the reopened session, and its `history`, after the `session_reopened`,
  has all the `updates` and, in `recorded_updates`, those the transcript
  has. A load that fails after the close leaves the session closed, with
  no `history`, as any failed load; of loads that overlap, one that
  succeeded before the close has its `history` once the last is answered,
  with the session still closed, in the host log. A load of the session
  sent after the close, while one before it is unanswered, joins that
  replay, as overlapping loads do.

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
  again, except in a process that served the session before the delete:
  that process remembers the transcript it started until it exits. A load
  that failed before the agent replayed anything leaves only its opening,
  no transcript, so the next load records the history too.

## Considered

- Always recording the replay, marked: every load of a session would add
  its whole history to the transcript again.
- Recording only what the transcript lacks, matching the replay against it:
  ACP's message ids are optional, and brnr's own records of a turn (an
  injected message's echo, a steer) aren't what an agent replays. A match
  would be a guess (P4).
- Recording the raw replay only, without events: `log` reads events, so the
  history still wouldn't be in it.
- Not creating the events file until the load succeeds (#191): the agent
  replays before it answers, so the load's raw ACP and its replay would be
  held in memory until then (P6), or written elsewhere and moved; and the
  files failed loads have left already would still keep their history out.
- Removing, or marking, the file a failed load leaves: removing it deletes
  the raw record of the failed load (P5), and either way the files already
  left stay as they are.

## Tests

Run `cargo test --release adr_0057_`. Named claims and their assertions:

- [tests/cli.rs](../../tests/cli.rs)
  - `adr_0057_a_first_load_records_the_replayed_history`.
  - `adr_0057_a_load_without_the_transcript_records_the_history`.
  - `adr_0057_a_load_of_an_empty_history_says_so`.
  - `adr_0057_a_bridges_load_finds_the_transcript_in_another_folder`.
  - `adr_0057_a_load_after_a_failed_load_records_the_history`.
  - `adr_0057_a_bridges_load_after_a_failed_load_records_the_history`.
- [tests/headless.rs](../../tests/headless.rs)
  - `adr_0057_an_editors_load_records_the_history_only_without_a_transcript`.
  - `adr_0057_an_editors_load_of_a_session_open_here_records_nothing_again`.
  - `adr_0057_overlapping_loads_in_an_editor_have_one_history`.
  - `adr_0057_a_failed_load_leaves_a_resumed_session_open`.
  - `adr_0057_a_load_answered_after_its_close_reopens_the_session`.
  - `adr_0057_a_loads_history_is_counted_across_its_sessions_close`.
  - `adr_0057_a_load_that_fails_after_its_sessions_close_has_no_history`.
  - `adr_0057_overlapping_loads_closed_meanwhile_have_one_history`.
  - `adr_0057_a_bridges_load_of_a_session_served_here_records_nothing_again`.
  - `adr_0057_an_editors_load_of_a_session_served_here_records_nothing_again`.
  - `adr_0057_an_editors_load_after_a_failed_load_records_the_history`.
- [src/log.rs](../../src/log.rs)
  - `adr_0057_only_openings_are_no_transcript`.

That a session brnr has a transcript of keeps the replay out of it however
often it loads, with a `history` event each time, is ADR 14's
`adr_0014_resume_by_loading_keeps_the_replay_out_of_the_transcript`.
