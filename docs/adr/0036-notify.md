# 36. `notify`

Accepted (former decisions 17 and 23); reviewed 2026-10-07. Implemented.
Resolves review item 4 with ADR 35.

Amended by 63: `brnr notify` is `brnr event notify`, with the same flags,
and `watch` is `event watch`.

## Context

Notifications are what most people want from a bridge: a message when an
approval is waiting or a turn has ended. As a bridge, `notify` was run as
`sh -c 'exec brnr notify --pid "$BRNR_PID" -- …'`: it subscribed on the
control socket and never read its own stdin, where the process writes the
same events from birth. After 16 MiB the process cut it off and sent it
SIGTERM, and the notifications stopped without a word.

## Decision

- `brnr notify (<session> | --pid <pid>) [--events …] -- <command> [args...]`
  runs the command once per event. The default events are
  `permission_request`, `turn_ended` and `exited`; `--events` works as for
  `watch` (ADR 23), where `default` means these three.
- The event is in the command's environment (`BRNR_EVENT`, `BRNR_TEXT`,
  `BRNR_TITLE`, `BRNR_MESSAGE` with the session's last agent message,
  `BRNR_SESSION_ID`, `BRNR_REQUEST`, `BRNR_PID`) and, whole, as JSON on its
  stdin. Nothing goes on its command line, so the agent's text can't become
  arguments (P8). The text, title and message are escaped as `watch` shows
  them and cut at 32 KiB, so the command always starts.
- Commands run one at a time, in order, each waited for, in a process group
  of its own, with their stdout sent to stderr (as a bridge, `notify`'s
  stdout is read by the process as requests). No event is read while one
  runs, so a command that hangs makes `notify` fall behind, and the process
  cuts it off like any slow peer (ADR 6): a started bridge gets SIGTERM, a
  connection is closed. It exits when the process does, and
  `notify <session>` also when that session closes (`session_closed`).
- Cut off (SIGTERM, its stdin ending before `exited`, its connection closing
  before `exited`), or stopped by SIGHUP, SIGINT or SIGQUIT, `notify` stops
  the command it is running (SIGTERM to its process group, then SIGKILL once
  it has exited, or 2 s later), says on its stderr that it was cut off and
  that no more notifications come (`brnr: cut off (SIGTERM); stopped sh
  (turn_ended); no more notifications`), and exits non-zero (P3). A started
  bridge's stderr is in the host log (`bridge-stderr`). On the socket it
  watches for the connection closing while a command runs, and reads what
  was sent before it closed, no more than the socket held, to tell the
  process's end (`exited` among it) from being cut off.
- `brnr notify --stdin [--events …] -- <command>` reads its events from its
  stdin, a started bridge's transport (ADR 35), instead of connecting. That is
  how it runs as a bridge: `command = ["brnr", "notify", "--stdin", "--", …]`
  in the profile, with no `sh -c` and no `$BRNR_PID`, and one subscription.

## Considered

- Shipping scripts for a few services: one command that runs any command
  covers them all.
- Running commands concurrently: it would reorder notifications.
- `notify` draining its stdin while subscribed on the socket: two
  subscriptions for one bridge.
- `notify` reading stdin when it detects it's a bridge: a guess (P4); the
  flag says it.
- Review item 4 rejected reading events from stdin (its D, the option now
  taken) because `notify` would behave one way as a bridge and another way
  by hand. With an explicit `--stdin` the difference is asked for, not
  hidden, and it is the only option with one subscription per bridge.
- The process writing to a started bridge's stdin only once it subscribes:
  ADR 35.

## Tests

Run `cargo test --release adr_0036_`. Named claims and their assertions:

- [tests/cli.rs](../../tests/cli.rs)
  - `adr_0036_notify_runs_a_command_per_event`.
  - `adr_0036_notify_reads_events_as_watch_does`.
  - `adr_0036_notify_works_as_a_bridge`.
  - `adr_0036_notify_reads_stdin_as_a_bridge`.
  - `adr_0036_notify_stdin_ends_with_its_input`.
  - `adr_0036_notify_fails_when_cut_off`.
  - `adr_0036_notify_cut_off_as_a_bridge_stops_its_command`.
  - `adr_0036_notify_cut_off_on_the_socket_stops_its_command`.
  - `adr_0036_notify_cuts_what_the_environment_cant_hold`.
