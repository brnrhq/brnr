# 35. Bridges

Amended by 48: a request, `logged`, answered once the process has written
what it recorded until then.

Amended by 63: requests `new` and `resume` open a session in the running
process (`session new --pid`, `session resume --pid`).

Accepted (from former decisions 25 and 37); reviewed 2026-10-07.
Implemented.
Resolves review item 4 with ADR 36.

## Context

A bridge connects brnr's sessions to something else: Slack, push
notifications, a phone. A started bridge used to be dropped when its stdout
closed, though it was still running and reading: a bridge that ran
`exec cat > file` got no events and was gone in 4 ms.

## Decision

- A bridge is any process that speaks brnr's JSON-lines protocol. Requests:
  `status`, `send`, `cancel`, `queue`, `subscribe`, `pending`, `approve`,
  `deny`, `set_mode`, `set_config`, `set_model` (the config option of
  category `model`, ADR 28), `fork`, `close`, `stop` (src/host/control.rs);
  those
  about a session name it by its exact id. Events: as ADR 20, 22 and 23 list
  them, each naming its session; `exited` is the process's.
- A request line that isn't valid UTF-8 is answered with an error, and the
  bridge stays (fixed in #15).
- Two transports: a connection to the process's control socket
  (`$BRNR_DIR/<pid>.sock`, which only the user can open, P13), or a child
  the process starts from the profile's `bridges`, whose stdout carries its
  requests and whose stdin carries events.
- A started bridge is subscribed from birth (every event but `acp`, or the
  bridge's `events` in the profile), so it sees everything from the
  process's start. Its environment has `BRNR_PID` and `BRNR_SOCKET`: a
  process's sessions come and go, and it has none when its bridges start.
- A started bridge ends when its process exits. Its stdout closing only means
  it has no more requests; it keeps getting events. When the brnr process
  stops, the bridge gets the last events (`exited`), its stdin closes (it
  should exit then), and one still running 2 s later (`BRIDGE_EXIT`) gets
  SIGTERM: time for a bridge to act on `exited`, as `notify` does.
- A bridge has to keep reading: one 16 MiB behind is cut off (ADR 6).
- On an editor's session, a bridge's actions are experimental, as the CLI's
  are (ADR 4). `stop` isn't one: stopping the process is process
  management, never experimental and not refused in strict mode (`command`
  in src/host/control.rs).

## Considered

- A started bridge ending when its stdout closes (as was): one that only
  listens would have to keep its stdout open.
- Started bridges getting no events on stdin until they subscribe: a change
  to the protocol, and they would miss the process's start.

## Tests

Run `cargo test --release adr_0035_`. Named claims and their assertions:

- [tests/security.rs](../../tests/security.rs)
  - `adr_0035_a_bridge_gets_no_raw_acp_unless_it_asks`.
- [tests/headless.rs](../../tests/headless.rs)
  - `adr_0035_bad_request_line_is_answered`.
- [tests/cli.rs](../../tests/cli.rs)
  - `adr_0035_a_bridge_that_closes_its_stdout_gets_events`.
