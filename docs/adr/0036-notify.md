# 36. `notify`

Accepted (former decisions 17 and 23); reviewed 2026-10-07. Implemented.
Resolves review item 4 with ADR 35.

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
- Commands run one at a time, in order, each waited for, with their stdout
  sent to stderr (as a bridge, `notify`'s stdout is read by the process as
  requests). A command that hangs makes `notify` fall behind, and the process
  cuts it off like any slow peer (ADR 6); `notify` then fails, saying so. It
  exits when the process does.
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
