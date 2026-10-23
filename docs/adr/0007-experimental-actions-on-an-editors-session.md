# 7. Experimental actions on an editor's session

Draft 2026-10-19. Implemented, but 7.1's real-editor checks are not run
yet: every cell is untested. Factor: F4 (ADR 1). Brings together former
ADRs 4, 5 and 70, and the second decision of 67 (docs/adr/README.md maps
them).

What brnr may do, through the side channel, to a session an editor owns,
and how the editor is shown it. These actions are useful, but ACP doesn't
define them and the editor doesn't expect them, so each needs compensating
behaviour that isn't faithful to ACP. They are opted into one by one, by
name, and stay experimental through 1.x (F4).

## 7.1 Side-channel actions on an editor's session are experimental, for all of 1.x

Every action on an editor-owned session through the control socket, from
the CLI or a bridge, is experimental. It is refused unless the editor's
part of the profile enables it by name (ADR 6.1):

```toml
[profiles.work.editor]
experimental = ["send", "permission"]
```

- An unknown name fails to load (P7). A refusal says what to add:
  "`permission` on an editor's session is experimental; enable it with
  `experimental = ["permission"]` under `[profiles.work.editor]`". Without
  `--profile`, the profile named is `default`.
- Observing is always allowed: `event watch`, `event log`,
  `session status`, `permission requests`, `permission show`,
  `elicitation requests`, `elicitation show`, `event notify`, bridges.
- In strict mode none of these actions is available, whatever the profile
  enables (ADR 2.1).

The actions, their commands, and what the host does for the editor:

- `send` (`prompt send`): sent as a prompt only while the session is idle
  (no prompt in flight, the editor's or brnr's), and refused while a turn
  runs. The editor controls its turns: whether a message mid-turn is
  queued, steered or interrupts is its call, so an editor's session has no
  held messages, `--steer` or `--interrupt` (ADR 4.2). The editor is shown
  the message as ADR 7.2 has it.
- `context` (`prompt send --context`; `queue clear` with `--context` or
  with neither flag): appended to the editor's own next prompt, and shown
  as ADR 7.2 has it ("Context via brnr").
- `cancel` (`prompt cancel`): the host answers the agent's pending
  permission requests `cancelled`, as ACP requires of whoever cancels, and
  its pending elicitations `cancel` (ADR 4.5), and withdraws them from the
  editor with `$/cancel_request`. Late answers are handled as for
  `permission`.
- `permission` (`permission allow`, `permission reject`): the host
  withdraws the request from the editor with `$/cancel_request` (stable
  ACP: a notification that cancels a pending request), and sends a
  `tool_call_update` for the request's tool call that sets only `status`,
  `in_progress` or `failed`, which the agent's own updates carry on from.
  It tells the editor in the session that the request was allowed or
  rejected, and by whom ("Allowed via brnr", "Rejected via brnr"). If the
  editor answers anyway, its answer is dropped (the agent must not get
  two), and the editor is told in the session that it was already allowed
  or rejected, and by whom ("Already allowed via brnr", "Already rejected
  via brnr").
- `elicitation` (`elicitation accept`, `elicitation decline`,
  `elicitation cancel`): the request is withdrawn from the editor with
  `$/cancel_request`, and the editor is told in the session ("Accepted via
  brnr", "Declined via brnr"). Its late answer is dropped, and it is told
  who answered first, as for `permission` (ADR 4.5).
- `config` (`config set`): the host sends the editor
  `current_mode_update`, or `config_option_update` made from the
  response's `configOptions`. The agent doesn't: the change was the host's
  request, and ACP answers the requester (ADR 6.2).
- `close` (`session close`, `session resume --take-over`): the host
  cancels a running turn, tells the editor in the session ("Session closed
  via brnr", or "Session taken over by brnr (process 4466)"), and sends
  `session/close` to the agent. Afterwards it answers the editor's
  requests for that session with an error saying where the session
  continues, but for a load or resume, which opens it again where no
  other process holds it (ADR 3.9).

The editor's acknowledgement of a `$/cancel_request` (an error) is dropped
without telling it. Beside the actions:

- `permission show` on an editor's request says where it can be answered:
  in the editor, and here only where `permission` is enabled; otherwise it
  gives the refusal `permission allow` would (`answerable`, `why_not` in
  each pending entry). `elicitation show` does the same for `elicitation`
  (ADR 4.5).
- `process stop` isn't one of these actions: stopping a process is process
  management, the same as `kill`.
- `session fork`, `session new --pid` and `session resume --pid` are
  refused on an editor's process. A session opened there would be a
  headless session inside a process that ends with the editor, and ACP
  can't tell the editor of a session it didn't create.
- When the editor itself queues prompts, steers (`_session/steering`) or
  cancels, brnr passes it through untouched and only reads it (P1),
  recording `user_message` with `by: editor`.
- Refusing an editor's load of a session another process owns isn't one
  of these actions: it is process management, with its own feature flag
  (ADR 3.9, ADR 3.10).
- Answering approvals from somewhere else is available for an editor's
  session only by opt-in. It is the default for headless sessions.

Every action here is experimental for all of 1.x, opt-in by name. None
becomes a default, or stops needing its opt-in, by a release alone. An
action leaves experimental only by an ADR of its own, which brings
evidence from real editors: the checks below for that action, run in each
editor brnr supports, with each editor's and adapter's version, what was
seen, and the cells left untested, said as such. The synthetic suite
(`tests/fake_agent.py`) passing isn't that evidence: it shows what brnr
sends, not what an editor makes of it. The compensations are adjusted per
editor from what is seen, and the documentation states what is known as caveats.

**Why.** ACP has one client per session. With an editor attached, the
editor is that client and owns the session (P11). Observing it and
notifying other services is what the side channel is for (P12). Acting on
it is something ACP doesn't define and the editor doesn't expect: each
action leaves the editor's view behind the agent's in some way, and only
some of that can be made up for. These are what P1 calls experimental:
actions brnr adds to an editor's session. Whether a compensation works can
only be seen in a real editor, and none has been tried yet.

**Considered.**

- Allowed, and labelled experimental only: it would still be default
  behaviour, reachable by accident (a Slack bridge allowing what the user
  is looking at in the editor).
- One switch for everything: the actions differ in how much of the editor
  they leave behind.
- Opt-in per request (`--experimental`, a field in the request): every
  script and bridge would carry it, and the bridge protocol would change.
- Refused until tested: no way to try the compensations at all.
- A switch per command: some make no sense alone (`permission allow`
  without `permission reject`, `config set --model` without the other
  options).
- `send` mid-turn on an editor's session passed through as a second
  prompt: its meaning would be the agent's and the editor's, which brnr
  can't promise (P2).
- Promoting an action out of experimental in 1.x now: there is no
  real-editor evidence yet for any of them.

### Real-editor checks

To be run by hand: an editor can't be faked. Each cell is an editor (Zed;
a JetBrains IDE through its ACP support, IntelliJ's AIR plugin; others as
they come) with an adapter (`brnr-claude-adapter`, `brnr-codex-adapter`),
and records the editor's, the adapter's and brnr's versions, the OS, and
pass, fail or untested for each check. The raw ACP (`brnr event log
<session> --events acp`) shows what crossed. Every check is also run with
the agent configured directly, without brnr, as the baseline: a difference
from it is the finding, not the behaviour alone. None has been run yet.

File system and terminal, which check ADR 2.7: with `brnr acp` and a
profile's `client_capabilities = ["fs", "terminal"]`, and with `brnr acp
--strict`; and, for the default, with `brnr acp` and no such setting:

0. The default: the agent sends neither `fs/*` nor `terminal/*`, reads and
   writes the files on disk itself, and runs commands in processes of its
   own; the host log has `capabilities_dropped`.
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
   and the editor is told.

The editor's path (#69):

5. Prompts and cancel: a prompt, a long turn cancelled from the editor,
   and a prompt after it.
6. Approvals: a permission request shown and answered in the editor, both
   allow and reject.
7. Settings: the editor's mode and model selectors change the agent's, and
   the agent's changes move them.
8. Disconnect: closing the editor's thread, window or the editor itself
   ends the agent and its children (ADR 3.1, ADR 3.7); restarting the
   agent from the editor leaves nothing behind.
9. Resume: after the editor closes, `brnr session resume <session>` goes
   on headless; the editor loads the session again once that process has
   closed it (ADR 3.9).

Experimental actions, per action, with the action enabled. These are the
cells an ADR promoting an action cites:

- whether the editor acts on a `$/cancel_request` from the agent's side
  (the request withdrawn from its view), and answers it with -32800, as
  ACP asks;
- how it renders a `tool_call_update` with status `in_progress` or
  `failed` for a tool call whose permission request it is showing;
- how it renders the echo form (a completed tool call, ADR 7.2) when it
  arrives between turns, not only within one, and, with `echo_as =
  "user_message_chunk"`, whether it renders an out-of-turn
  `user_message_chunk` (#216);
- how it shows a JSON-RPC error on its `session/prompt` after the session
  was closed or taken over from outside, and whether loading the session
  again then works;
- whether an unsolicited `current_mode_update` or `config_option_update`
  moves its mode or model selector;
- how it shows a turn it didn't start (`send` while it was idle), and
  whether it lets the user prompt while that turn runs;
- whether it sends `_session/steering` itself;
- from ADR 3.7, the first case to try, IntelliJ restarting its agents:
  whether the old proxy, host and agent are SIGKILLed, and by whom.

## 7.2 How a message sent through brnr is shown to the editor

A message or context sent to an editor's session through the side channel
(`prompt send`, `prompt send --context`) is shown to the editor in the
form `echo_as` in `[profiles.<p>.editor]` chooses, by the `sessionUpdate`
it is sent as:

- `"tool_call"`, the default: one `session/update` with `sessionUpdate:
  tool_call`, `toolCallId: brnr-echo-<n>`, kind `other`, status
  `completed`, titled `Message via brnr` (or `Context via brnr`), with the
  message's content blocks as its content. It is the one update editors
  render as a block of its own at any point in a turn.
- `"user_message_chunk"`: one `session/update` `user_message_chunk` per
  content block of the message, as the agent sends a user's message when
  it replays a session; no title, no tool call. It carries no "via brnr":
  the editor shows it as the user's, which it is.

Either way:

- Another value fails to load, naming the two (P7). `brnr doctor` shows
  the profile's `echo_as` when it isn't the default.
- It is sent when the message is sent; when the agent takes it up is the
  agent's business.
- Only the editor sees it, in either form. Events, bridges and the
  transcript's story keep `user_message` with `by: control`.
- `echo_as` applies to messages and context only. brnr's own notes to the
  editor are always tool calls in the same form: an approval or
  elicitation answered elsewhere ("Allowed via brnr", "Rejected via brnr",
  "Accepted via brnr", "Declined via brnr"), the editor's late answer to
  one ("Already allowed via brnr", "Already rejected via brnr"), a session
  closed or taken over ("Session closed via brnr", "Session taken over by
  brnr (process 4466)"), and a close that failed (ADR 7.1).
- Both forms are stable ACP, so strict mode doesn't change `echo_as`.
  There it is moot, as strict mode has no experimental actions (ADR 2.1).

**Why.** In ACP's model, user messages are what the client sends. Some
editors don't render a `user_message_chunk` that arrives out of turn,
mid-turn or between turns, and drop or misplace it; a completed tool call
they all render, so it is the default. Other editors do render the chunk,
and for them the tool call puts the user's own words in a block that says
a tool ran, where `user_message_chunk` is what ACP has for them (P1, P2).
brnr can't tell which editor it serves (P4), so the profile says. brnr's
notes are brnr's words: a user chunk would make them the user's, as an
agent chunk would make them the agent's.

**Consequences.** Which editors render an out-of-turn
`user_message_chunk`, and how each shows the tool call, is checked in real
editors (ADR 7.1, #216). Until then the default is the form every editor
renders.

**Considered.**

- `user_message_chunk` as the only form: honest about what it is, but not
  rendered by every editor.
- `agent_message_chunk` or `agent_thought_chunk`: rendered, but the text
  appears as the agent's own words.
- A per-editor default, from the editor's `clientInfo`: brnr would guess
  what an editor renders (P4), and be wrong for the next version of it.
- `user_message_chunk` for brnr's notes too: they would read as the
  user's words.
- `echo_as` as an experimental action, named in `experimental`: it doesn't
  act on the session; it chooses how something an action does is shown.

## Tests

Run `cargo test --release adr_0007_`. Named claims, by section:

- 7.1: [tests/headless.rs](https://github.com/brnrhq/brnr/blob/main/tests/headless.rs):
  `adr_0007_take_over_from_an_editor_is_refused`,
  `adr_0007_experimental_actions_are_refused_without_opt_in`,
  `adr_0007_cancel_withdraws_the_editors_requests`,
  `adr_0007_approve_answers_in_the_editors_place`,
  `adr_0007_settings_are_told_to_the_editor`,
  `adr_0007_close_tells_the_editor`,
  `adr_0007_take_over_from_an_editor`,
  `adr_0007_the_editors_own_steer_is_recorded`;
  [src/host/sessions.rs](https://github.com/brnrhq/brnr/blob/main/src/host/sessions.rs):
  `adr_0007_a_session_closed_under_the_editor_is_its_again_once_loaded`.
  The real-editor checks have no automated test: they are what the matrix
  records.
- 7.2: [tests/headless.rs](https://github.com/brnrhq/brnr/blob/main/tests/headless.rs):
  `adr_0007_send_to_an_editors_session_waits_for_no_turn`,
  `adr_0007_context_joins_the_editors_next_prompt`,
  `adr_0007_messages_can_be_shown_as_user_message_chunks`.
