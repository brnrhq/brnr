# 45. Bug reports: `doctor --report`, and a link on a panic

Accepted 2026-10-08. Implemented. Part of #23's answer to error reporting:
reporting without telemetry (#24).

## Context

A bug report needs the brnr version, the OS, the adapters, what doctor
finds and what the host log says, and a user had to gather them by hand:
`--version`, `doctor --json` (which names paths and sessions) and a host log
from `~/.brnr/hosts/`, trimmed. A panic in the CLI said only what Rust says,
and a detached host's panic is recorded (ADR 11) where nobody reads it as it
happens. brnr listens on no network (P13) and never phones home (P15, ADR
44): what reaches an issue is what the user chose to paste.

## Decision

- `brnr doctor --report` runs the checks and prints, instead of them, a bug
  report to read, then paste into an issue: Markdown, starting with a
  comment that says to read it first, then
  - brnr's version and the OS (`macOS 26.4 (Darwin 25.4.0, arm64)`: the
    product's name and version from SystemVersion.plist or os-release, and
    `uname`'s);
  - the adapters, as doctor's lines have them (where each is, and the
    version, ADR 39);
  - the checks that aren't ok (`fail`, `warn` and `info`);
  - the last 20 lines of the latest host log, and of the latest log of a
    process that panicked or died without recording it (ADR 11), when that
    is another one, each with how it ended (`exited`, `brnr panicked`,
    `running`, `died without recording it`). A line longer than 4000
    characters is cut, saying how much more there was. Each line that is a
    record has what ADR 25 redacts redacted again, for a log an older brnr
    wrote; it is otherwise as recorded.
  - Everywhere, the home directory is `~`, as a path and as the start of a
    project folder's name.
- `--report --json` is one object with the same data (P5, ADR 34): `brnr`,
  `os`, `adapters` and `checks` (doctor's `{level, check, message}`), and
  `host_logs` (`run`, `ended`, `lines`, the lines as the text has them).
  `--report` exits as doctor does; `--fix` doesn't go with it.
- A panic prints, after Rust's message, a link to a pre-filled bug form
  (`issues/new?template=bug.yml`) with the title (`Panic at <file:line:col>`),
  `brnr --version`, the OS and the panic's message (at most 1000
  characters of it), URL-encoded: never the command's arguments, only which
  command it was. Nothing is sent; the user opens the link or doesn't. It
  goes where the user of the panicking process looks:
  - the CLI (every command but `host`), and `brnr acp`: on stderr, the
    editor's log for `acp`;
  - a host in the foreground (ADR 9): on its stderr, from its hook;
  - a detached host has no terminal (its stderr is its host log): when its
    start fails with the panic, `brnr start` and `brnr acp` print the link
    after the reason. A detached host's panic later on is the `exited`
    reason watchers and `wait` see, and the host log `doctor --report`
    shows: no link, which would be noise in every event a panic ends.
- The bug form asks for `brnr doctor --report` instead of `doctor --json`.

## Consequences

- The report shows the host log's lines as they are, prompts included (the
  `started` record has the request, ADR 8): only what brnr knows is secret is
  redacted. The comment at its top, the README and the form say to read it
  first.
- A panic that keeps the hook from running (an abort, ADR 11) prints no
  link.

## Considered

- Sending reports (telemetry, a crash reporter): brnr reaches nothing beyond
  the machine (P13, P15), and the user decides what to share.
- The session transcripts in the report: they hold prompts and code; `brnr
  log` is there for what shows the bug.
- Every host log of a process that died: the latest is what a report is
  about, and doctor's check already counts the rest.
- The command line in the link: arguments carry prompts and paths.
- Writing the link into the host log too: the log is the record, and the
  panic is in it.

## Tests

Run `cargo test --release adr_0045_`. Named claims and their assertions:

- [tests/headless.rs](../../tests/headless.rs)
  - `adr_0045_a_panic_prints_a_link_to_report_it`.
- [tests/doctor.rs](../../tests/doctor.rs)
  - `adr_0045_the_report_is_what_to_paste`.
  - `adr_0045_the_report_shows_a_death_without_a_record`.
