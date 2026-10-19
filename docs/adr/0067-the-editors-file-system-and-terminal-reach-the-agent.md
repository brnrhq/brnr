# 67. The editor's file system and terminal reach the agent in strict mode, or when its profile names them, and side-channel actions stay experimental in 1.x

Draft 2026-10-14, rewritten 2026-10-19 (#272). Implemented. The real-editor
checks below are not done yet: every cell of the matrix is untested.
Resolves the decision part of #69, and #272; the real-editor evidence is
still to be gathered.

## Context

ACP v1 has two client capabilities that v2 drops: `fs` (`readTextFile`,
`writeTextFile`) and `terminal`. With them, an agent reads the editor's
state of a file, unsaved changes included, has its writes applied by the
editor, and runs commands in the editor's terminals
(https://agentclientprotocol.com/protocol/v1/file-system). Without them, it
reads and writes the files on disk and runs commands in processes of its
own.

ADR 2 dropped both from the editor's `initialize` by default, as v2 does,
and passed them in strict mode. This record's first version (#69) passed
them in every mode, with no setting to drop them, by P1's test: an agent
without them is affected now, in v1, the version brnr speaks (ADR 54).

ADR 1's critical success factors settle it differently (#272). F2 is the
conventions brnr follows ahead of the spec, by default; F1 is stable ACP to
the letter, in strict mode. They meet by mode, not by rank. Agents that
rely on `fs` and `terminal` now rely on what v2 takes away, and brnr
discourages that, while strict mode stays faithful to v1.

## Decision

- By default, `brnr acp` removes `fs` and `terminal` from the editor's
  `initialize`, as ACP v2 does (F2). The host log notes
  `capabilities-dropped`, with what it dropped. The agent answers as to a
  client that has neither.
- A profile passes them by name, in its editor part (ADR 33):
  `client_capabilities = ["fs", "terminal"]`, or one of them. Each passes as
  the editor sent it. An unknown name fails to load (P7).
- In strict mode both pass, whatever the profile says, as stable ACP v1 has
  them (F1, ADR 41).
- When the agent was offered them and sends `fs/*` or `terminal/*`, the
  requests go to the editor as the agent wrote them, and the editor's
  answers, results or errors, go back to the agent. It is the same path
  every request of the agent's takes to the editor (ADR 2); only the
  editor's own requests get the host's ids (ADR 61).
- When ACP v2 is stable, brnr follows it, and this record is rewritten from
  what editors and agents speaking v2 do.
- A headless process offers the agent neither: its `initialize` has empty
  `clientCapabilities`, as no editor is there to serve them. That covers a
  session started headless, one forked in a headless process, and one taken
  over from an editor (`session resume --take-over`), which goes on in a new
  headless process with an agent of its own. A request for them there is
  answered "method not found". No headless session runs in an editor's
  process, where the agent may have been offered them: `session fork`,
  `session new --pid` and `session resume --pid` are refused there (ADR 4).
- What transparent means for the editor's process, now:
  - Observing (`event watch`, `event log`, `session status`,
    `event notify`, bridges) never changes the stream.
  - The named changes to the stream are: requests reach the agent with the
    host's ids (ADR 61); the editor's `fs` and `terminal` capabilities are
    dropped unless named or in strict mode (above); an editor's load of a
    session another process holds is refused unless a feature flag allows
    it (ADR 3, ADR 42); and the experimental actions a profile enables
    (ADR 4).
  - Headless operation is brnr as an ordinary ACP client (P1, P12).
- Every action of ADR 4's table on an editor's session (`send`, `context`,
  `cancel`, `permission`, `config`, `close`) is experimental for all of
  1.x, opt-in by name as ADR 4 has it. None becomes a default, or stops
  needing its opt-in, by a release alone.
- An action leaves experimental only by an ADR of its own, which brings
  evidence from real editors: the checks below for that action, run in each
  editor brnr supports, with each editor's and adapter's version, what was
  seen, and the cells left untested, said as such. The synthetic suite
  (`tests/fake_agent.py`) passing isn't that evidence: it shows what brnr
  sends, not what an editor makes of it.

## Consequences

- By default, an agent run through `brnr acp` doesn't see the editor's
  unsaved buffers, its writes aren't applied by the editor, and it runs
  commands in processes of its own, where run directly it might do all
  three through the editor. That is the cost of following v2 ahead of it;
  a profile's `client_capabilities`, or strict mode, gives them back. The
  README says so.
- With them passed, the editor serves those requests
  (docs/threat-model.md, B3). What it answers them with (files as its
  buffers have them, unsaved changes and a `.env` included, and the
  commands' output) is in the session's raw ACP as it passed, not
  redacted. That is accepted: the threat model lists it as a gap brnr
  takes, not one to close.
- A session taken over from an editor, or resumed headless after it, goes
  on without them: the agent reads the files on disk, and what was unsaved
  in the editor isn't there.
- When the editor goes, its agent goes with it (ADR 2), so an agent never
  waits on an `fs` or `terminal` request no editor will answer. A request
  that comes while the process is ending gets "method not found" from the
  host.
- An `initialize` longer than 32 MiB goes to the agent as it is, with them
  (ADR 51). Editors don't send one that long.

## Real-editor checks

To be run by hand: an editor can't be faked. Each cell is an editor
(Zed; a JetBrains IDE through its ACP support, IntelliJ's AIR plugin;
others as they come) with an adapter (`brnr-claude-adapter`,
`brnr-codex-adapter`), and records the editor's, the adapter's and brnr's
versions, the OS, and pass, fail or untested for each check. The raw ACP
(`brnr event log <session> --events acp`) shows what crossed. Every check
is also run with the agent configured directly, without brnr, as the
baseline: a difference from it is the finding, not the behaviour alone.

File system and terminal (this record), with `brnr acp` and a profile's
`client_capabilities = ["fs", "terminal"]`, and with `brnr acp --strict`;
and, for the default, with `brnr acp` and no such setting:

0. The default: the agent sends neither `fs/*` nor `terminal/*`, reads and
   writes the files on disk itself, and runs commands in processes of its
   own; the host log has `capabilities-dropped`.
1. Unsaved buffers: edit a file in the editor without saving, and ask the
   agent what the edited lines say. It answers with the unsaved text, and
   the raw ACP has `fs/read_text_file` and the editor's answer.
2. Writes: ask the agent to edit an open file. The edit arrives through
   `fs/write_text_file`, the editor shows it in the buffer (or its diff
   view), and it is on disk as the editor saves.
3. Terminal: ask the agent to run a command. The editor shows its terminal
   (`terminal/create`), the agent gets its output and exit status
   (`terminal/output`, `terminal/wait_for_exit`), and `terminal/release`
   follows. Cancelling the turn while the command runs kills it
   (`terminal/kill`, or release).
4. Take-over: with `close` enabled, `brnr session resume <session>
   --take-over`; the session goes on headless, reading the files on disk,
   and the editor is told (ADR 4).

The editor's path, from #69:

5. Prompts and cancel: a prompt, a long turn cancelled from the editor,
   and a prompt after it.
6. Approvals: a permission request shown and answered in the editor, both
   allow and reject.
7. Settings: the editor's mode and model selectors change the agent's, and
   the agent's changes move them.
8. Disconnect: closing the editor's thread, window or the editor itself
   ends the agent and its children (ADR 2, ADR 11); restarting the agent
   from the editor leaves nothing behind.
9. Resume: after the editor closes, `brnr session resume <session>` goes on
   headless; the editor loads the session again once that process has
   closed it (ADR 3).

Experimental actions: the list in ADR 4's Consequences, per action, with
the action enabled. These are the cells an ADR promoting an action cites.

## Considered

- Passing them in every mode, as this record's first version had it:
  faithful to v1 (F1), but agents then build on what v2 drops, and
  following the conventions ahead of the spec is what the default is for
  (F2). Strict mode keeps the v1 behaviour for whoever wants it.
- Dropping them in strict mode too: strict mode is stable ACP to the
  letter, and v1 has them.
- Dropping them by default with no setting, as ADR 2 had it: strict mode
  would be the only way to get them, and it also turns off steering and
  fork (ADR 41).
- Dropping `terminal` only, keeping `fs`: both are stable v1 client
  capabilities that v2 drops, and the argument is the same for each; the
  profile names them one by one.
- Redacting file and terminal data in transcripts: redaction is by field
  (ADR 25), and a file's or a command's contents have no field a secret is
  known to be in, so it would be a guess at what is secret (P4), or the
  data dropped from the record of what passed (ADR 22). The transcripts
  are only the user's to read (P13).
- Serving `fs` and `terminal` from the host for headless sessions: brnr
  would read, write and run commands for the agent, a boundary of its own
  (docs/threat-model.md), where the agent does all three itself when not
  offered them.
- Promoting an experimental action in 1.x now: there is no real-editor
  evidence yet for any of them.

## Tests

Run `cargo test --release adr_0067_`. Named claims and their assertions:

- [tests/headless.rs](../../tests/headless.rs)
  - `adr_0067_fs_and_terminal_reach_the_agent_in_strict_mode_or_when_named`.
  - `adr_0067_the_agents_file_and_terminal_requests_reach_the_editor`.
  - `adr_0067_headless_sessions_offer_neither`.

The real-editor checks above have no automated test: they are what the
matrix records.
