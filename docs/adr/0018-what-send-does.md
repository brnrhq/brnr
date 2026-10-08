# 18. What `send` does

Accepted 2026-10-07. Implemented. On an editor's session `send` is ADR 4's
experimental action: refused while a turn runs, and never held, steered or
interrupting. Replaces former decision 24.

## Context

`send` while a turn ran sent another `session/prompt`, and the README said
claude-agent-acp folds it into the running turn. ACP doesn't say what a
prompt sent mid-turn means, and the adapters differ. claude-agent-acp 0.85.1
queues it as a turn of its own (`turnQueue`, advertised as
`_meta.claudeCode.promptQueueing`); codex-acp 2.1.1 replaces the session's
active prompt with it (`trackActivePrompt`), and stops following the first
one's turn (read from the code, not run). The CLI of a headless session is a
contract (P12) and can't depend on either (P2).

Both adapters implement steering, `_session/steering`, advertised as
`_meta.steering.supported` in `initialize`. With
`_meta.steering.idleBehavior: "promptRequired"`, a steer answers `injected`
while a turn runs, its output streaming within that turn, and
`promptRequired` when the session is idle, leaving the client to send it as
a normal prompt (claude-agent-acp's `steer()`).

## Decision

For headless sessions:

| | Idle | A turn is running |
|---|---|---|
| `send` | sent as a prompt | held; sent as its own turn when the running one ends, in order |
| `send --steer` | sent as a prompt, not steered (as `promptRequired` would have it) | injected into the running turn; refused if the agent doesn't advertise steering (P7), or in strict mode |
| `send --interrupt` | sent as a prompt | `session/cancel`, then sent ahead of what is held |
| `send --context` | appended to the next prompt | appended to the next prompt |

- Each held message is its own prompt and turn, in the order sent;
  interrupts go ahead of held messages, keeping their own order.
- A second `session/prompt` is never sent while one runs.
- `_session/steering` is an ACP extension, a convention both adapters
  implement alike: used by default, refused in strict mode (ADR 41). What a
  second prompt mid-turn means isn't a convention: the adapters disagree
  (P2).
- `--after-turn` is gone: it is what `send` does now (P9).
- The response's status is `delivered`, `held`, `steered` or `interrupting`
  (`queued`, a second prompt mid-turn, is gone).
- On an editor's session the editor controls its turns: `send` is
  experimental there, and refused while a turn runs (ADR 4).

## Considered

- Passing a mid-turn prompt through and saying per agent what it does: the
  CLI couldn't promise one behaviour, and an adapter release could change
  what `send` does.
- Steering by default where the agent offers it, holding elsewhere: the
  default's meaning would vary by agent.
- Sending everything held during a turn as one prompt when it ends: closer to
  how Claude Code batches queued messages, but `--wait` would no longer map
  one message to one turn.
