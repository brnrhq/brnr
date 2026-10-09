# Changelog

What changed in each release of brnr, for the people who use it. A
release's section is its GitHub release's notes (ADR 40).

- Each release is `## X.Y.Z - YYYY-MM-DD`, newest first, under
  `## Unreleased`, which is what's on main since the last release. A pull
  request that changes what users see adds its line to Unreleased;
  `release.sh` names the section for the version it releases.
- Under it, the headings that apply, in this order: **Breaking** (before 1.0
  nothing is kept for compatibility, ADR 1's P9: a change is made outright,
  and says here what to do instead), **Added**, **Changed**, **Fixed**, and
  **Security**: a fixed vulnerability, named by its advisory (`GHSA-…`) and
  CVE id where it has them, with what it allowed and who it affected.
- An entry says what is now true, in a sentence, with the ADR it follows.
  0.6.0 and earlier came before `docs/adr`; their entries cite the record
  that now holds the decision.

## Unreleased

### Breaking

- Profiles have parts: `[profiles.<p>.headless]` has `cwd`, `mode`,
  `config`, `mcp_servers`, `permission_timeout`, `stop_when_idle` and
  `auth`; `[profiles.<p>.editor]` has `experimental` and `features`. `log` is
  `"all"`, `"events"` or `false`. The flat layout fails to load, and
  `brnr doctor` says which key goes where (ADR 33).
- Acting on an editor's session is experimental: `send`, `cancel`, `mode`,
  `model`, `config` and the rest are refused there until named in the editor
  part's `experimental = [...]`. Strict mode refuses them all (ADR 4).
- `send` holds a message while a turn runs. `--after-turn`, the `queued`
  status and the bridge modes `now` and `after-turn` are gone; `--steer`
  sends into the running turn, where the agent can be steered (ADR 18).
- `turn_ended` has `messages` instead of `message`, and `exited` no longer
  has `undelivered`: a dropped message is a `message_dropped` event
  (ADR 17, 20). `usage` and `tool_progress` are left out by default, and
  `session_changed` carries a merge patch of what changed (ADR 23).
- A session has two transcripts: `<id>.jsonl` its events, `<id>.acp.jsonl`
  the raw ACP (ADR 22).
- `brnr model` sets the config option of category `model`;
  `session/set_model` is no longer sent (ADR 28). A boolean config option
  takes `true` or `false` and is sent as a boolean (ADR 28).
- `brnr host` isn't run by hand: a process is started with one resolved
  request on its stdin (ADR 8). As a profile bridge, `notify` is
  `command = ["brnr", "notify", "--stdin", "--", …]` (ADR 36).
- Text output shows control characters and bidi overrides in the agent's
  text escaped; `--json` keeps the text as sent. `notify`'s `BRNR_TEXT`,
  `BRNR_TITLE` and `BRNR_MESSAGE` are escaped and capped at 32 KiB (ADR 1,
  P8; ADR 36).
- `wait` on a session that is already idle exits as its last turn ended: 1
  if it failed or was stopped (ADR 21). `start --foreground` exits 1 when the
  start fails (ADR 9), and `notify` exits 1 when it is cut off (ADR 36).
- `approve --option` refuses a reject option, and `deny --option` an allow
  one (ADR 27).
- A start's `--mode`, `--model` and `--set` win over its profile, whatever
  the option's id; two values for one setting (`--set a=1 --set a=2`, or
  `--model` and `--set` of the model option) fail the start instead of the
  last one winning (ADR 58).
- Transcript and lock file names escape every byte of a session id but
  lowercase ASCII letters, digits, `-` and `_` (`a/b` is `a%2fb`). Existing
  files aren't renamed; UUID ids, which the Claude Code and Codex adapters
  use, keep their names (ADR 53).
- A headless start, or `brnr sessions`, fails against an agent that answers
  `initialize` with an ACP version other than 1, or none (ADR 54).
- A line over 32 MiB from the agent isn't interpreted: headless it is
  dropped, and an editor gets it byte for byte (ADR 51).

### Added

- `brnr start --auth <method>`, `--strict` (and `brnr acp --strict`), and
  `--resume <session> --take-over` (ADR 30, 41, 3).
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
- [docs/interface.md](docs/interface.md), the reference for commands, JSON,
  exit statuses, configuration and the bridge protocol, and
  [docs/threat-model.md](docs/threat-model.md).

### Changed

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

### Security

- The agent's text is escaped wherever brnr shows it, so a permission
  request can't disguise its command with control characters, and terminal
  sequences (OSC 52, titles) don't reach the terminal; `show` warns about a
  command with control characters (ADR 1, P8).
- Every command refuses a runtime directory other users can access, as the
  process already did, and ignores metadata whose socket isn't next to it
  (ADR 1, P13).
- MCP servers' `env` and `headers` values are redacted in transcripts and
  everywhere brnr shows them (ADR 25).

## 0.6.0 - 2026-10-07

### Breaking

- The approval policy is gone: no `--permissions`, no `permissions` profile
  key, no rules by tool kind. A config that still has `permissions` fails to
  load, and `brnr doctor` says so; delete the key. Headless, every request
  waits for `brnr approve` or `deny`, or a bridge, and `permission_timeout`
  still denies what nobody answers. How much the agent asks is its mode
  (`brnr start --mode`, `mode =` in the profile, `brnr mode`) (ADR 27).
- `brnr list` and `brnr sessions` share one table: SESSION, TITLE, STATE,
  PID, (AGENT,) LAST ACTIVE, CWD, most recently active first.
  `sessions --json`'s `updated` and `brnr` are now `last_active`, `state` and
  `pid` (ADR 15).
- `brnr ps` has no CWD column; `--json` keeps the process's `cwd` (ADR 15).
- The editor sees an injected message as a completed tool call ("Message via
  brnr", "Context via brnr"), not a `user_message_chunk`, which editors don't
  show out of turn. Events, bridges and transcripts still have
  `user_message`, `by: control` (ADR 5).

## 0.5.0 - 2026-10-06

### Breaking

- Commands take a session, the id the agent gave it, or a process's pid.
  `<target>`, host ids, names, prefixes and `--session` are gone, and brnr no
  longer picks a session or a request for you; `show`, `approve` and `deny`
  take `<session> <request>` (ADR 13).
- `brnr stop <pid>` stops a process; `close` ends a session (ADR 13, 16).
- The editor going away stops its agent, as if the editor had run it
  directly; `start --resume` carries the session on headless.
  `on_disconnect`, `brnr acp --name` and `--on-disconnect`, and the
  `owner_changed` event are gone (ADR 2, 14).
- `--stop-when-idle <s>` and `stop_when_idle` take seconds, counted from the
  start; `0` closes the session as soon as it is idle (ADR 12).
- `brnr host` is out of the usage: `start --foreground [--quiet]` runs a
  session in the foreground, for supervisors such as systemd (ADR 9).
- `brnr sessions [--profile <p>] [--cwd <dir>] [-- <agent>]` starts the agent
  to ask it for its sessions, instead of taking a target (ADR 15).
- `list --json` and `status --json` have new shapes: `list` is an index,
  with each session's title, and `status` the detail (ADR 15, 34).
- Bridges name a session by its exact id; the `sessions` request is gone, and
  `BRNR_PID` replaces `BRNR_HOST` and `BRNR_HOST_ID`. For `notify`,
  `BRNR_SESSION_ID` replaces `BRNR_SESSION`, and `BRNR_PID` `BRNR_HOST`
  (ADR 35, 36).

### Added

- `brnr ps` lists brnr's processes; `watch` and `notify` take a session or
  `--pid <pid>` (ADR 13).
- `--json` on every command that prints data, with the same fields as the
  text (ADR 34).
- A usage error prints only that command's usage.

## 0.4.0 - 2026-10-06

### Breaking

- `watch` and `log` have no `--raw` (use `--events all`, or
  `--events default,acp`) and no `--thoughts` (`--events
  default,agent_thought`), and leave out `acp` and `agent_thought` by
  default, in `--json` too (ADR 23).
- `log` shows its transcript as the events `watch` shows live, not the
  stored lines; the file is unchanged, and `brnr list --json` gives its path
  (ADR 22).

### Added

- `--events` takes event names, `default` and `all`, in `log`, `watch` and
  `notify`; all three refuse an unknown name before connecting (ADR 23).
- `start --resume` takes an id brnr has no transcript of and passes it to the
  agent, so what `brnr sessions` lists can be resumed; `sessions` has a BRNR
  column, running or inactive (ADR 14, 15).

### Changed

- `owner_changed` is in every session's transcript, so `log` shows the
  process taking over from the editor (ADR 22).
- The help is grouped: chat, approvals, sessions, events, settings, brnr.
- An adapter formula's `revision` is dropped when its npm version moves
  (ADR 39).

## 0.3.0 - 2026-10-05

### Breaking

- `brnr proxy` is `brnr acp`, with no alias: editor configs need
  `brnr acp -- <agent>` (ADR 2).
- The compiled adapters are `brnr-claude-adapter` and `brnr-codex-adapter`;
  `claude-agent-acp` and `codex-acp` now mean the npm packages (ADR 38).

### Added

- Homebrew formulae for the adapters, compiled on your machine, each
  versioned by its npm package: `brew install
  brnrhq/tap/brnr-claude-adapter` and `brnr-codex-adapter` (ADR 37, 39).
- An adapter's `--version` says what it was built from, `brnr doctor` shows
  each adapter's version, and `brnr status` the agent's name and version
  (ADR 39).
- `brnr acp --help`.

### Fixed

- brnr finds the adapters next to a symlinked brnr, as Homebrew installs it
  on Linux (ADR 38).
- A reader is behind once its unwritten backlog is past 16 MiB, so one long
  message doesn't cut off a reader that is keeping up; every reader gets
  `exited` before the process ends (ADR 6).

## 0.2.0 - 2026-10-02

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

## 0.1.0 - 2026-10-02

The first release.

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
