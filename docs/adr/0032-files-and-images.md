# 32. Files and images in a message

Accepted (former decision 15); reviewed 2026-10-07. Implemented.

## Decision

- `--file <path>`: a `resource_link` to the file; the agent reads it itself.
  ACP always allows it.
- `--image <path>`: an `image` block, base64-encoded, refused unless the
  agent advertises image prompts (P7).
- Both on `send` and `start`, read by the CLI; for `start`, before the
  process is launched (ADR 7).

## Considered

- Embedding the file's contents (`resource`): it needs the agent's
  `embeddedContext` capability (both adapters have it now), and duplicates
  what the agent can read anyway.
