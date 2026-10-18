# 16. Fork and close

Accepted (former decision 11); reviewed 2026-10-07. Implemented; on an
editor's session `close` is ADR 4's experimental action.

Amended by 63: a new session in a running process is offered after all,
`session new --pid` (and `session resume --pid`), refused where `fork` is
but in strict mode: `session/new` and `session/resume` are stable ACP. A
headless process with no session left stops unless one is being opened.

Amended 2026-10-16 (#230): a fork's source is recorded and shown. The
`session-opened` record that starts the fork's transcript, and the host
log's, name the session it was forked from, `forked_from` (ADR 22), and
`session list` (a FORKED FROM column) and `session status` say it, in text
and `--json` alike (P5), null where there is none. For a session open in a
process, the process says it if it opened the session as a fork, else its
transcript's first record does (a fork resumed since); for one that has
ended, its transcript's first record.

Amended 2026-10-17 (#238): a fork an editor makes through `brnr acp`
(`session/fork`) opens in its process as `brnr session fork`'s does: as the
agent answers, in the cwd the request names, with `forked_from`, the request
in its source's transcript and the fork's. Before, the process took it for a
session it had not heard of until its first prompt, opened it then in the
process's own cwd, and didn't know it was a fork.

## Decision

- `brnr fork <session>` forks the session (`session/fork`) into a new one in
  the same process and prints its id. Refused on an editor's process
  (ADR 4). `session/fork` is unstable in ACP v1: a convention both adapters
  implement, used by default and refused in strict mode (ADR 41). A plain
  "new session in this process" isn't offered: `brnr start` is the way to get
  one.
- `brnr close <session>` cancels a running turn and closes that one session
  (`session/close`); sessions beside it in the process keep running. A
  headless process with no session left stops. On an editor's session it is
  the experimental `close` (ADR 4). `start --resume --take-over` closes the
  same way (ADR 3).
- Messages still held in a closed session are reported dropped, then
  `session_closed` (ADR 20).
- Both are refused when the agent can't (no `fork` or `close` session
  capability) (P7), and `fork` when `stop_when_idle` is set and the agent
  can't close sessions (ADR 12).

## Considered

- `fork` and `close` refused outright while an editor is attached (former
  decision 11): `close` is possible with compensation (ADR 4); `fork` stays
  refused.
- Closing by stopping the whole process: it would take sibling sessions with
  it.

## Tests

Run `cargo test --release adr_0016_`. Named claims and their assertions:

- [tests/cli.rs](../../tests/cli.rs)
  - `adr_0016_fork_and_close`.
  - `adr_0016_close_cancels_the_turn_first`.
  - `adr_0016_a_fork_keeps_its_source`: `session list` and `session status`
    say which session a fork came from, and null for the parent; once the
    process has ended, and with the fork resumed in another process, its
    transcript still says it, its first record a `session-opened` with
    `forked_from`.
- [tests/headless.rs](../../tests/headless.rs)
  - `adr_0016_an_editors_fork_keeps_its_source`: an editor's `session/fork`
    through `brnr acp` is listed at once, in its own cwd, with
    `forked_from` in `session list` and `session status`; its transcript
    starts with a `session-opened` with `forked_from` and has the request.
