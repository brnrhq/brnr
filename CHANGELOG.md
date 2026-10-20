# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

brnr's own conventions:

- A pull request that changes what users notice adds its entry under
  [Unreleased]. `release.sh` dates that section for the release, and a
  release's section is its GitHub release's notes (ADR 40).
- Before 1.0 nothing is kept for compatibility (ADR 1, P9). A breaking
  change goes under Changed or Removed, starts with **Breaking:**, and says
  what to do instead.
- From 1.0.0, what ADR 64 covers is kept compatible within a major version.
  A breaking change to it waits for the next major, where it is marked as
  above. A form replaced in a minor release goes under Deprecated, says what
  to use instead, and keeps working until the next major.
- Security names a fixed vulnerability by its advisory (`GHSA-…`) and CVE
  ids where it has them.
- An entry cites the ADR it follows. 0.6.0 and earlier came before
  `docs/adr`, so they cite the record that now holds the decision.

## [Unreleased]

### Added

- `turn_ended`, the status's `last_turn` and `prompt send --wait --json` have
  `tokens`: the counts the agent reported in its answer to the prompt (ACP's
  unstable `usage`), `total`, `input`, `output`, `thought`, `cached_read` and
  `cached_write`, each null where it gave none that is a count, and null when
  the answer has no `usage` object. brnr adds nothing up. `session status`
  shows the last turn now, in text and `--json`, and a turn's text line ends
  with its counts: `turn ended: end_turn (control); tokens: 1.2k total, 1.0k
  input, 234 output` (ADR 4.4).
- `session list` and `session status` say which session a fork came from
  (`forked_from`, a FORKED FROM column in the list), also once its process
  has ended: the fork's transcript records it in its first record,
  `session-opened`. That is so for a fork `session fork` makes and for one
  an editor makes through `brnr acp` (`session/fork`) (ADR 3.12, ADR 5.1,
  #238).

### Changed

- **Breaking:** brnr refuses a config file that isn't yours, or that the
  group or others can read or write, or whose folder isn't yours or they
  can write, or, on macOS, either with an access control list (`chmod
  -N` removes it), as ssh does its keys: the file names the commands brnr runs
  and can hold MCP secrets. Every command that reads it fails, naming the
  path, the owner or mode, and the `chmod` or `chown` that fixes it, and
  `brnr doctor` fails its config check the same way; brnr doesn't change
  the file. A config written as 0644 now fails: `chmod 600` it. A symlink
  is followed, and the file it points at, and that file's folder, are
  checked (ADR 9.4, #271).
- The decision records are 11 topic records, each decision a numbered
  section, cited as `ADR T.k` (`ADR 3.9`: ownership is a per-session lock).
  Entries of releases before this one cite the former numbers, 1 to 71;
  [docs/adr/README.md](docs/adr/README.md) maps each to its section now.
- **Breaking:** `brnr acp` drops the editor's `fs` and `terminal` client
  capabilities from its `initialize` again by default, as ACP v2 has
  neither, and notes `capabilities-dropped` in the host log. A profile
  passes them by name, `client_capabilities = ["fs", "terminal"]` in its
  editor part, and strict mode passes both, as stable ACP v1 has them
  (ADR 2.7, #272).
- The Claude adapter is built from claude-agent-acp 0.90.0
  (`brnr-claude-adapter`) (ADR 11.3).
- A command refused for where its session is says it in the same words,
  whichever command it is: `session delete` of a session open in a process
  says `sess-1 is running in process 4466: close it first (…)`, and
  `session resume` and `session delete` of one whose process doesn't
  answer, is opening it or isn't listed say which, without offering
  `--take-over` or closing it first, which can't work there (ADR 3.9,
  #281).
- **Breaking:** each cwd has a project folder of its own: a new session's
  transcript goes in `~/.brnr/projects/<claude>-<hash>/`, `<claude>` the
  cwd as `~/.claude/projects` names it and `<hash>` 8 hex digits of a hash
  of the cwd (`-Users-me-src-3db3a304`), where `/a-b` and `/a/b`, and on
  macOS `/A` and `/a`, shared one folder. Transcripts already written stay
  where they are, and a session resumed in the cwd it began in goes on in
  its file there. Nothing to do, but a tool that joins brnr's folders to
  `~/.claude/projects` by the exact name now matches by the prefix (ADR 9.2,
  #120).

### Fixed

- What reads transcripts holds every part of their path to the rules they
  are written by: `session list`, `event log`, `session resume`, `session
  delete`, catching up and `doctor` don't look in, or read through, a
  symlinked `$BRNR_HOME/projects`, `hosts` or state directory, nor read a
  transcript or host log with another hard link or of another user's, and
  `doctor --fix` makes nothing private through one: it changes a mode
  through what it opened, not by name. A symlinked `projects` listed the
  sessions in whatever it pointed at (ADR 9.3, #302).
- `brnr doctor` warns of a state directory, `hosts` or `projects` that
  isn't a directory, where it said a private one was fine, and names the
  problem of the path it gives as an example (ADR 9.3).
- `doctor --report` redacts the MCP secrets (ADR 9.1) in a host-log line
  nested deeper than 128, and shows it as a record, as it shows the others;
  it showed such a line as written, so an older brnr's unredacted secret in
  it was in the report. The line is read as transcripts are (ADR 2.5, ADR
  11.5, #303).
- A flag's value is never taken as a request for help: `brnr session new
  --prompt -h` sends `-h` as the prompt, where it printed the usage, as
  `prompt send` already did (ADR 8.1).
- A watcher cut off while its process stops reads what it missed from the
  transcript, where it could fail with `the process closed the connection`:
  a stopping process hangs up on the question whether it cut the watcher
  off, so the watcher waits for it to go and asks its host log (ADR 10.2,
  #275).
- A start whose `session/load` the agent fails, after replaying the history,
  records the last message replayed, as an editor's and a bridge's failed
  load do; it was lost (P3, ADR 5.6, #291).
- The editor's `session/load` that the agent fails, after replaying the
  history, holds the session's lock until the process has written what it
  recorded of the replay, 2 s at most, as a bridge's and a start's do, and
  `status` lists it as closing meanwhile. Its lock went at once, and a
  resume in another process could write before those records (ADR 5.5,
  #291).
- The editor's `session/resume` that the agent fails, of a session the
  editor closed moments before, holds the session's lock until the
  process has written the close's records, 2 s at most, as a failed load
  does. Its lock went at once, and a resume in another process could write
  before the `session_closed` (ADR 5.5, #304).
- Messages and context held in a session whose load the agent fails, or in
  one the agent opened that isn't the process's to serve, are dropped with
  their `message_dropped` and `context_dropped` events, `by` `close`, with
  no `session_closed` after; they went without a word (P3, ADR 5.2, #304).
- A Claude fork can be prompted in the process that forked it, where its
  first prompt failed with `Session not found`: claude-agent-acp 0.90.0
  opens the session it forks. `adapters/check.py --brnr` checks a fork
  prompted and closed in one process, and its parent prompted after (ADR 3.12,
  ADR 11.3, #227).
- A process whose disk stalls as it exits can be ended with a signal
  (Ctrl-C, `kill`), and says on stderr how much of its records it didn't
  write; until then it waits for the disk, as before. A panic lets go of
  the process's sessions before it waits, as an exit does (ADR 5.5, #165).
- A headless process no longer dies when `response_timeout` answers a
  permission request nested deeper than its main thread's stack takes (a
  million deep): the event loop's timers run on a stack that takes what the
  host holds, as its events do (ADR 2.5, #189).
- A bridge's load of a session (`session resume --pid`, an agent without
  `session/resume`) finds its transcript in any folder, and one the process
  served and closed moments before, its logger behind, where it recorded
  the agent's replay on top of the history again; so does the editor's
  load of one its process served and closed (ADR 5.6, #190).
- A load after one that failed records the history the agent replays: the
  events file a failed load leaves, holding only its openings, no longer
  counts as brnr's transcript of the session, for `session resume`, a
  bridge's resume or an editor's `session/load` (ADR 5.6, #191).
- A process counts a session as having a transcript once it has recorded
  something of it beyond its opening, as the disk does: a load there after
  one that replayed and then failed, its logger behind, no longer records
  the history again, and one after a session closed with all its records
  skipped by a slow disk records it, as another process's would (ADR 5.6,
  #247).
- A load that replays and then fails, the editor's or a bridge's, records
  the last agent message the agent replayed, marked replayed, as the rest
  of the replay; it was dropped (ADR 5.6, #255).
- A fork an editor makes through `brnr acp` (`session/fork`) is a session of
  its process as the agent answers, in the cwd the request names: it was
  listed, and its transcript started, only at its first prompt, under the
  process's cwd, and its modes and config options weren't known until the
  agent reported them again (ADR 3.12, ADR 5.1, #238).
- `brnr permission requests` lists a request nested deeper than 128,
  where it failed with `recursion limit exceeded`: it reads the host's
  answer as the host reads the agent's line (ADR 2.5, #239).
- `brnr prompt send --wait`, `event wait`, `event watch` and `event notify`
  (from the socket and `--stdin`) see a permission request nested deeper
  than 128, which they dropped without a word: they read events as the
  host reads the agent's line (ADR 2.5, #250).
- Those commands, and `session new --wait`, follow an event nested about a
  million deep, where they overflowed their stack and aborted (exit 134):
  each event is used and let go of on a stack that takes it, as the host
  does; with no such stack to be had, they say so and exit non-zero (ADR 2.5,
  #252).
- `brnr permission show` shows a request nested deeper than 128, and
  `elicitation requests` and `elicitation show` work in a process where one
  waits, where each failed with `recursion limit exceeded`: every command
  reads the host's answers as `permission requests` does (ADR 2.5).
- `brnr permission requests` and `elicitation requests`, with no session,
  say a process failed when one does after another held a request nested
  about a million deep, where they overflowed their stack and aborted: what
  was read already is let go of on a stack that takes it (ADR 2.5, #301).
- `brnr event log` shows a record nested deeper than 128, such as a deep
  `permission_request`, as `event watch` showed it live, where it said the
  line `isn't a record brnr can read, not shown`. `session list`, `session
  resume` and `doctor` read such a record too, where they passed over it
  for the one before: a session whose last record nests that deep is
  listed as last active then (ADR 2.5, ADR 5.1).
- `session list`, `event log`, `session resume`, catching up and `doctor`
  no longer read a transcript through a symlink, nor in a symlinked
  project folder, which they listed and read as the user's sessions though
  brnr never writes there; `session delete` already left them alone. The
  transcripts brnr writes are where they were (ADR 9.3).

## [0.9.0] - 2026-10-16

### Added

- `process stop` and `session close` take `--json`: `{"pid": <n>,
  "status": "stopping"}` and `{"session": "<id>"}`, what their text says
  (ADR 34).
- The `history` event has `loads`, how many loads its replay covers: more
  than 1 when an editor loads a session again before the agent has answered
  its last load, and `recorded_updates`, how many of its `updates` the
  transcript has (ADR 57).
- A `session_reopened` event, `by` `editor`, says that the agent answered an
  editor's load of a session with success after answering its close: the
  session is open in the editor's process again, and the load's `history`
  follows, counting what it replayed across the close; what came while the
  session was closed isn't recorded, and `recorded_updates` says so:
  `history: 4 updates replayed by the agent, 3 recorded` (ADR 57).
- Elicitations, the agent asking for input (`elicitation/create`): a form,
  or a URL to open, such as an MCP server's login. Headless, brnr advertises
  both modes, which claude-agent-acp needs for AskUserQuestion. `brnr
  elicitation requests`, `show`, `accept`, `decline` and `cancel` list, show
  and answer them; `accept --field name=value` (or a JSON object on stdin)
  is checked against the form before anything is sent, and a URL is shown,
  never opened. One outside any session is named with `--pid`. Events
  `elicitation_request`, `elicitation_resolved` and `elicitation_completed`,
  for watchers, bridges and editor sessions too; `event wait --for
  elicitation`; `event notify` runs for `elicitation_request` by default;
  `prompt send --wait` announces one on stderr; `session status` counts
  `elicitations`, and a session with one is `waiting`. The socket's
  `pending` has `elicitations`, and `elicitation` answers one. On an
  editor's session, answering is the experimental `elicitation` (ADR 68).
- A command the process cuts off for falling behind catches up: `event
  watch`, `event notify`, `event wait` and the `--wait`s ask the process
  whether it cut them off (`was_cut_off`, a new request), subscribe again,
  and read what they missed from the transcript before going on live, so a
  burst bigger than a peer's queue and the 64 MiB set aside (#117) no longer
  loses events. Where the transcript can't have them (`log = false`,
  `log = "events"` for `acp`, records skipped), or cut off 4 times in a row
  while catching up, the command says what it missed, an `events-missed`
  note, and exits 3 (ADR 69).
- Every event, live and in the transcript, and every raw ACP record has its
  position in its process, `seq`; `subscribe` answers with the connection's
  `peer`, `host_id` and the `seq` it starts after, as `session new --wait`'s
  ready report does; `logged` answers `lost` when a file has lost records it
  hasn't noted yet; and `records-skipped` says the positions it lost,
  `first_seq` and `last_seq` (ADR 69).
- A `peer_cut_off` event tells the other peers that one was cut off, and the
  `last_queued` event it was sent, as the host log's `peer-dropped` now says
  too (ADR 69).
- `echo_as` in a profile's editor part: `"user_message_chunk"` shows a
  message or context sent to the editor's session through brnr as the
  user's message, one `user_message_chunk` per block, for an editor that
  renders one outside a turn; `"tool_call"`, "Message via brnr", stays the
  default. brnr's own notes stay tool calls (ADR 70).
- Release candidates, `vX.Y.Z-rc.N`: a GitHub pre-release, and on crates.io
  (`cargo install brnr --version X.Y.Z-rc.N --locked`), while Homebrew stays
  on the last release. A release's notes take in its release candidates',
  so they say everything since the last release (ADR 40).

### Changed

- **Breaking:** a headless session's elicitation waits for an answer, where
  brnr declined it at once: an unattended run now waits on it. Set
  `response_timeout` (or `--response-timeout`), which has no default and
  cancels one nobody answered, never declines or accepts it (ADR 68).
- **Breaking:** the headless `permission_timeout` is `response_timeout`, and
  `session new` and `session resume`'s `--permission-timeout` is
  `--response-timeout`: one timeout for the user's responses, which answers
  a permission request nobody answered `reject_once` (else cancels its turn)
  and an elicitation `cancel`. Rename the key in your profiles and the flag
  in your scripts; a profile with `permission_timeout` fails to load
  (ADR 68).
- **Breaking:** `prompt send`'s options go before its prompt: from the
  prompt's first word on, every word is the prompt's, as after `--`, so an
  option after the text (`brnr prompt send $s "fix it" --wait`) is sent as
  text; put it before the text. `-` (stdin) goes alone: a word after it
  fails the command; quote a prompt that starts `- ` (ADR 63).
- **Breaking:** `prompt send --timeout` without `--wait` fails, as
  `session new`'s does, where it was ignored; drop it or add `--wait`
  (ADR 21).
- **Breaking:** `event log` has no `-f`: use `--follow`. A word with one
  dash is an argument there and in `session new` and `resume`, as for the
  other commands, not an unknown option (ADR 63).
- **Breaking:** a session's lock is `$BRNR_HOME/sessions/<id>.lock`, beside
  its transcript, where it was `$BRNR_DIR/sessions/<id>.lock`. Two
  processes whose runtime directories differed (on Linux, a shell with
  `XDG_RUNTIME_DIR` and a cron job without it) but shared `~/.brnr` each
  took a lock of their own, so both could own one session and append to one
  transcript, and each environment's commands saw only its own process. Now
  the second resume is refused, naming the process and its runtime
  directory (`sess-1 is running in process 4466 with runtime directory
  /run/user/1000/brnr (BRNR_DIR=/run/user/1000/brnr reaches it)`), with or
  without `--take-over`; `session list` there shows the session
  `unreachable` with that pid; commands on it, and an editor's load of it,
  are told the same, with the directory after `BRNR_DIR=` quoted for the
  shell where it needs to be; and `doctor` lists locks left by processes that are
  gone under `session locks`. `log = false` still writes the locks there. A
  lock left in `$BRNR_DIR/sessions/` is read by nothing: remove the
  directory. How the runtime and state directories are found, and how a
  session id (`file_name`) and a cwd (`project_key`) become file names, are
  fixed for 1.x (ADR 3, ADR 65).
- **Breaking:** `brnr acp` no longer drops the editor's `fs` and `terminal`
  client capabilities, in strict mode or without: the agent gets them as the
  editor offers them, its `fs/*` and `terminal/*` requests go to the editor,
  and the editor's answers come back to it. Whether real editors then serve
  unsaved buffers and their terminals as they do run directly isn't checked
  yet (ADR 67's real-editor checks, #216). There is no setting to drop them
  again: whether to serve them is the editor's. A
  headless session still offers neither, and the host log has no
  `capabilities-dropped` record (ADR 67).
- Every side-channel action on an editor's session (`send`, `context`,
  `cancel`, `permission`, `config`, `close`) stays experimental, opt-in by
  name, for all of 1.x; one leaves experimental only by an ADR with evidence
  from real editors (ADR 67).
- From 1.0.0, what `docs/interface.md` marks **Covered** is kept compatible
  within a major version: the command line, `--json`, exit codes, events,
  the protocol of the bridges brnr starts, transcripts, where brnr's files
  are, config and what `brnr acp` passes through. Text output, error
  wording, host logs and the control socket are not covered. Until 1.0.0
  nothing is kept for compatibility, as before (ADR 64).
- `event notify` on the socket, cut off for falling behind while its
  command runs, lets the command finish and catches up, where it stopped
  the command and exited 1; a started bridge (`--stdin`) is cut off as
  before (ADR 69).
- The adapters are built from claude-agent-acp 0.89.1 (`brnr-claude-adapter`)
  and codex-acp 2.2.2 (`brnr-codex-adapter`) (ADR 39).

### Removed

- **Breaking:** the `brnr` crate's public library API is removed: its
  modules (`brnr::render`, `brnr::schema`, `brnr::json` and the rest) are
  private, and its one public item is `brnr::run`, the command line as a
  function. Run `brnr` with `--json`, or call `brnr::run`, instead; a typed
  facade that mirrors the command line follows #150 (ADR 66).

### Fixed

- A detached process's own stderr (`host-stderr` in the host log) is read
  in pieces of 64 KiB at most, as the agent's and a bridge's are, where a
  line of any length was held whole. The agent's stderr pieces end between
  characters, so a character the 64 KiB boundary cut is no longer two
  U+FFFD in the host log; a line of exactly 64 KiB is one record, not one
  and an empty one; and a failed start's tail cuts a long line between
  characters (ADR 11, ADR 51).
- `brnr prompt send $s explain ls -h` sends `explain ls -h`: `-h` and
  `--help` print the usage only before the prompt, where they printed it,
  sending nothing, from anywhere in it (ADR 63).
- `session status --json` without a session prints its usage, and an option
  `session status`, `session close` or `process stop` doesn't take is said
  as unknown, where each was taken for the session or pid (`no session
  --json`) (ADR 63).
- `session list`, `process list`, `doctor` and `skill` say which option
  they don't know, as the other commands do, where they printed their usage
  (ADR 63).
- Text says in words what `--json` has as a value (ADR 34): a failed turn
  is `turn failed: boom (control)`, without JSON quotes; an exit is
  `agent exited with code 1`, `agent killed by SIGTERM` or `agent exited
  (status unknown)` (ADR 11), in `event watch`, `event log`, `event notify`'s
  `BRNR_TEXT` and `event wait --for exit`; `event watch --pid` shows each
  session's whole id, to paste into the next command (ADR 13); and
  `session status` says `working` for a busy session whose turn time it
  doesn't have, and `waiting for an approval` for one that is waiting,
  where it said `idle`.
- `permission requests` with no session lists the approvals in the
  processes that answer, and exits 0, when one doesn't: it passes over a
  process whose status went unanswered, as `process list` and
  `session list` do, saying `process <pid> is not answering; its approvals
  aren't listed` on stderr, instead of waiting for it again and failing
  with `process <pid>: not answering` (ADR 27, ADR 3).
- `session list` shows LAST ACTIVE in UTC, in `--json` and text alike, and
  orders rows by the time each names: an agent's `updatedAt` with an offset
  (`2026-10-11T08:00:00+02:00`) was shown hours off and sorted by its
  digits. It is now written as brnr writes its own times
  (`2026-10-11T06:00:00.000000Z`); one that isn't an RFC 3339 time is shown
  as the agent gave it, said on stderr, after the sessions with a time
  (ADR 63).
- An editor's `session/load` through `brnr acp` records the agent's replay
  only when brnr has no transcript of the session, marked `replayed`, with a
  `history` event either way, as a headless load does. Loading a session
  already open in the editor's process no longer records its history again
  or leaves its unfinished tool calls running in `session status`, and a
  load keeps the title and commands the replay gives. Loads that overlap
  share one replay, recorded marked until the last is answered, and one
  `history` event, rather than the later ones' replay being recorded as
  live. The editor still gets the replay as the agent sent it (ADR 57).
- `session status` no longer lists as running the tool calls a cancelled or
  failed turn left unfinished: a turn's tool calls end with it (ADR 34). A
  `tool_call` again with the id of a running call updates that call, with no
  second entry and no second start (ADR 23).
- A session closed in a process (`session close`, `session resume
  --take-over`, a process's last session) is let go of, and the close
  answered, once the process has written its records of it, for 2 s at
  most, as an exit is: a resume elsewhere appends after its
  `session_closed`, and a load there finds its transcript and doesn't record
  the history again. Meanwhile `event log` of it waits for those records,
  and `session resume` and `session delete` say it is closing there, free
  in 2 s at most (ADR 48).
- A session whose load fails after the agent has replayed its history
  (`session resume --pid`, or a start) is held, and the command answered,
  the same way, once the process has written what it recorded of the
  replay, where its lock went at once and a resume elsewhere could write
  before those records, or record the history again (ADR 48).
- After an ACP line nested deeper than 128, the host takes a stack that deep
  only while it keeps something of the line, not for every event after it
  (ADR 26).
- A started bridge's stderr is read as bytes and decoded lossily, so a line
  that isn't UTF-8 no longer stops the reading and kills the bridge with
  SIGPIPE on its next write; a line longer than 64 KiB goes to the host log
  in pieces of that, as the agent's stderr does (ADR 35, ADR 51).
- `prompt send --interrupt` while no turn runs but the last turn's
  `turn_ended` waits for a steer's answer is held, ahead of what is held,
  and goes once that turn has ended: before, it started a turn at once, and
  if that turn ended first the earlier turn's `turn_ended` was lost, its
  message's `--wait` timed out, and the late steer was put in the wrong turn
  (ADR 56).
- A headless process with `stop_when_idle` waits without using a whole CPU
  while the agent is slow to answer a session's `session/close`, once the
  session's idle time has run out (ADR 12, ADR 16).
- The crate on crates.io carries NOTICE, as Apache-2.0 asks a
  redistribution to (ADR 47).
- brnr works started with SIGCHLD ignored, as some supervisors leave it.
  On Linux, `brnr acp` and `brnr session new` failed to start their process
  (`No child processes`), and a process never saw its agent exit, so one in
  the foreground never exited. brnr sets SIGCHLD to its default for itself;
  what it starts, the agent among it, still finds it ignored, as it would
  started directly (ADR 2, ADR 8, ADR 9).

## [0.8.0] - 2026-10-09

### Added

- `session new` and `resume` take `--thought-level <l>`, the option of
  category `thought_level`, and `--permission-timeout <s>`, which wins over
  the profile's `permission_timeout` as the other flags win over theirs
  (ADR 58, ADR 63).
- `brnr queue show <s> <m>` shows one held message in full: its text,
  whether it interrupts, and its attachments; the socket's `queue` takes
  `show` (ADR 63).
- `brnr session delete <s>` has the agent delete its copy of a session
  (`session/delete`), the agent brnr recorded for it or the one named, and
  is refused for a session open in a process and for an agent that can't
  delete sessions. brnr's transcript stays, with a new `session_deleted`
  event in it, as it does for an editor's `session/delete` through
  `brnr acp`. `--purge` also deletes brnr's transcript of the session (its
  events and raw ACP files, not the host logs) whatever the agent answered,
  and exits non-zero unless the agent deleted the session or doesn't have it
  (`resource_not_found`; the Claude adapter answers another error, so
  `--purge` of a session it already deleted exits non-zero). A file that
  can't be deleted, or have `session_deleted` recorded in it, is said and
  exits non-zero, the others done all the same, and a transcript made
  private first is said on stderr (ADR 63).
- `session new --pid <pid>` and `session resume --pid <pid> <session>` open
  a session in a running process, with its agent, profile and MCP servers,
  and the socket and bridges have the same as `new` and `resume`. The
  session, its settings and its prompt commit as one, as a start's do: a
  setting that fails, or the start timeout passing (the process's, as for a
  start), closes the session again and says so (or names the session left
  open, for an agent that can't close sessions); the process stopping fails
  it; and a command gone first leaves no session. A session being opened
  keeps the process from stopping as its other sessions close or go idle.
  The process flags and `-- <agent>` are an error with `--pid`; an editor's
  process refuses, as does one with `stop_when_idle` whose agent can't close
  sessions; `--take-over` works as without `--pid`, and first checks that
  the process would resume the session (ADR 63).

### Changed

- **Breaking:** commands are grouped by what they act on,
  `brnr <group> <verb>`, and the old names fail as unknown commands, with no
  aliases: `ps` and `stop` are `process list` and `process stop`; `status`,
  `fork` and `close` are `session status`, `session fork` and
  `session close`; `send`, `cancel` and `commands` are `prompt send`,
  `prompt cancel` and `prompt commands`; `queue <s>` is `queue list <s>`
  (and `queue clear <s>`, below), and `queue --drop <m>` is
  `queue drop <s> <m>`; `pending` and `show` are `permission requests` and
  `permission show`; `log`, `watch`, `notify` and `wait` are `event log`,
  `event watch`, `event notify` and `event wait`. Flags are unchanged. A
  profile's bridge that runs `brnr notify` runs `brnr event notify` instead.
  `brnr --help` has a section per group, `brnr <group> --help` lists a
  group's commands, and `brnr <group> <verb> --help` (or `-h`, before any
  `--`) prints that command's usage (ADR 63).
- **Breaking:** `brnr mode`, `brnr model` and `brnr config` are
  `brnr config get <s>` and `brnr config set <s>`. `get` lists every option
  with its category, value and choices, each choice with its name and
  description, the current one marked, and the agent's v1 modes as a row
  with no option; `--mode`, `--model`, `--thought-level` and
  `--option <o>` narrow it. `set` takes `--mode <m>`, `--model <m>`,
  `--thought-level <l>` (the option of that category) and
  `--option <o>=<v>` (by id) together, resolved as a start's settings are:
  `brnr mode $s plan` is `brnr config set $s --mode plan`, and
  `brnr config $s effort=high` is `brnr config set $s --option effort=high`.
  A mode, model or thought level the agent has no option for fails ("the
  agent offers no thought level"), as do two values for one setting, and a
  v1 mode the agent doesn't list fails before anything is sent, a start's
  too; an `--option` id the agent hasn't advertised is sent, and a refusal
  says what was already set. `get` lists only advertised options (ADR 63).
- **Breaking:** an agent with a config option of category `mode` has its
  mode set through that option, and `session status` reports its value,
  even where the agent has v1 modes too; `session/set_mode` is sent only
  with v1 modes and no mode option. `session status --json` has `modes` as
  the agent gave them (`currentModeId`, `availableModes`) (ADR 63).
- **Breaking:** the socket's `set_mode`, `set_model` and `set_config` are
  one `set_config`, with `mode`, `model`, `thought_level` and `options` (an
  object of option ids to values), answered once every setting is set with
  what was sent (`set`); a failure names what was set before it (ADR 63).
- **Breaking:** the editor's experimental action `settings` is `config`:
  `experimental = ["config"]` under `[profiles.<name>.editor]`; `settings`
  fails to load (ADR 4, ADR 63).
- **Breaking:** `brnr start` is `brnr session new`, and
  `brnr start --resume <s>` is `brnr session resume <s>`, with every flag
  `start` took but `--set`, which is `--option <o>=<v>`; `start` fails as an
  unknown command, with no alias. `--take-over` goes with `session resume`
  only. A start is atomic as before, and `resume` brings a recorded
  session's cwd, agent and profile as `--resume` did (ADR 7, 14, 63).
- **Breaking:** a profile's headless `config` is `options`, beside the new
  `model` and `thought_level`: `config = { model = "opus" }` becomes
  `model = "opus"` (or `options = { model = "opus" }`). A profile with
  `config` fails to load, and `brnr doctor` says so (ADR 33, ADR 63).
- **Breaking:** `approve` and `deny` are `brnr permission allow` and
  `brnr permission reject <session> <request> [--always] [--option <id>]`,
  and answer with the request's option of one kind: `allow_once`,
  `allow_always` (`--always`), `reject_once` or `reject_always`. No other
  kind stands in, so a request without that kind, or with two, fails,
  listing the options: where `approve` took `allow_always` and `deny`
  answered `cancelled`, name the option with `--option`, or cancel the turn
  with `prompt cancel`. `--option` of an ACP kind must be on the verb's side,
  and the always kind with `--always`. The socket's `approve` and
  `deny {option}` are `allow` and `reject {always, option}`, and the editor's
  experimental action `approve` is `permission` (ADR 63).
- **Breaking:** `permission_timeout` answers with the `reject_once` option;
  a request without one has its turn cancelled (`session/cancel`), which
  answers every request pending in the session `cancelled`. It never
  answers `reject_always`, which it took where there was no `reject_once`
  (ADR 63).
- **Breaking:** `permission_resolved` has `answer` (`allowed`, `rejected` or
  `cancelled`) and `option_kind`, the chosen option's kind, and reads
  `permission p1 allowed with allow (allow_once), by …` as text. The
  editor is told "Allowed via brnr" or "Rejected via brnr" (ADR 63).
- **Breaking:** `queue --clear` and `--clear-context` are
  `queue clear <s> --messages` and `--context`, and `queue clear <s>` with
  neither drops both; `queue list` takes neither flag (ADR 63).
- **Breaking:** `brnr session list` replaces `list` and `sessions`, which
  fail as unknown commands. On its own it is brnr's index, open and ended
  sessions in every cwd, with nothing started (what `list --all` was;
  `--include active` for what `list` was, `--include inactive` for
  `list --inactive`). With `--profile` or `-- <agent>` it is what `sessions`
  was, joined: the cwd's sessions (here, or `--cwd`) and the agent's, every
  page of `session/list`, on the session id; an agent that can't list fails
  it. A cwd is the same however it is spelled (a trailing slash, a symlink),
  and an `unreachable` session is the cwd's if the agent lists it there or
  brnr has its transcript there. A SOURCE column after AGENT (`source` in `--json`) says who knows each,
  `brnr`, `agent` or `both`; a session only the agent knows is `inactive`
  where it was `-`, and every row has its AGENT (ADR 63).

## [0.7.0] - 2026-10-09

### Added

- `brnr start --auth <method>`, `--strict` (and `brnr acp --strict`), and
  `--resume <session> --take-over` (ADR 30, 41, 3).
- `send --steer` sends into the running turn, where the agent can be
  steered (ADR 18).
- A session is locked by the process that serves it. An editor's load of a
  session another process holds is refused unless the editor part has
  `features = ["shared_sessions"]` (ADR 3, 42). A session that can't be
  locked isn't served headless; an editor's is passed through, and `status`
  shows `lock_error` (ADR 50).
- Events: `message_dropped`, `context_dropped` and `session_closed`
  (ADR 20), `tool_progress` (ADR 23), and `line_too_long` (ADR 51).
- `brnr doctor --report`: a bug report to paste into an issue, redacted.
  A panic prints a link to a pre-filled issue; nothing is sent (ADR 45).
- `brnr skill`, the skill for agents that use brnr, and
  `brnr skill install` (ADR 46).
- brnr is on crates.io: `cargo install brnr --locked` (ADR 47).
- Each release has its source tarball as an asset, with build provenance
  (`gh attestation verify`), and a CycloneDX SBOM; the Homebrew formulae
  build from that tarball. Release builds are checked to be reproducible
  (ADR 52).
- A panic is recorded, as `panic` and an `exited` with the reason; the agent
  is killed and the runtime files removed (ADR 11).
- `start --foreground` passes the agent's stderr through, and a failed start
  ends with the agent's last stderr lines (ADR 9, 10).
- `doctor` checks what keeps a session from being locked, and tells
  processes that are gone by their socket, so a reused pid isn't taken for
  one (ADR 11, 50).
- `docs/interface.md`, the reference for commands, JSON, exit statuses,
  configuration and the bridge protocol, and `docs/threat-model.md`.
- A `history` event after a session is loaded says how many updates the
  agent replayed, and whether brnr recorded them or its transcript already
  had the session. `--events history` selects it; it's shown by default
  (ADR 57).

### Changed

- **Breaking:** brnr never follows a symlink in its state directory. A
  symlinked `~/.brnr` or `$BRNR_HOME`, project folder or transcript is
  refused, as are another user's paths and transcripts with other hard
  links. A refused host log fails the start, and a refused session file is
  `session-log-failed` in the host log. Set `BRNR_HOME` to the directory a
  symlink pointed at instead (ADR 59).
- **Breaking:** profiles have parts. `[profiles.<p>.headless]` has `cwd`,
  `mode`, `config`, `mcp_servers`, `permission_timeout`, `stop_when_idle`
  and `auth`; `[profiles.<p>.editor]` has `experimental` and `features`.
  `log` is `"all"`, `"events"` or `false`. The flat layout fails to load,
  and `brnr doctor` says which key goes where (ADR 33).
- **Breaking:** acting on an editor's session is experimental. `send`,
  `cancel`, `mode`, `model`, `config` and the rest are refused there until
  named in the editor part's `experimental = [...]`; strict mode refuses
  them all (ADR 4).
- **Breaking:** `send` holds a message while a turn runs (ADR 18).
- **Breaking:** `turn_ended` has `messages` instead of `message`, and a
  dropped message is a `message_dropped` event instead of `exited`'s
  `undelivered` (ADR 17, 20). `usage` and `tool_progress` are left out by
  default, and `session_changed` carries a merge patch of what changed
  (ADR 23).
- **Breaking:** a session has two transcripts: `<id>.jsonl` its events,
  `<id>.acp.jsonl` the raw ACP (ADR 22).
- **Breaking:** `brnr model` sets the config option of category `model`;
  `session/set_model` is no longer sent. A boolean config option takes
  `true` or `false` and is sent as a boolean (ADR 28).
- **Breaking:** `brnr host` isn't run by hand: a process is started with one
  resolved request on its stdin (ADR 8). As a profile bridge, `notify` is
  `command = ["brnr", "notify", "--stdin", "--", …]` (ADR 36).
- **Breaking:** text output shows control characters and bidi overrides in
  the agent's text escaped; `--json` keeps the text as sent. `notify`'s
  `BRNR_TEXT`, `BRNR_TITLE` and `BRNR_MESSAGE` are escaped and capped at
  32 KiB (ADR 1, P8; ADR 36).
- **Breaking:** `wait` on a session that is already idle exits as its last
  turn ended: 1 if it failed or was stopped (ADR 21). `start --foreground`
  exits 1 when the start fails (ADR 9), and `notify` exits 1 when it is cut
  off (ADR 36).
- **Breaking:** `approve --option` refuses a reject option, and
  `deny --option` an allow one (ADR 27).
- **Breaking:** a start's `--mode`, `--model` and `--set` win over its
  profile, whatever the option's id. Two values for one setting
  (`--set a=1 --set a=2`, or `--model` and `--set` of the model option) fail
  the start instead of the last one winning (ADR 58).
- **Breaking:** transcript and lock file names escape every byte of a
  session id but lowercase ASCII letters, digits, `-` and `_` (`a/b` is
  `a%2fb`). Existing files aren't renamed; UUID ids, which the Claude Code
  and Codex adapters use, keep their names (ADR 53).
- **Breaking:** a headless start, or `brnr sessions`, fails against an agent
  that answers `initialize` with an ACP version other than 1, or none
  (ADR 54).
- **Breaking:** a line over 32 MiB from the agent isn't interpreted:
  headless it is dropped, and an editor gets it byte for byte (ADR 51).
- **Breaking:** between `brnr acp` and its process, the process sends `EOF`
  once the agent's stdout ends, and a `STDERR` frame for each read of the
  agent's stderr rather than for each line. Run both from the same version,
  as `brnr acp` starts them (ADR 62).
- **Breaking:** the host log records the agent's stderr as lines alongside
  forwarding it: one `agent-stderr` record per line, or per 64 KiB of a
  longer one, with a line still unfinished recorded when stderr ends or the
  process exits. For when bytes reached the editor, read its stderr, not the
  host log (ADR 62).
- **Breaking:** the host log has two new notes: `agent-stdout-ended`, and
  `not-sent-to-editor` for a line of brnr's own (a sent message's tool call,
  a refusal, a withdrawal) not written because the editor's stdout had
  already ended (ADR 62).
- **Breaking:** `brnr log` reports on stderr each transcript line it can't
  read, and a transcript that ends partway through a record, where it used
  to skip them silently. It still exits 0 (ADR 55).
- **Breaking:** a turn's `turn_ended` waits until the agent has answered
  every steer sent into it, so it can come after the prompt's answer. A
  steer the agent never answers holds it until `cancel` or `close` drops
  the steer (ADR 56).
- **Breaking:** events from a load's replay that brnr records carry
  `"replayed": true`, and text shows them as `(replayed) …`. A replayed
  `user_message` has no `by` and no message id (ADR 57).
- `brnr start` commits once its ready report is written: interrupted before
  that, the process stops and the prompt is never sent (ADR 7).
- Backpressure replaces the 30 s timeout on the editor link and the agent's
  stdin; signals reach the agent on a link of their own (ADR 2, 6).
- One long message doesn't cut off a reader that keeps up, also one that
  asked for `acp`. A reader that stopped reading is cut off soon after
  16 MiB, or 64 MiB of lines longer than that (ADR 49, 60).
- `log` of a running session waits, up to 5 s, for its process to write
  what it has recorded, and an exiting process stays listed until its
  transcript has its `exited` (ADR 48).
- Past 64 MiB waiting for a slow disk, the logger skips records and the
  transcript says `records-skipped` (ADR 22).
- `close` cancels a running turn first (ADR 16).
- ACP is read with `agent-client-protocol-schema`'s types, and every RFC 8259
  line is read, lone surrogates and deep nesting too; the bytes are still
  forwarded untouched (ADR 26, 43).
- A GitHub release's notes are its section of this file, not the titles of
  the pull requests it merged (ADR 40).

### Removed

- **Breaking:** `send --after-turn`, the `queued` status, and the bridge
  modes `now` and `after-turn`: a message sent during a turn is held until
  it ends, or use `--steer` (ADR 18).

### Fixed

- An editor that stops reading ends its agent and the agent's process group,
  as the editor going away does (ADR 2).
- An agent that crashes has its process group's children stopped too, not
  only on a requested stop (ADR 11).
- Distinct session ids never share a transcript or a lock (ADR 53).
- A process closes a session's transcript files when it closes the session,
  so one that forks and closes sessions doesn't run out of descriptors
  (ADR 22).
- A `session/load` replay keeps the session's title, mode, config and
  commands (ADR 14).
- `start --foreground` reports a failed setup step, and isn't stopped by
  `stty tostop` (ADR 9).
- `log --last 0`, and timeouts too large to count, no longer panic;
  `brnr list | head` ends quietly; `brnr sessions` kills an agent that
  ignores SIGTERM.
- A bridge's request line that isn't UTF-8 gets an error reply, instead of
  the bridge being dropped (ADR 35).
- A session whose transcript ends partway through a record, as a process
  killed mid-write leaves it, is still listed, logged and resumed, from
  its last whole record. A process that reopens it starts its first record
  on a new line, and a line that isn't UTF-8 no longer fails `log`
  (ADR 55).
- A steered message always ends, in its turn's `turn_ended` or in a
  `message_dropped`, whichever order the agent answers in: `send --steer
  --wait` no longer times out when the agent acknowledges the steer after
  the turn's answer (ADR 56).
- `start --resume` of a session brnr has no transcript of, through
  `session/load`, records the history the agent replays. A transcript made
  by an earlier load keeps its gap unless it is deleted (ADR 57).

### Security

- Every request to the agent has an id of brnr's, the editor's too, and its
  answer goes only to whoever sent it: an editor's request whose id was one
  brnr gave its own (`brnr-1`) can no longer end the editor's turn with the
  answer to `brnr mode`, nor a `send`'s turn with the editor's answer, and
  the editor's `$/cancel_request` reaches only its own requests. The editor
  gets its ids back as it wrote them (GHSA-84pw-hh9c-w2m8; ADR 61).
- The agent's text is escaped wherever brnr shows it, so a permission
  request can't disguise its command with control characters, and terminal
  sequences (OSC 52, titles) don't reach the terminal; `show` warns about a
  command with control characters (ADR 1, P8).
- Through `brnr acp`, the editor's stdout ends when the agent closes its
  stdout, even while the agent runs on, and the agent's stderr reaches the
  editor as it is written, without waiting for a newline, as it would from
  the agent run directly. A failed start's error includes the stderr line
  the agent is still writing, such as a login prompt (GHSA-4q62-fhcc-rgf2;
  ADR 62).
- Every command refuses a runtime directory other users can access, as the
  process already did, and ignores metadata whose socket isn't next to it
  (ADR 1, P13).
- MCP servers' `env` and `headers` values are redacted in transcripts and
  everywhere brnr shows them (ADR 25).
- A transcript or host log others can read, write or search, such as one
  restored from a backup, copied in or chmod'ed, is made private before
  anything more is written to it, its directories up to the state
  directory too. A `made-private` record in the host log names each path
  and the mode it had. Before, brnr only created transcripts private and
  went on appending to one that others could read (GHSA-3j4p-vjm3-pgv9;
  ADR 59, ADR 1 P13).

## [0.6.0] - 2026-10-07

### Changed

- **Breaking:** headless, every permission request waits for `brnr approve`
  or `deny`, or a bridge; `permission_timeout` still denies what nobody
  answers. How much the agent asks is its mode (`brnr start --mode`,
  `mode =` in the profile, `brnr mode`) (ADR 27).
- **Breaking:** `brnr list` and `brnr sessions` share one table: SESSION,
  TITLE, STATE, PID, (AGENT,) LAST ACTIVE, CWD, most recently active first.
  `sessions --json`'s `updated` and `brnr` are now `last_active`, `state`
  and `pid` (ADR 15).
- **Breaking:** the editor sees an injected message as a completed tool call
  ("Message via brnr", "Context via brnr"), not a `user_message_chunk`,
  which editors don't show out of turn. Events, bridges and transcripts
  still have `user_message`, `by: control` (ADR 5).

### Removed

- **Breaking:** the approval policy: `--permissions`, the `permissions`
  profile key and the rules by tool kind. A config that still has
  `permissions` fails to load, and `brnr doctor` says so; delete the key
  (ADR 27).
- **Breaking:** `brnr ps`'s CWD column; `--json` keeps the process's `cwd`
  (ADR 15).

## [0.5.0] - 2026-10-06

### Added

- `brnr ps` lists brnr's processes; `watch` and `notify` take a session or
  `--pid <pid>` (ADR 13).
- `start --foreground [--quiet]` runs a session in the foreground, for
  supervisors such as systemd (ADR 9).
- `--json` on every command that prints data, with the same fields as the
  text (ADR 34).
- A usage error prints only that command's usage.

### Changed

- **Breaking:** commands take a session, the id the agent gave it, or a
  process's pid, and brnr no longer picks a session or a request for you;
  `show`, `approve` and `deny` take `<session> <request>` (ADR 13).
- **Breaking:** `brnr stop <pid>` stops a process; `close` ends a session
  (ADR 13, 16).
- **Breaking:** the editor going away stops its agent, as if the editor had
  run it directly; `start --resume` carries the session on headless (ADR 2,
  14).
- **Breaking:** `--stop-when-idle <s>` and `stop_when_idle` take seconds,
  counted from the start; `0` closes the session as soon as it is idle
  (ADR 12).
- **Breaking:** `brnr sessions [--profile <p>] [--cwd <dir>] [-- <agent>]`
  starts the agent to ask it for its sessions, instead of taking a target
  (ADR 15).
- **Breaking:** `list --json` and `status --json` have new shapes: `list` is
  an index, with each session's title, and `status` the detail (ADR 15, 34).
- **Breaking:** bridges name a session by its exact id, and `BRNR_PID`
  replaces `BRNR_HOST` and `BRNR_HOST_ID`. For `notify`, `BRNR_SESSION_ID`
  replaces `BRNR_SESSION`, and `BRNR_PID` `BRNR_HOST` (ADR 35, 36).

### Removed

- **Breaking:** `<target>`, host ids, names, prefixes and `--session`: pass
  a session id or a pid (ADR 13).
- **Breaking:** `on_disconnect`, `brnr acp --name` and `--on-disconnect`,
  the `owner_changed` event and the bridges' `sessions` request (ADR 2, 35).
- **Breaking:** `brnr host` from the usage: use `start --foreground` (ADR 9).

## [0.4.0] - 2026-10-06

### Added

- `--events` takes event names, `default` and `all`, in `log`, `watch` and
  `notify`; all three refuse an unknown name before connecting (ADR 23).
- `start --resume` takes an id brnr has no transcript of and passes it to the
  agent, so what `brnr sessions` lists can be resumed; `sessions` has a BRNR
  column, running or inactive (ADR 14, 15).

### Changed

- **Breaking:** `watch` and `log` leave out `acp` and `agent_thought` by
  default, in `--json` too (ADR 23).
- **Breaking:** `log` shows its transcript as the events `watch` shows live,
  not the stored lines; the file is unchanged, and `brnr list --json` gives
  its path (ADR 22).
- `owner_changed` is in every session's transcript, so `log` shows the
  process taking over from the editor (ADR 22).
- The help is grouped: chat, approvals, sessions, events, settings, brnr.
- An adapter formula's `revision` is dropped when its npm version moves
  (ADR 39).

### Removed

- **Breaking:** `--raw` and `--thoughts` from `watch` and `log`: use
  `--events all` or `--events default,acp`, and `--events
  default,agent_thought` (ADR 23).

## [0.3.0] - 2026-10-05

### Added

- Homebrew formulae for the adapters, compiled on your machine, each
  versioned by its npm package: `brew install
  brnrhq/tap/brnr-claude-adapter` and `brnr-codex-adapter` (ADR 37, 39).
- An adapter's `--version` says what it was built from, `brnr doctor` shows
  each adapter's version, and `brnr status` the agent's name and version
  (ADR 39).
- `brnr acp --help`.

### Changed

- **Breaking:** `brnr proxy` is `brnr acp`, with no alias: editor configs
  need `brnr acp -- <agent>` (ADR 2).
- **Breaking:** the compiled adapters are `brnr-claude-adapter` and
  `brnr-codex-adapter`; `claude-agent-acp` and `codex-acp` now mean the npm
  packages (ADR 38).

### Fixed

- brnr finds the adapters next to a symlinked brnr, as Homebrew installs it
  on Linux (ADR 38).
- A reader is behind once its unwritten backlog is past 16 MiB, so one long
  message doesn't cut off a reader that is keeping up; every reader gets
  `exited` before the process ends (ADR 6).

## [0.2.0] - 2026-10-02

### Added

- `brnr doctor [--fix]` checks the runtime directory, the transcripts'
  permissions, the config and each profile, the adapters and the running
  processes; `--fix` tightens permissions and removes stale files.
- `brnr log` of a running or finished session, and a readable `watch`
  (ADR 22, 23, 24).
- `brnr wait --for idle|turn|permission|exit`, `send --wait` and
  `start --wait`, which print the reply and exit 0 for `end_turn`, 1
  otherwise and 124 on `--timeout` (ADR 21).
- `brnr start --resume <session>` (ADR 14), `brnr cancel`, which drops the
  held messages and lists them (ADR 19), `brnr queue`, and `brnr show`, a
  request's command, paths and diff (ADR 27).
- `brnr status` summarises a session; `brnr mode`, `model`, `config` and
  `commands`, and `start --mode`, `--model` and `--set` (ADR 28).
- `--stop-when-idle`, and `stop_when_idle` in a profile (ADR 12).
- `--file` and `--image` for `send` and `start` (ADR 32), and MCP servers
  for headless sessions (ADR 31).
- `brnr sessions`, `fork` and `close` (ADR 15, 16).
- A start that needs a login fails with the agent's auth methods (ADR 30).
- `brnr notify -- <command>`: a command run for each event, the event in its
  environment and on its stdin (ADR 36).
- Events for tool calls, the plan, usage, session changes and thoughts
  (ADR 23), and message ids that `--wait` matches replies to (ADR 17).
- Approval rules by ACP tool kind, and `permission_timeout` (ADR 27).

### Fixed

- An interrupted or timed-out `brnr start` stops its process without sending
  the prompt (ADR 7).
- An agent that stops reading its stdin doesn't freeze the process (ADR 6).
- `brnr stop` stops the agent's children too: its whole process group
  (ADR 13).
- A reader that falls behind is disconnected instead of growing the
  process's memory without limit; the limit is 16 MiB, not lines, so one that
  keeps up through a burst stays (ADR 6).
- A duplicate `--name` is refused, held messages are reported on exit,
  interrupts keep their order, and an empty `--prompt` is refused.

### Security

- Transcripts are private: directories 0700, files 0600 (`brnr doctor --fix`
  tightens existing ones), and the runtime directory check doesn't follow
  symlinks (ADR 1, P13).
- `--prompt` goes to the process on its stdin, not its command line, where
  `ps` showed it (ADR 8).

## [0.1.0] - 2026-10-02

### Added

- `brnr proxy -- <agent>`, what an editor runs as its agent: the agent lives
  in a process of its own, which the editor going away can't take down
  (ADR 2).
- `brnr start` and `brnr host`, headless sessions, where brnr is the agent's
  client (ADR 2, 29).
- Commands that reach a running agent over a Unix socket only you can open:
  `list`, `status`, `send` (`--after-turn`, `--interrupt`, `--context`),
  `watch`, `pending`, `approve`, `deny` and `stop`.
- Profiles in `~/.config/brnr/config.toml` (ADR 33), and bridges (ADR 35).
- Transcripts in `~/.brnr/projects/<folder>/`, one per session, under the
  same folder and name as Claude Code's own (ADR 22).
- `adapters/`, which builds the Claude Code and Codex ACP adapters as
  single-file executables (ADR 37).
- `brew install brnrhq/tap/brnr`, building from source, and `brnr --version`.

[Unreleased]: https://github.com/brnrhq/brnr/compare/v0.9.0...HEAD
[0.9.0]: https://github.com/brnrhq/brnr/compare/v0.8.0...v0.9.0
[0.8.0]: https://github.com/brnrhq/brnr/compare/v0.7.0...v0.8.0
[0.7.0]: https://github.com/brnrhq/brnr/compare/v0.6.0...v0.7.0
[0.6.0]: https://github.com/brnrhq/brnr/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/brnrhq/brnr/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/brnrhq/brnr/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/brnrhq/brnr/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/brnrhq/brnr/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/brnrhq/brnr/releases/tag/v0.1.0
