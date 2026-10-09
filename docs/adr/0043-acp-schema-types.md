# 43. ACP types from the official schema crate

Accepted 2026-10-07. Implemented with `agent-client-protocol-schema`
1.10.2, without schemars and without its unstable features (src/schema.rs).
Read with its types: session updates, the capabilities and login methods in
`initialize`'s answer, the session `session/new` or `session/fork` opened,
permission option kinds, JSON-RPC errors, and `brnr sessions`' `session/list`.
Written with its types: `session/set_config_option`'s params (ADR 28).
Strict mode's stable methods are the crate's. Left to `serde_json::Value`,
as the types don't take them or brnr passes them on as they were said: the
JSON-RPC envelope and the editor's requests, extension methods and `_meta`,
the unstable `fork` capability, update kinds the schema lacks, what brnr
rewrites, and what events and the status carry as it was said (a prompt's
content, a permission request's tool call and options, locations, plan
entries, config options, commands, modes, stop reasons).

## Context

brnr forwards the original bytes and interprets a copy of each message
(P1), with hand-written lookups (`msg["params"]["sessionId"]`). The official
Rust libraries, as of October 2026:

- `agent-client-protocol` (3.1.0), built on Symposium's `sacp`: `Client`,
  `Agent`, `Proxy` and `Conductor` roles, typed and untyped messages, async.
  Its proxies follow the proxy-chains RFD, still a prototype: a proxy runs
  under a conductor, which starts it with `proxy/initialize` and talks to it
  through `proxy/successor` envelopes.
- `agent-client-protocol-schema` (1.10.2): the protocol's types, with serde;
  ACP v2's draft behind `unstable_protocol_v2`; no runtime.

## Decision

- brnr keeps its byte relay and its threads, and interprets its copy of each
  message with `agent-client-protocol-schema`'s types where they take what
  it reads: the parts of messages the status line lists (session updates,
  `initialize`'s capabilities and login methods, the session opened,
  permission option kinds, JSON-RPC errors, `session/list`), each read as
  its type in place of hand-written lookups. The envelope and the rest stay
  `serde_json::Value`, read as far as brnr can (src/schema.rs).
- Its stable types are what P1 and strict mode (ADR 41) mean by stable ACP.
- The crate parses with serde_json too, so reading lone surrogates and deep
  nesting stays brnr's own work (ADR 26).
- A message the types don't know (an extension method, a newer field) is
  still forwarded untouched, and interpreted as far as brnr can (P4).

## Considered

- The full SDK's `Proxy` and `Conductor`: a rewrite onto async; every message
  decoded and encoded again, so byte fidelity would depend on routing
  everything through its raw paths; and brnr isn't a proxy in the RFD's
  sense, since the editor runs no conductor.
- A spike of an SDK-based proxy first, to measure that.
- Keeping hand-written access to `serde_json::Value`: nothing tells brnr what
  is stable ACP and what isn't.
- Later, perhaps: brnr as a component in someone else's conductor chain, an
  experimental feature once the proxy-chains RFD is stable.
