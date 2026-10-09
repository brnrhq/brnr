# 54. brnr speaks ACP version 1, and a start in another fails

Accepted 2026-10-08. Implemented (src/schema.rs, src/host/requests.rs,
src/ctl/settings.rs); resolves #66.

## Context

ACP's initialization has the client ask for the latest version it speaks
(`protocolVersion` in `initialize`); an agent that speaks it answers with
the same one, and one that doesn't answers with the latest it does. A
client that doesn't speak the version chosen is to close the connection
and tell the user.

brnr is the client in a headless start (`brnr start`) and in `brnr
sessions`, and asks for version 1, the one its schema types are (ADR 43).
It read the capabilities and login methods in the answer, whatever version
it chose, and went on: an agent answering `protocolVersion: 999` got
`session/new` and the prompt, in strict mode too, and the start succeeded.
What brnr then read of the agent was read as version 1 messages, which
they needn't be (P4).

## Decision

- brnr speaks ACP version 1 (`schema::PROTOCOL_VERSION`, the schema crate's
  `ProtocolVersion::V1`), and asks for it.
- An answer to its `initialize` that chooses another version, or none brnr
  can read as one (missing, a string, not a whole number up to 65535), ends
  there: nothing else of the answer is read, and nothing more is sent. A
  start fails before it commits (ADR 7), as any start does: `brnr start`
  exits with status 1 and says "the agent speaks ACP version 999; brnr
  speaks only version 1", or "the agent's answer to initialize has no ACP
  version brnr can read (protocolVersion: …)", the process stops, and with
  it the agent. `brnr sessions` fails with the same, and stops its agent.
- Strict mode and the default are the same here: version 1 is what brnr
  implements, so it is the only one either accepts. A version is added when
  it is implemented, by a decision that says how.
- An editor's process isn't the client: the editor's `initialize` and the
  agent's answer are passed through (P1), and the version they agree on is
  theirs to judge.

## Considered

- Going on with a version brnr doesn't speak, and reading what it can (P4
  read as "as far as brnr can"): what brnr reads and sends would be a guess
  at what another version means, and ACP says the client stops.
- Accepting a lower version (0, the pre-release one) as close enough: brnr
  doesn't implement it, and the schema crate calls it unsupported.
- Checking the editor's negotiated version too, and refusing for it: the
  editor is the client there, and brnr in the proxy role doesn't refuse
  what it can pass through (P4, as in ADR 50).
