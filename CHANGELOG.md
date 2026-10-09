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
- Security names a fixed vulnerability by its advisory (`GHSA-…`) and CVE
  ids where it has them.
- An entry cites the ADR it follows. 0.6.0 and earlier came before
  `docs/adr`, so they cite the record that now holds the decision.

## [Unreleased]

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
  `brnr --help` has a section per group, and `brnr <group> --help` lists a
  group's commands (ADR 63).
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
  A setting the agent has no option for fails ("the agent offers no thought
  level"), as do two values for one setting, and a v1 mode the agent doesn't
  list fails before anything is sent, a start's too (ADR 63).
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
  it. A SOURCE column after AGENT (`source` in `--json`) says who knows each,
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

[Unreleased]: https://github.com/brnrhq/brnr/compare/v0.7.0...HEAD
[0.7.0]: https://github.com/brnrhq/brnr/compare/v0.6.0...v0.7.0
[0.6.0]: https://github.com/brnrhq/brnr/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/brnrhq/brnr/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/brnrhq/brnr/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/brnrhq/brnr/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/brnrhq/brnr/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/brnrhq/brnr/releases/tag/v0.1.0
