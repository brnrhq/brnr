# 10. The agent's stderr

Accepted 2026-10-07. Implemented.
Resolves review item 9.

## Context

The agent's stderr went to the host log, and with an editor through the
proxy to the editor, but headless nowhere else. "claude CLI not found" was in
neither a failed start's error nor the foreground's output; the start said
only "the agent exited before the session started".

## Decision

- Always: in the host log.
- With an editor: through the proxy, unchanged (P1).
- A failed headless start, detached or in the foreground, ends its error with
  the agent's last stderr lines, from a ring buffer of 20 (P3).
- In the foreground, the agent's stderr goes to the foreground's stderr as it
  comes, unchanged, as running the agent directly would; stdout stays the
  events. A supervisor's log collects it; at a terminal `2>/dev/null` hides
  it. The adapters are chatty (claude-agent-acp writes
  `[session/create] … phase=…` lines for every session).

## Considered

- The foreground showing it only when the start fails: a supervisor's log
  would lose the agent's diagnostics.
- Behind a flag.
