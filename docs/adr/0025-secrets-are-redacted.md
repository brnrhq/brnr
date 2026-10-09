# 25. Secrets are redacted in what brnr records

Accepted 2026-10-07. Implemented.
Resolves review item 10.

## Context

A profile's MCP servers go to the agent with their `env` and `headers`
(`GITHUB_TOKEN = "…"`, `Authorization`) in the `session/new`, `load`,
`resume` and `fork` requests the host sends, and an editor's own
`session/new` carries its servers the same way. These were recorded in the
host log and the session's transcript (`host_request`,
src/host/requests.rs), and sent to `acp` subscribers. Transcripts are
readable only by the user, but that is a token on disk once per session, and
in every bridge that asks for `acp`, which may forward it anywhere.

## Decision

The values of MCP servers' `env` and `headers` are replaced by
`"<redacted>"` in everything brnr records (the host log, the raw ACP file of
ADR 22, the `started` request of ADR 8) and in `acp` events, whoever sent the
request. Events never carry them. The agent gets them as before, and the
editor's bytes pass unchanged (P1). This is the one exception to P5's raw
ACP: the keys and the structure stay, so the interpretation can still be
checked.

## Considered

- Keeping them, protected by file permissions only.
- Not recording these requests at all: the record of the session's opening
  would be lost.

## Tests

Run `cargo test --release adr_0025_`. Named claims and their assertions:

- [tests/security.rs](../../tests/security.rs)
  - `adr_0025_a_resumed_sessions_secrets_are_redacted`.
- [tests/headless.rs](../../tests/headless.rs)
  - `adr_0025_a_profiles_mcp_secrets_are_redacted`.
  - `adr_0025_an_editors_mcp_secrets_are_redacted`.
- [src/log.rs](../../src/log.rs)
  - `adr_0025_secrets_are_redacted_keys_and_structure_stay`.
