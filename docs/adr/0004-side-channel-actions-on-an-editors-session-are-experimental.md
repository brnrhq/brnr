# 4. Side-channel actions on an editor's session are experimental

Accepted 2026-10-07. Implemented: each action is refused on an editor's
session unless its profile enables it, and in strict mode, and each
compensation in the table is sent. The `tool_call_update` for an approve or a
deny sets only `status` (`in_progress`, `failed`), which the agent's own
updates carry on from; the editor's acknowledgement of a `$/cancel_request`
(an error) is dropped without telling it. Not yet tried against real editors
(see Consequences).
Amends former decisions 9 and 11; resolves review item 6 with ADR 28.

## Context

ACP has one client per session. With an editor attached, the editor is that
client and owns the session (P11). Observing it and notifying other services
(`watch`, `log`, `status`, `notify`, bridges) is what the side channel is for
(P12). Acting on it is something ACP doesn't define and the editor doesn't
expect: each action leaves the editor's view behind the agent's in some way,
and only some of that can be made up for. These are what P1 calls
experimental: actions brnr adds to an editor's session.

## Decision

Every action on an editor-owned session through the control socket, from the
CLI or a bridge, is experimental, and refused unless the editor's part of the
profile enables it by name (ADR 33):

```toml
[profiles.work.editor]
experimental = ["send", "approve"]
```

Unknown names fail to load (P7). A refusal says what to add ("`approve` on an
editor's session is experimental; enable it with `experimental = ["approve"]`
under `[profiles.work.editor]`"). Observing (listing, showing, watching) is
always allowed.

| Action | Commands | Behaviour and compensation |
|---|---|---|
| `send` | `send` | Sent as a prompt only while the session is idle (no prompt in flight, the editor's or brnr's); refused while a turn runs. The editor controls its turns: whether a message mid-turn is queued, steered or interrupts is its call, so there are no held messages, `--steer` or `--interrupt` on an editor's session (ADR 18). Shown to the editor as a completed tool call (ADR 5). |
| `context` | `send --context`, `queue --clear-context` | Appended to the editor's own next prompt, and shown as "Context via brnr" (ADR 5). |
| `cancel` | `cancel` | The host answers the agent's pending permission requests `cancelled`, as ACP requires of whoever cancels, and withdraws them from the editor with `$/cancel_request`; late answers are handled as for `approve`. |
| `approve` | `approve`, `deny` | When a request is answered from outside, the host withdraws it from the editor with `$/cancel_request` (stable ACP: a notification that cancels a pending request), and sends a `tool_call_update` for the request's tool call. If the editor answers anyway, its answer is dropped (the agent must not get two) and the editor is told in the session that it was already approved or denied, and by whom. |
| `settings` | `mode`, `model`, `config` | The host sends the editor `current_mode_update`, or `config_option_update` made from the response's `configOptions`. The agent doesn't: the change was the host's request, and ACP answers the requester (ADR 28). |
| `close` | `close`, `start --resume --take-over` | Cancel a running turn; tell the editor in the session (a completed tool call, "Session taken over by brnr (process 4466)"); `session/close` to the agent. Afterwards the host answers the editor's requests for that session with an error saying where it continues. |

- `fork` stays refused on an editor's process: a forked session would be a
  headless session inside a process that ends with the editor, and ACP can't
  tell the editor of a session it didn't create.
- When the editor itself queues prompts, steers (`_session/steering`) or
  cancels, brnr passes it through untouched and only reads it (P1),
  recording `user_message` with `by: editor`.
- In strict mode none of these actions is available, whatever the profile
  enables (ADR 41).
- Refusing an editor's load of a session another process owns isn't one of
  them: it is process management, with its own feature flag (ADR 3,
  ADR 42).

## Consequences

- What each compensation does in real editors isn't known yet. They are to
  be tried against the editors brnr supports (Zed, IntelliJ's AIR plugin,
  others as they come), and adjusted per editor from what is seen; the README
  states what is known as caveats. The first case to try is in ADR 11.
- Answering approvals from somewhere else, which the README opens with, is
  available for an editor's session only by opt-in. It is the default for
  headless sessions.

## Considered

- Allowed, and labelled experimental only: it would still be default
  behaviour, reachable by accident (a Slack bridge approving what the user is
  looking at in the editor).
- One switch for everything: the actions differ in how much of the editor
  they leave behind.
- Opt-in per request (`--experimental`, a field in the request): every script
  and bridge would carry it, and the bridge protocol would change.
- Refused until tested: no way to try the compensations at all.
- A switch per command: some make no sense alone (`approve` without `deny`,
  `model` without `config`).
- `send` mid-turn on an editor's session passed through as a second prompt:
  its meaning would be the agent's and the editor's, which brnr can't promise
  (P2).
