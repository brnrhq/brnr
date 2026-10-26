# Use it from an editor

Configure your editor's ACP agent command as:

```sh
brnr acp -- brnr-claude-adapter  # or claude-agent-acp, from npm
brnr acp -- brnr-codex-adapter   # or codex-acp, from npm
brnr acp -- brnr-muse-adapter    # or muse-acp, from @brokkai/muse-acp
brnr acp --profile work          # agent and settings from a profile
```

A bare agent name is looked up next to `brnr` first, so the editor's `PATH`
doesn't need to include it; so is a [bridge](profiles.md#bridges)'s, such as `brnr` in
`brnr event notify`.

`brnr acp` passes bytes, signals (HUP, INT, QUIT, TERM, USR1, USR2), the
agent's stderr, stdin's EOF, its stdout closing and the exit status through
unchanged. It changes the stream in three ways: every request reaches the
agent with an id of brnr's, and its answer comes back to the editor with the
editor's own id, so that the editor's requests and brnr's are never confused
(ADR 2.4); and, each below, a session another brnr process serves can't be
loaded, and the experimental actions a profile enables act on the editor's
session. By default the editor's `fs` and `terminal` capabilities don't
reach the agent, as ACP v2 has neither: the agent reads and writes the files
on disk and runs commands itself, and doesn't see the editor's unsaved
buffers. A profile passes them by name (`client_capabilities = ["fs",
"terminal"]`), and strict mode passes both, as stable ACP v1 has them; a
headless session offers neither
([ADR 2.7](../adr/0002-acp-faithfully.md#27-the-editors-file-system-and-terminal-reach-the-agent-in-strict-mode-or-when-its-profile-names-them)). When the editor goes away, so does the agent;
`brnr session resume <session>` carries on with one of its sessions headless
(a turn still running is lost).

Watching an editor's session (`event watch`, `event log`, `session status`,
`permission requests`, `permission show`, `elicitation requests`,
`elicitation show`, `event notify`, bridges) always works. Acting on it doesn't, by default: ACP has one
client per session, the editor, and every action from outside leaves the
editor's view behind the agent's in some way.

## Experimental actions

Each is refused on an editor's session unless the editor's part of its profile
enables it by name (without `--profile`, the profile is `default`):

```toml
[profiles.work.editor]
experimental = ["send", "permission"]
```

| Action | Commands | What brnr does for the editor |
|---|---|---|
| `send` | `prompt send` | Sent only while no turn runs: the editor controls its turns, so nothing is held, steered or interrupts. Shown as a completed tool call, "Message via brnr", or as the user's message (`echo_as`, below). |
| `context` | `prompt send --context`, `queue clear --context` (or neither flag) | Added to the editor's next prompt, shown as "Context via brnr", or as the user's message (`echo_as`). |
| `cancel` | `prompt cancel` | The agent's pending approvals are answered `cancelled` and its elicitations `cancel`, and withdrawn from the editor (`$/cancel_request`). |
| `permission` | `permission allow`, `permission reject` | The request is withdrawn from the editor, its tool call set `in_progress` or `failed`, and the editor told at once who answered ("Allowed via brnr", "Rejected via brnr"). If the editor answers anyway, its answer is dropped and it is told who answered first. |
| `elicitation` | `elicitation accept`, `elicitation decline`, `elicitation cancel` | The request is withdrawn from the editor, and the editor told at once who answered ("Accepted via brnr", "Declined via brnr"). If the editor answers anyway, its answer is dropped and it is told who answered first. |
| `config` | `config set` | The editor is sent the `current_mode_update` or `config_option_update` the agent sends only to whoever asked. |
| `close` | `session close`, `session resume --take-over` | The turn is cancelled, the editor is told ("Session closed via brnr", "Session taken over by brnr (process 4466)"), and its later requests for the session get an error saying where it continues. |

A message or context is a tool call because not every editor renders a user
message that arrives outside a turn. For an editor that does, `echo_as =
"user_message_chunk"` in the editor part shows it as the user's message
instead, one `user_message_chunk` per content block
([ADR 7.2](../adr/0007-experimental-actions-on-an-editors-session.md#72-how-a-message-sent-through-brnr-is-shown-to-the-editor)).
brnr's own notes ("Allowed via brnr" and the rest) stay tool calls.

`session fork` is never available in an editor's process, and in strict mode
none of these is, whatever the profile enables. `brnr process stop` isn't an
action on a session: it stops an editor's process as it does any other.

## What the editor doesn't see

- The editor's child is `brnr acp`, not the agent, so its pid and process tree
  differ.
- A SIGKILL sent to `brnr acp` can't be passed on. brnr sees it go and kills
  the agent's process group, as whenever the editor goes.
- Loading or resuming a session another brnr process serves is refused, with a
  JSON-RPC error naming the process and how to release it
  (`brnr session close <session>`, or closing it in the other editor that has
  it open): the editor can't pass `--take-over`. `shared_sessions` lets it
  through ([feature flags](#strict-mode-and-feature-flags)).
- A message sent from outside reaches the agent as an ordinary user message,
  but the editor is shown a completed tool call, the one update editors render
  anywhere in a turn; brnr's notes to it take the same form. A turn
  `prompt send` starts is one the editor didn't start.
- The editor's acknowledgement of a `$/cancel_request` (an error) is dropped
  without telling it.
- The agent's stdout and stderr reach the editor as they would directly: stdout
  ends when the agent closes it, even if the agent runs on, and stderr comes as
  it is written, newline or not. From when stdout ends, brnr's own notes to the
  editor (a message sent from outside, a refusal) aren't shown; the host log
  says which. The editor's stdin closes when the agent's does (a stop, the
  agent closing it or exiting), so its next write fails, as on a pipe; what
  it wrote before then goes nowhere, and the host log says how much. An
  agent that closes its stdin itself is found out only by a write that
  fails: that line is recorded, and its request waits, as if it had been
  sent, and the writes after it fail
  ([ADR 2.6](../adr/0002-acp-faithfully.md#26-the-editors-stdin-and-stdout-end-when-the-agents-do-and-its-stderr-comes-as-written)).
- Both streams come through one connection, in order, with 16 MiB in flight
  rather than a pipe's 64 KiB: an editor that stops reading one of them holds
  up the other too, once that much is waiting. The editor's stderr is also
  `brnr acp`'s own, for its errors, so it ends when `brnr acp` exits rather
  than when the agent closes its stderr, and an editor closing it doesn't give
  the agent EPIPE.
- None of the compensations above has been tried against real editors yet
  (Zed, IntelliJ's AIR plugin): how each shows them is still to be seen.
  Each action stays experimental through 1.x, and leaves it only by an ADR
  that brings what was seen in real editors; ADR 2.7 lists the checks.

# Strict mode and feature flags

By default brnr speaks stable ACP plus the conventions agents and editors
already implement alike, ahead of the spec: `_session/steering` (for
`prompt send --steer`) and `session/fork`, unstable in ACP v1 (for
`brnr session fork`). Strict mode (`strict = true` in a profile,
`--strict` on `acp`, `session new` and `session resume`) is stable ACP to
the letter: no steering, no fork, and no experimental actions. What it
refuses says why.

Either way, brnr speaks ACP version 1. A `brnr session new` (or `brnr session list` with an agent)
whose agent answers `initialize` with another version, or one brnr can't
read, fails with that, before anything else is sent, and the agent is
stopped ([ADR 2.2](../adr/0002-acp-faithfully.md#22-brnr-speaks-acp-version-1)). An editor's process
passes the editor's `initialize` and the agent's answer through: the version
they agree on is theirs.

How brnr's processes share sessions isn't protocol, and strict mode doesn't
change it: it has defaults that protect you, and feature flags in a profile's
editor part to change them. The one so far is `shared_sessions`: the editor
may load a session another brnr process serves. The agent then serves it to
both and the conversation splits; the process holding the session's lock still
writes its transcript, and `session status` says the session is shared.
