# 31. MCP servers for headless sessions

Accepted (former decision 13); reviewed 2026-10-07. Implemented, except the
redaction (ADR 25) and the move to the profile's headless part (ADR 33).

## Decision

- `[[profiles.<p>.headless.mcp_servers]]`, each with a `name` and either
  `command`, `args` and `env` (stdio) or a `url` with `type = "http"` or
  `"sse"` and optional `headers`, passed in the `session/new`,
  `session/resume`, `session/load` and `session/fork` requests the host
  sends.
- HTTP and SSE servers are refused at start if the agent doesn't advertise
  support for them (P7); codex-acp 2.1.1 has no SSE.
- With an editor, the MCP servers are the editor's: its own `session/new`
  carries them, and the profile adds none (P1).
- Their `env` and `headers` values are redacted in what brnr records
  (ADR 25).
