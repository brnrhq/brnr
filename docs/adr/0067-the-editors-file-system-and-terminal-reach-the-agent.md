# 67. The editor's file system and terminal reach the agent, and side-channel actions stay experimental in 1.x

Draft 2026-10-14. Implemented. The real-editor checks below are not done
yet: every cell of the matrix is untested. Amends 1 (P1's conventions and
named changes no longer include dropping `fs` and `terminal`), 2 (the
editor's `initialize` keeps them), 4 (its actions stay experimental for all
of 1.x, and leave it only by an ADR of their own), 41 (dropping them isn't
one of its conventions, so strict mode and the default are the same here)
and 51 (no `initialize` is stripped of them, long or not). Resolves the
decision part of #69; its real-editor evidence is still to be gathered.

## Context

ADR 2 dropped the `fs` and `terminal` client capabilities from the editor's
`initialize` by default, because ACP v2's `ClientCapabilities` has neither.
Traced from the code before this record (src/host/acp.rs):

1. The editor sends `initialize` with `clientCapabilities.fs`
   (`readTextFile`, `writeTextFile`) and `clientCapabilities.terminal`.
2. `editor_request` sees `initialize`, and `drop_capabilities` removes `fs`
   and `terminal` (not in strict mode), noting `capabilities-dropped` in the
   host log.
3. The agent gets `initialize` without them, and answers as to a client
   that has neither.
4. The agent reads and writes files on disk itself, and runs commands in
   processes of its own: it never sends `fs/*` or `terminal/*`.
5. So the agent doesn't see what the editor has in its buffers and hasn't
   saved, the editor doesn't apply the agent's writes, and the editor's
   terminals aren't used. Run directly, the same agent in the same editor
   would do all three.

By P1's own test (can a party that follows the protocol be affected?) the
agent is affected now, in v1, which is the version brnr speaks (ADR 54) and
the one editors and agents implement. ACP v1 documents the file system
methods for exactly this: reading the editor's state of a file, unsaved
changes included (https://agentclientprotocol.com/protocol/v1/file-system).
That v2 drops them says where the protocol is going, not that an agent
without them works the same today. #69 asked for the default to be decided
on that basis, and for the side-channel actions, experimental since ADR 4,
to have a rule for leaving experimental that isn't "the synthetic suite
passes".

## Decision

- The editor's `initialize` reaches the agent as the editor sent it,
  client capabilities included, in strict mode and without. When the agent
  sends `fs/*` or `terminal/*`, the requests go to the editor as the agent
  wrote them, and the editor's answers, results or errors, go back to the
  agent. It is the same path every request of the agent's takes to the
  editor (ADR 2); only the editor's own requests get the host's ids
  (ADR 61).
- There is no setting to drop them again (P9). The question comes back when
  ACP v2 lands, and is decided then by a new record, from what editors and
  agents speaking v2 do.
- A headless process offers the agent neither: its `initialize` has empty
  `clientCapabilities`, as no editor is there to serve them. That covers a
  session started headless, one forked in a headless process, and one taken
  over from an editor (`session resume --take-over`), which goes on in a new
  headless process with an agent of its own. A request for them there is
  answered "method not found". No headless session runs in an editor's
  process, where the agent was offered them: `session fork`,
  `session new --pid` and `session resume --pid` are refused there (ADR 4).
- What transparent means for the editor's process, now:
  - Observing (`event watch`, `event log`, `session status`,
    `event notify`, bridges) never changes the stream.
  - The named changes to the stream are: requests reach the agent with the
    host's ids (ADR 61); an editor's load of a session another process
    holds is refused unless a feature flag allows it (ADR 3, ADR 42); and
    the experimental actions a profile enables (ADR 4).
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

- An agent run through `brnr acp` sees the editor's unsaved buffers, has
  its writes applied by the editor, and runs commands in the editor's
  terminals, as it would run directly. The editor serves those requests
  (docs/threat-model.md, B3).
- A session taken over from an editor, or resumed headless after it, goes
  on without them: the agent reads the files on disk, and what was unsaved
  in the editor isn't there. It was so before; the difference is that the
  editor's session now had them.
- When the editor goes, its agent goes with it (ADR 2), so an agent never
  waits on an `fs` or `terminal` request no editor will answer. A request
  that comes while the process is ending gets "method not found" from the
  host.
- The host log has no `capabilities-dropped` record any more.
- What the editor answers the agent's `fs` and `terminal` requests with
  (files as its buffers have them, unsaved changes and a `.env` included,
  and the commands' output) is in the session's raw ACP as it passed, not
  redacted. That is accepted: the threat model lists it as a gap brnr
  takes, not one to close.

## Real-editor checks

To be run by hand: an editor can't be faked. Each cell is an editor
(Zed; a JetBrains IDE through its ACP support, IntelliJ's AIR plugin;
others as they come) with an adapter (`brnr-claude-adapter`,
`brnr-codex-adapter`), and records the editor's, the adapter's and brnr's
versions, the OS, and pass, fail or untested for each check. The raw ACP
(`brnr event log <session> --events acp`) shows what crossed. Every check
is also run with the agent configured directly, without brnr, as the
baseline: a difference from it is the finding, not the behaviour alone.

File system and terminal (this record), with `brnr acp` and with
`brnr acp --strict`:

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

- Dropping them by default, as ADR 2 had it: it follows a protocol version
  no editor or adapter speaks yet, and it changes what the agent can do now
  (P1). A future protocol's direction isn't evidence that an agent without
  them works the same.
- Passing them through by default with a setting to drop them: nothing
  shows anyone needs it, and a setting is a surface to keep for a question
  v2 reopens anyway.
- Dropping `terminal` only, keeping `fs`: both are stable v1 client
  capabilities, and the argument is the same for each.
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
  - `adr_0067_the_editors_capabilities_reach_the_agent`.
  - `adr_0067_the_agents_file_and_terminal_requests_reach_the_editor`.
  - `adr_0067_headless_sessions_offer_neither`.

The real-editor checks above have no automated test: they are what the
matrix records.
