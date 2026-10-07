# 41. Strict mode

Accepted 2026-10-07. Not yet implemented.

## Context

brnr follows ACP faithfully (P1). To the letter, that would leave out what
agents and editors already implement ahead of the spec, and what people use:
steering a running turn, forking a session. Practical, it leans on
conventions that a stricter setup may not want: an extension method, a
`_meta` field, a method the schema still marks unstable.

What the ACP schema marks, as of `@agentclientprotocol/sdk` 1.6.0: in v1,
`session/list`, `resume`, `close` and `delete` are stable, `session/fork` is
unstable, and `fs` and `terminal` are stable client capabilities; v2 drops
`fs`, `terminal`, `session/load` and `session/set_mode`. `$/cancel_request` is
stable in both. Extension methods start with `_` (`_session/steering`).

## Decision

- By default brnr speaks stable ACP plus the conventions current agents and
  editors implement alike. Each is listed here, with what brnr does with it:
  - `_session/steering`, advertised as `_meta.steering.supported` in
    `initialize`, with `_meta.steering.idleBehavior: "promptRequired"` (both
    adapters): `send --steer` (ADR 18).
  - `session/fork`, unstable in ACP v1 (both adapters): `brnr fork`
    (ADR 16).
  - Dropping the editor's `fs` and `terminal` capabilities, as ACP v2 does
    (ADR 2).

  A convention is added to brnr by adding it to this list.
- Strict mode, per process (`strict = true` in a profile, `--strict` on `acp`
  and `start`), is stable ACP to the letter and nothing else: no extension
  methods, no unstable methods, no `_meta` field acted on, the editor's
  capabilities passed through, and none of ADR 4's experimental actions,
  whatever the profile enables. What strict mode refuses says why
  ("`--steer` uses `_session/steering`, an ACP extension, which strict mode
  doesn't").
- Strict mode is about the protocol only. Headless operation, observing a
  session, brnr's CLI and its process management (ADR 42) are the same with
  it and without.
- Stable means what the ACP schema doesn't mark unstable, as
  `agent-client-protocol-schema` builds it without its unstable features
  (ADR 43).

## Considered

- Stable ACP only, always: it leaves out steering and forking, which both
  adapters implement and people use.
- Each convention as an experimental action, enabled by name like ADR 4's:
  conventions don't act on an editor's session behind its back; a switch per
  process is enough.
- Strict mode also deciding process management (whether an editor may share
  a session another process owns): that isn't protocol (ADR 42).
