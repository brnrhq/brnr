# brnr

[![License](https://img.shields.io/github/license/brnrhq/brnr)](LICENSE)
![Platforms](https://img.shields.io/badge/platforms-macOS%20%7C%20Linux-lightgrey)
[![Rust](https://img.shields.io/badge/dynamic/toml?url=https%3A%2F%2Fraw.githubusercontent.com%2Fbrnrhq%2Fbrnr%2Fmain%2FCargo.toml&query=%24.package%5B%27rust-version%27%5D&label=rust&suffix=%2B)](CONTRIBUTING.md#building-and-testing)
[![CI](https://github.com/brnrhq/brnr/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/brnrhq/brnr/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/brnrhq/brnr)](https://github.com/brnrhq/brnr/releases/latest)
[![Homebrew](https://img.shields.io/badge/brew-brnrhq%2Ftap%2Fbrnr-orange)](#install)
[![crates.io](https://img.shields.io/crates/v/brnr)](https://crates.io/crates/brnr)

[![OpenSSF Scorecard](https://api.scorecard.dev/projects/github.com/brnrhq/brnr/badge)](https://scorecard.dev/viewer/?uri=github.com/brnrhq/brnr)
[![OpenSSF Best Practices](https://www.bestpractices.dev/projects/15315/badge)](https://www.bestpractices.dev/projects/15315)


A burner phone for your coding agents.

brnr sits between your editor and an [ACP](https://agentclientprotocol.com/)
agent (Claude Code, Codex, …). The editor talks to the agent exactly as before,
but the agent now lives in a process of its own that you can also reach from
outside the editor: watch everything its sessions do, and carry them on once
the editor has gone. Or run sessions with no editor at all, and send them
messages and answer their approvals from somewhere else. (Doing that to an
editor's session is [experimental](#experimental-actions).)

```text
editor ──stdio── brnr acp ──socketpairs── brnr process ──pipes── agent
                                             │
                                  control socket ── brnr prompt send / event watch / …
                                             │
                                          bridges (Slack, push, …)
```

- **brnr acp** is what the editor runs as its agent. It only relays bytes.
- **The brnr process** owns the agent for its whole life, out of the editor's
  process group and process tree. When the editor goes, the agent goes, as if
  the editor had run it. `brnr process list` lists these processes.
- **The other commands** (`brnr prompt send`, `brnr event watch`, …) talk to
  a session through its process, over a Unix socket that only you can open.
  Most are a group, what they act on, and a verb: `brnr <group> --help` lists
  a group's ([ADR 63](docs/adr/0063-commands-are-grouped-by-what-they-act-on.md)).

macOS and Linux (Unix sockets only). Why brnr does what it does, and what else
was considered, is in [docs/adr](docs/adr/README.md).

brnr never phones home: it makes no network requests of its own and sends
no telemetry ([ADR 44](docs/adr/0044-brnr-never-phones-home.md)). Who can
reach a session, and what keeps others out, is in the
[threat model](docs/threat-model.md).

## Interface reference

The [external interface reference](docs/interface.md) describes command
syntax, inputs and outputs, JSON results, exit codes, configuration, and
the bridge/control-socket protocol. Use it when writing scripts or bridges;
the sections below provide examples and explain the workflows.

## Install

```sh
brew install brnrhq/tap/brnr
brew install brnrhq/tap/brnr-claude-adapter   # for Claude Code, compiled on your machine
brew install brnrhq/tap/brnr-codex-adapter    # for Codex
```

The adapter formulae compile the community's ACP adapters for Claude Code and
Codex into standalone executables that need no Node.js ([Adapters](#adapters)
says whose they are), and link them next to `brnr`, where brnr finds them
even when an editor's `PATH` doesn't include Homebrew. Each is versioned by
the npm package it builds, so `brew upgrade` rebuilds an adapter when that
package moves. `brnr doctor` shows which version each adapter was built from.

Or brnr alone, from [crates.io](https://crates.io/crates/brnr), with the
adapters from Homebrew or npm:

```sh
cargo install brnr --locked
```

Or from source, below.

The formulae build from the source tarball on each
[release](https://github.com/brnrhq/brnr/releases), `brnr-X.tar.gz`, which
the release workflow makes from the tag and attests. To check one you
downloaded was made that way:

```sh
gh attestation verify brnr-0.7.0.tar.gz -R brnrhq/brnr
```

Each release also carries `brnr-X.cdx.json`, an attested CycloneDX SBOM of
brnr's Rust dependencies across all targets (not the adapters' npm packages).
CI checks that two clean release builds, in different directories and at
different times, have the same SHA-256 on each of Linux and macOS. This
checks reproducibility within one toolchain and runner, not across Rust or
OS versions. After publishing and updating the tap, a fresh macOS runner
installs the formula, checks its version against the tag, and runs
`brnr doctor` ([ADR 52](docs/adr/0052-reproducible-builds-and-sbom.md)).

More at
[brnrhq.github.io/brnr](https://brnrhq.github.io/brnr/).

## Build

```sh
cargo build --release          # target/release/brnr
adapters/build.sh              # optional: target/release/brnr-claude-adapter, brnr-codex-adapter (needs bun)
```

`brnr` is one binary. The adapters are separate, single-file builds of the ACP
adapters for Claude Code and Codex; see [Adapters](#adapters).

## Use it from an editor

Configure your editor's ACP agent command as:

```sh
brnr acp -- brnr-claude-adapter  # or claude-agent-acp, from npm
brnr acp -- brnr-codex-adapter   # or codex-acp, from npm
brnr acp --profile work          # agent and settings from a profile
```

A bare agent name is looked up next to `brnr` first, so the editor's `PATH`
doesn't need to include it; so is a [bridge](#bridges)'s, such as `brnr` in
`brnr event notify`.

`brnr acp` passes bytes, signals (HUP, INT, QUIT, TERM, USR1, USR2), the
agent's stderr, stdin's EOF, its stdout closing and the exit status through
unchanged. It changes the stream in four ways: every request reaches the
agent with an id of brnr's, and its answer comes back to the editor with the
editor's own id, so that the editor's requests and brnr's are never confused
(ADR 61); and, each below, the editor's `fs` and `terminal` capabilities are
dropped, a session another brnr process serves can't be loaded, and the
experimental actions a profile enables act on the editor's session. When the editor goes away, so does the agent;
`brnr start --resume <session>` carries on with one of its sessions headless
(a turn still running is lost).

Watching an editor's session (`event watch`, `event log`, `session status`,
`permission requests`, `permission show`, `event notify`, bridges) always
works. Acting on it doesn't, by default: ACP has one
client per session, the editor, and every action from outside leaves the
editor's view behind the agent's in some way.

### Experimental actions

Each is refused on an editor's session unless the editor's part of its profile
enables it by name (without `--profile`, the profile is `default`):

```toml
[profiles.work.editor]
experimental = ["send", "approve"]
```

| Action | Commands | What brnr does for the editor |
|---|---|---|
| `send` | `prompt send` | Sent only while no turn runs: the editor controls its turns, so nothing is held, steered or interrupts. Shown as a completed tool call, "Message via brnr". |
| `context` | `prompt send --context`, `queue list --clear-context` | Added to the editor's next prompt, shown as "Context via brnr". |
| `cancel` | `prompt cancel` | The agent's pending approvals are answered `cancelled`, and withdrawn from the editor (`$/cancel_request`). |
| `approve` | `approve`, `deny` | The request is withdrawn from the editor, its tool call set `in_progress` or `failed`, and the editor told at once who answered ("Approved via brnr", "Denied via brnr"). If the editor answers anyway, its answer is dropped and it is told who answered first. |
| `config` | `config set` | The editor is sent the `current_mode_update` or `config_option_update` the agent sends only to whoever asked. |
| `close` | `session close`, `start --resume --take-over` | The turn is cancelled, the editor is told ("Session closed via brnr", "Session taken over by brnr (process 4466)"), and its later requests for the session get an error saying where it continues. |

`session fork` is never available in an editor's process, and in strict mode
none of these is, whatever the profile enables. `brnr process stop` isn't an
action on a session: it stops an editor's process as it does any other.

### What the editor doesn't see

- The `fs` and `terminal` client capabilities are dropped from the editor's
  `initialize`: ACP v2 drops them and they add nothing an agent needs, so no
  agent comes to rely on them. The agent can't use the editor's buffers or
  terminal. Strict mode passes them through.
- The editor's child is `brnr acp`, not the agent, so its pid and process tree
  differ.
- A SIGKILL sent to `brnr acp` can't be passed on. brnr sees it go and kills
  the agent's process group, as whenever the editor goes.
- Loading or resuming a session another brnr process serves is refused, with a
  JSON-RPC error naming the process and how to release it
  (`brnr session close <session>`, or closing it in the other editor that has
  it open): the editor can't pass `--take-over`. `shared_sessions` lets it
  through ([feature flags](#strict-mode-and-feature-flags)).
- A message sent from outside reaches the agent as an ordinary user message,
  but the editor is shown a completed tool call, the one update editors render
  anywhere in a turn; brnr's notes to it take the same form. A turn
  `prompt send` starts is one the editor didn't start.
- The editor's acknowledgement of a `$/cancel_request` (an error) is dropped
  without telling it.
- The agent's stdout and stderr reach the editor as they would directly: stdout
  ends when the agent closes it, even if the agent runs on, and stderr comes as
  it is written, newline or not. From when stdout ends, brnr's own notes to the
  editor (a message sent from outside, a refusal) aren't shown; the host log
  says which ([ADR 62](docs/adr/0062-stdout-ends-and-stderr-comes-as-written.md)).
- Both streams come through one connection, in order, with 16 MiB in flight
  rather than a pipe's 64 KiB: an editor that stops reading one of them holds
  up the other too, once that much is waiting. The editor's stderr is also
  `brnr acp`'s own, for its errors, so it ends when `brnr acp` exits rather
  than when the agent closes its stderr, and an editor closing it doesn't give
  the agent EPIPE.
- None of the compensations above has been tried against real editors yet
  (Zed, IntelliJ's AIR plugin): how each shows them is still to be seen.

## Talk to it from outside

```sh
brnr list                          # running sessions; --all: ended ones too (--inactive: only those)
brnr session status $s             # what it's doing: turn, tools, plan, usage, last message
brnr prompt send $s "also update the changelog"                # held while a turn runs
brnr prompt send $s --steer "and the tests too"                # into the running turn
brnr prompt send $s --interrupt "stop, wrong branch"
brnr prompt send $s --context "the API key is in .env.local"   # added to the next prompt; --replace: instead of the last
brnr prompt send $s --wait "what did you change?"              # prints the reply
brnr prompt send $s --file src/api.rs --image screenshot.png "why does this look wrong?"
brnr prompt cancel $s              # stop the running turn (held messages are dropped and listed)
brnr queue list $s                 # held messages and context; --clear, and brnr queue drop $s m3
brnr event watch $s                # live: messages, tools, plan, approvals (--events all: everything)
brnr event log $s                  # the story so far; --last 2, --follow, and event watch's flags
```

A `<session>` is the session's id, as the agent gave it: `brnr list` shows
them, with the agent's title for each, and `start --json` prints the new one,
with its process's `pid` and the prompt's id as `message`
(`s=$(brnr start --json … | jq -r .session)`). `event log` works on a session
that has ended too; commands that need it running say so when it isn't. On an
editor's session, those that act on it are [experimental](#experimental-actions).

brnr never sends a second prompt while a turn runs: ACP doesn't say what one
means, and agents differ. `prompt send` holds the message and sends it as a
turn of its own when the running one ends, in order. `--steer` puts it into
the running turn instead (`_session/steering`, refused if the agent doesn't
offer it), and `--interrupt` cancels the turn and sends the message ahead of
what is held. On an idle session each is sent at once. `prompt send` says
which: `delivered`, `held`, `steered` or `interrupting`. Every message gets an
id (`m<n>`), which `prompt send --wait` uses to find the turn that answers it.
A turn that ends before the agent has answered the steers into it has its
`turn_ended` wait for those answers, so it lists every message the agent took
into it.

A message that never goes out is a `message_dropped` event, saying why:
`prompt cancel` (unless `--keep-held`), `queue drop` or `queue list --clear`,
its session closing, the agent exiting, or a steer the agent didn't take.
Context that never joins a prompt is a `context_dropped` event the same way:
`queue list --clear-context`, its session closing, or the agent exiting
(`prompt cancel` keeps it for the next prompt).

Every command that prints data takes `--json`, with the same data as its
text: one JSON value, or one event per line for `event log`, `event watch`
and `start --foreground`.

### Waiting, for scripts

```sh
brnr event wait $s                 # until no turn is running and nothing is held
brnr event wait $s --for permission   # until an approval is waiting
brnr event wait $s --for turn      # the next turn's end; --for exit: its process's
brnr start --wait --stop-when-idle 0 --prompt "fix the failing tests" -- brnr-claude-adapter > answer.md
```

`event wait`, `prompt send --wait` and `start --wait` exit 0 when the turn
ended normally (`end_turn`); 1 when it failed or stopped for another reason,
when its message was dropped (`brnr: m3 was dropped (cancel)`), when the agent
exited first, or when the session closed before the turn or approval waited
for; and 124 on `--timeout <s>`. `event wait` on a session that is idle
already, or that closes while it waits, exits as its last turn ended. While
`prompt send --wait` and `start --wait` wait, approvals are announced on
stderr; `start --wait` says `started …` there too, so stdout is the reply and
nothing else. With `--json`, the turn is one object at its end: `session`,
`message`, `reply`, `stop_reason`, `error`, `dropped` (and `pid`, for
`start`).

### Approvals

```sh
brnr permission requests           # approvals waiting, in every session
brnr permission show $s p1         # one in full: the command, paths, the diff
brnr approve $s p1                 # or: brnr deny $s p1, --option <id>
```

Whatever brnr shows of the agent's text has its control characters escaped
(`\u001b`), so a command can't be dressed up as another; `permission show`
says when a command has any. `--json` has the text as the agent sent it. On an
editor's session `permission show` says the request is waiting in the editor,
and that it can be answered here too, experimentally, where `approve` is
enabled, or else why it can't (`answerable` and `why_not` in `--json`).

### Settings

```sh
brnr config get $s [--model]       # the options, their values and choices, and the modes
brnr config set $s --mode plan     # the option of category mode (or the agent's modes)
brnr config set $s --model opus    # the option of category model
brnr config set $s --option effort=high   # any of the agent's config options, by id
brnr prompt commands $s            # the agent's slash commands (send them as text)
```

`--mode`, `--model` and `--thought-level` find the option of that category
(`mode`, `model`, `thought_level`), whatever its id, and `--option <o>=<v>` an
option by id; they go together, and `config get` takes the same, without
values, to list only those options; it lists only options the agent
advertised, so one it doesn't have fails. A mode, model or thought level the
agent has no option for fails ("the agent offers no thought level") before
anything is sent, as two values for one setting do
(`--model large --option llm=small`, when `llm` is the model); each is sent
once, the mode first. An `--option` id the agent hasn't advertised is sent
too, for the agent to take or refuse; when it refuses, the error says what
was already set. A mode is the agent's own `session/set_mode` only for an
agent with modes and no mode option. `config get` lists each option's choices
under it, the current one marked `*`, with their names and descriptions, and
has the agent's modes as a row with no option (`-`, `null` in `--json`).

A boolean option (one an editor's session has when the editor advertises
boolean config options) takes `true` or `false`, and is sent as a boolean;
anything else for it fails before it reaches the agent.

### Sessions and processes

```sh
brnr session fork $s               # a copy of the session, in the same process
brnr session close $s              # cancel a running turn, then close the session
brnr sessions -- brnr-claude-adapter   # the agent's own list for this folder (--cwd), and what brnr knows of each
brnr start --resume <id> -- brnr-claude-adapter    # any of them, even one brnr never saw
brnr start --resume $s --take-over # one another process serves: closed there, resumed here
brnr process list                  # brnr's processes: pid, owner, agent, sessions
brnr process stop 4466             # stdin closed, then SIGTERM, then SIGKILL, 5 s apart
```

A process holds a lock for each session it serves
(`$BRNR_DIR/sessions/<id>.lock`), so which process has a session is known
without asking it. A process that doesn't answer (within 5 seconds) still has
its sessions: `list` and `process list` show them `unreachable`, commands on
them say the process isn't answering, and `start --resume` refuses them,
naming the process, as it does any session that is running. A process that is
gone, even one whose pid another process has now (nobody listens on its
socket), isn't shown or counted as running, and its metadata and socket are
removed. `--take-over` asks the owner to close the session (an editor's
process does only if its profile enables `close`) and resumes it in a new
headless process; sessions beside it keep running.

A session whose lock can't be taken (`sessions/` isn't private, or a directory
is where its lock file goes) isn't served headless: `start` fails with the
cause and the process stops before the agent gets any prompt, and a
`session fork` into it is refused. An editor's process serves it all the same,
as the editor would be served without brnr; `session status` says it isn't
locked, and `doctor` what is in the way.

`session close` ends one session; a headless process with none left stops.
`session fork` is refused in an editor's process, in strict mode, when the
agent can't fork, and with `stop_when_idle` when the agent can't close
sessions.

`brnr sessions` starts the agent just to ask it (`session/list`), so it
includes sessions started outside brnr and needs nothing running; its STATE
and PID columns say, as `brnr list` would, which are running and in which
process (`unreachable` for one whose process holds it but doesn't answer),
which brnr has a transcript of (`inactive`), and which only the agent knows
(`-`). `--resume` takes an id brnr knows, or any id the agent knows, which it
resumes in `--cwd` (or here) with the agent after `--` (or the profile's).
`brnr process stop` signals the agent's whole process group, so whatever the
agent started in that group goes with it. The same cleanup runs when the agent
exits or crashes on its own. A SIGKILL of the brnr host itself bypasses
cleanup: agents and bridges that ignore their closed pipes, or their
descendants, can survive it; `doctor` reports the unrecorded death.

### Events

`event watch`, `event log`, `event notify`, the foreground and bridges all
show the same events, live or read back from the transcript:

| Event | |
|---|---|
| `user_message` | a message sent to the agent, `by` the `editor` or `control`, with its id |
| `agent_message`, `agent_thought` | the agent's text, whole |
| `tool_call` | a tool call starting (`started`) and ending (`completed`, `failed`) |
| `tool_progress` | a tool call's status changing in between (`in_progress`) |
| `plan`, `usage` | the plan; the context window and cost, as the agent reports them |
| `session_changed` | the title, mode, config options or commands: what changed |
| `permission_request`, `permission_resolved` | an approval waiting; its answer, and who gave it |
| `turn_ended` | its `stop_reason` or `error`, and the `messages` it carried |
| `message_dropped` | a message that never went, `by` `cancel`, `queue`, `close`, `exit` or `steer` |
| `context_dropped` | context that never joined a prompt, `by` `queue`, `close` or `exit` |
| `session_closed` | `by` `close` (`brnr session close`, `--take-over`), `idle` or `editor` |
| `history` | a load's replay: how many `updates`, and whether they were `recorded` (as events with `replayed`) |
| `line_too_long` | a line `from` the `agent` or `editor` past the `limit` (32 MiB), unread: `relayed` to the other side with an editor, dropped headless |
| `exited` | the agent exited: its `status`, and a `reason` when brnr itself crashed |
| `acp` | every ACP message, with its direction |

Each names its session; `exited` and `line_too_long` are the process's, and
are in every session's transcript. For config options and commands, `session_changed` is a JSON merge
patch by id or name: `{"model": "opus"}`, a command added, `null` for one gone.

`--events` chooses, for `event watch`, `event log` and `event notify` alike:
names, `all`, and `default` (`--events default,agent_thought` adds thoughts).
Without it, `event watch`, `event log` and the foreground leave out the quiet
ones, `acp`, `agent_thought`, `usage` and `tool_progress`, in text and JSON
alike. Bridges get every event but `acp` unless they choose. Watching a
session ends when it closes.

### Notifications

`brnr event notify` runs a command for each event of a session, or of every
session in a process (`--pid`): by default `permission_request`, `turn_ended`
and `exited`; `--events` as for `event watch`, where `default` means these
three. The event is in its environment (`BRNR_EVENT`, `BRNR_TEXT`,
`BRNR_TITLE`, `BRNR_MESSAGE`, `BRNR_SESSION_ID`, `BRNR_REQUEST`, `BRNR_PID`)
and, as JSON, on its stdin; nothing is put on its command line, so what the
agent writes can't become arguments. The text, title and message are escaped
as `event watch` shows them and cut at 32 KiB; the event on stdin is whole.
Commands run one at a time, in order, each in a process group of its own.
`event notify` exits when the process does (or the session closes). Cut off
first, as one that falls behind is, it stops the command it is running, says
on stderr that no more notifications come, and exits non-zero.

```sh
brnr event notify $s -- sh -c 'curl -s -d "$BRNR_TEXT" ntfy.sh/my-agents'
```

In a profile it is a bridge, for every process the profile starts, and reads
the events the process writes on its stdin:

```toml
[[profiles.work.bridges]]
command = ["brnr", "event", "notify", "--stdin", "--",
           "sh", "-c", "terminal-notifier -title \"$BRNR_TITLE\" -message \"$BRNR_TEXT\""]
```

If the bridge's `events` limit what it gets, they must include
`agent_message`, `session_changed` and `exited`, which `event notify` needs
for its environment and to know when to stop. What it says on stderr, that it
was cut off included, is in the host log (`bridge-stderr`).

## Headless sessions

```sh
brnr start --cwd ~/work/project --prompt "fix the failing tests" -- brnr-claude-adapter
brnr start --mode plan --model opus --prompt - < task.md       # set up before the first prompt
brnr start --set effort=high --prompt - < task.md              # any of the agent's config options, likewise
brnr start --resume $s                                         # carry on a session that ended
brnr start --auth api-key --prompt - -- brnr-codex-adapter < task.md   # log in first (OPENAI_API_KEY)
brnr start --foreground --prompt - -- brnr-codex-adapter < task.md     # in the foreground; Ctrl-C stops it
```

A start is atomic. `start` reads the prompt and attachments first, hands the
process everything in one request, and the start commits once the session is
open with its mode and config options set; only then does the agent get the
prompt. Until then, a step failing, the timeout (120 seconds,
`BRNR_START_TIMEOUT`) or `start` being interrupted stops the process, and the
prompt is never sent. A start that fails ends its error with the agent's last
lines on stderr (`claude CLI not found`).

`--mode`, `--model` and `--set` win over the profile's `mode` and `config`,
setting by setting. The model is the agent's config option of category
`model`, whatever its id, so `--model large` replaces a profile's
`config = { model = "small" }`; the mode likewise. Two different values for
one setting from the flags (`--model large --set model=small`), or from the
profile, fail the start before anything is set.

`--stop-when-idle <s>` closes a session once it has been idle that many
seconds (no turn running, nothing held, no approval waiting), counting from
the start; `0` is as soon as it is. Its last session isn't closed: the
process stops instead.

`--foreground` keeps the session in the terminal, for a supervisor such as
systemd or a container. It shows the session's events on stdout (`--json`: as
JSON lines; `--quiet`: not) and brnr's messages and the agent's own stderr on
stderr, and exits as the agent did, non-zero if the start failed. Ctrl-C stops
the session (twice kills the agent). A display that can't keep up skips
events, saying how many (`… 120 events not shown`); stdout closing
(`| head -1`) ends the display, not the session.

`--resume` uses the agent's `session/resume` (or `session/load`), in the
session's cwd, with the agent and profile it last had, and appends to the
same transcript. A load replays the session's history: brnr records it, as
events marked `replayed`, only when it has no transcript of the session (one
it has never seen, or whose transcript is gone), so repeated loads don't
record it twice. A `history` event says how many updates were replayed and
whether they were recorded. brnr's transcript has only what happened through
brnr: turns taken in the agent's own client, or with `log = false`, aren't
in it, and a later load doesn't add them.

With no editor attached brnr is the agent's client: approvals wait for
`brnr approve`/`deny` or a bridge (`permission_timeout` denies what nobody
answers), elicitation is declined, and anything else is answered with
"method not found". How much the agent asks is the agent's own setting: its
mode (`--mode`, `brnr config set --mode`). brnr doesn't log in for you. If the agent needs
a login, the start fails with the agent's methods: log in with the agent's own
CLI first, or name a method that needs no terminal with `--auth <id>` (`auth`
in the profile), such as codex-acp's `api-key`.

## For agents

An agent can run other agents through brnr: start headless workers, send
them work, wait for their turns, read their logs and bring their approvals
to you. brnr ships a skill that teaches it how
([skills/brnr](skills/brnr/SKILL.md), [ADR 46](docs/adr/0046-a-skill-for-agents-that-use-brnr.md)):
exact ids, waiting rather than polling, never approving what you didn't
delegate, and cleaning up what it starts.

```sh
brnr skill                         # SKILL.md; brnr skill orchestrate, approvals, observe, setup: its references
brnr skill install                 # into ~/.claude/skills/brnr (Claude Code) and ~/.agents/skills/brnr (Codex)
brnr skill install --dir .claude/skills   # or a project's, for each --dir
```

The skill is built into the binary, so it is always the one for the brnr
you run; install it again after upgrading. Every command in it runs as a
test, as the README's do.

## Strict mode and feature flags

By default brnr speaks stable ACP plus the conventions agents and editors
already implement alike, ahead of the spec: `_session/steering` (for
`prompt send --steer`), `session/fork`, unstable in ACP v1 (for
`brnr session fork`), and dropping the editor's `fs` and `terminal`
capabilities, as ACP v2 does. Strict mode (`strict = true` in a profile,
`--strict` on `acp` and `start`) is stable ACP to the letter: no steering, no
fork, the editor's capabilities passed through, and no experimental actions.
What it refuses says why.

Either way, brnr speaks ACP version 1. A `brnr start` (or `brnr sessions`)
whose agent answers `initialize` with another version, or one brnr can't
read, fails with that, before anything else is sent, and the agent is
stopped ([ADR 54](docs/adr/0054-acp-version-1.md)). An editor's process
passes the editor's `initialize` and the agent's answer through: the version
they agree on is theirs.

How brnr's processes share sessions isn't protocol, and strict mode doesn't
change it: it has defaults that protect you, and feature flags in a profile's
editor part to change them. The one so far is `shared_sessions`: the editor
may load a session another brnr process serves. The agent then serves it to
both and the conversation splits; the process holding the session's lock still
writes its transcript, and `session status` says the session is shared.

## Profiles

`~/.config/brnr/config.toml`:

```toml
[profiles.work]                     # every process of the profile
agent = ["brnr-claude-adapter"]
log = "all"                         # "events": no raw ACP; false: no transcripts
strict = false                      # stable ACP only

[[profiles.work.bridges]]
command = ["~/bin/slack-bridge", "--channel", "#agents"]
events = ["permission_request", "turn_ended"]   # omit for every event but acp

[profiles.work.headless]            # brnr start
cwd = "~/work/project"
mode = "plan"                       # the agent's mode and config options,
config = { model = "opus" }         #   set before the first prompt
permission_timeout = 600            # deny what nobody answered in 10 minutes
stop_when_idle = 600                # close a session idle 10 minutes
# auth = "api-key"                  # a login method to run first (codex-acp's)

[[profiles.work.headless.mcp_servers]]   # for sessions brnr opens; an editor brings its own
name = "github"
command = "github-mcp-server"       # or url = "https://…", type = "http" | "sse", headers
args = ["stdio"]
env = { GITHUB_TOKEN = "…" }

[profiles.work.editor]              # brnr acp
experimental = ["send", "approve"]  # actions on the editor's session
features = ["shared_sessions"]      # process management
```

Without `--profile`, the `default` profile applies, if there is one. A key in
the wrong part, an unknown key and an unknown name fail to load, saying where
the key goes; `brnr doctor` checks every profile.

## Bridges

A bridge is any process that speaks brnr's JSON-lines protocol: one started by
the brnr process from the profile (requests on its stdout, events on its
stdin, its stderr in the host log; a bare command found next to `brnr` first,
as an agent's is), or anything that connects to the control socket. Its
environment has `BRNR_PID` and `BRNR_SOCKET`.

Requests: `status`, `logged`, `send`, `cancel`, `queue`, `subscribe`,
`pending`, `approve`, `deny`, `set_config`, `fork`, `close`, `stop` (see the [request and response reference](docs/interface.md#bridge-and-control-socket-protocol)); those about a session name it by
its exact id, and those that act on an editor's session are
[experimental](#experimental-actions), as the CLI's are. Events:
[as above](#events). A started bridge gets them from the process's start.

A started bridge lasts until its process exits; its stdout closing only means
it has no more requests. When the process stops, the bridge gets the last
events (`exited`), then its stdin closes; one still running 2 seconds later
gets SIGTERM.

## When a reader falls behind

Nothing is buffered without limit, and who gives way depends on who is
reading. The session's own pipes get backpressure, as a direct pipe would give
them: an editor that stops reading holds the agent back on its stdout for as
long as it does, with no timeout of brnr's own (and observers see nothing new
meanwhile), and an agent that stops reading its stdin holds back the editor's
writes. A signal to `brnr acp`, or its stdout closing, doesn't wait behind
them: it goes to the brnr process on a link of its own, and reaches the agent
at once. Observers never slow the session: a bridge or watcher 16 MiB behind
is cut off (a connection closed, a started bridge sent SIGTERM), not counting
the one line, however long, that took it past 16 MiB, nor (up to 64 MiB of
them) lines longer than 16 MiB that came after it, so a reply bigger than
that reaches every peer that keeps up, and the
foreground's display skips events. Nor does brnr's own record: past 64 MiB
waiting for a slow disk, records are skipped, and
[the transcript](#transcripts) says so.

However long the lines: the session's pipes hold back past 16 MiB queued,
and brnr reads a line of up to 32 MiB, held beside that while it waits for
its newline. A longer one goes on as it comes,
unread and unrecorded, to the editor or the agent unchanged, or is dropped
headless, and a `line_too_long` event says so. The agent's stderr
goes on as it is written, newline or not, 64 KiB at most at a time, every
byte.

## Transcripts

Like the agents' own transcripts, keyed by project folder:

```text
~/.brnr/projects/<folder>/<session id>.jsonl       # a session's events
~/.brnr/projects/<folder>/<session id>.acp.jsonl   # its raw ACP
~/.brnr/hosts/<run id>.jsonl                       # each process's, for what belongs to no session
```

`<folder>` is the session's cwd with every non-alphanumeric character replaced
by `-`, as in `~/.claude/projects`, so a claude-agent-acp session's events
file has the same folder and name as Claude Code's own transcript. In
`<session id>`, as in a lock's name, every byte but lowercase ASCII letters,
digits, `-` and `_` is `%` and two hex digits (`a/b` is `a%2fb`), so two
sessions never share a file. The events are the ones bridges get, `exited`
included, and are what `brnr event log`, `list --all` and `--resume` read;
`event log` reads the raw file too when `acp` events are chosen. A thread of
the process's own writes them, so the session never waits for the disk;
`event log` of a running session first waits until its process has written
what it recorded until then (up to 5 s, then it says so and shows what is
there), so a turn `start --wait` just reported is in it, and a process that
exits is listed until its transcript has its `exited` (2 s at most). A process
killed (SIGKILL) loses what it hadn't written yet, and may leave its last
record cut short: the session is still listed, logged and resumed as of its
last whole record, `event log` says on stderr which lines it couldn't read,
and the next process starts on a new line; brnr never rewrites a transcript.
Every record carries `host_id`, `host_pid`, `proxy_pid` and `agent_pid` for
joining. `log = "events"` leaves out the raw file, most of the space;
`log = false` writes nothing.

Records skipped behind a slow disk are counted, and a `records-skipped` record
(`count`, `acp` of them raw ACP, `since`, `until`) marks the gap in the host
log and in each session's events file that lost some. Records a full disk
didn't take are noted likewise, with the `error`, in the file that lost them
(a raw file's in its events file) once it takes records again. How a process
ended (`exited`, or a panic) is never skipped, so `brnr doctor` can tell an
exit from a death. `brnr event log` always shows a gap, whatever `--events`
chose.

The values of MCP servers' `env` and `headers` are recorded as
`"<redacted>"`, in the raw ACP, the host log and `acp` events alike; the agent
gets them as given.

Transcripts hold prompts and tool output, so brnr writes them only where only
you can read them (ADR 59): `~/.brnr` (or `$BRNR_HOME`), each directory below
it and the file must be yours, with no group or other access, before a record
goes in, whether brnr made them or found them there. It creates them 0700 and
0600; one restored, copied or chmod'ed so that others can reach it is made
private when a process opens it, and the host log has a `made-private`
record (`path`, and the `mode` it had). Symlinks are never followed, the
state directory included (set `BRNR_HOME` to where one points instead), and
what isn't your own directory or file, or has another hard link, is refused:
a process whose host log can't be opened doesn't start (`log: <path>: …`),
and a session whose file can't be is served without it, with a
`session-log-failed` record in the host log saying why. There is no
override; `log = false` writes nothing.

## Adapters

The ACP adapters aren't brnr's work. `adapters/` packages two existing
open-source adapters, unchanged, as standalone executables
(`bun build --compile`). Each carries its own JavaScript runtime, so it needs
no Node.js (or bun) installed, unlike the npm packages:

| Command | Builds | By | License |
|---|---|---|---|
| `brnr-claude-adapter` | [claude-agent-acp](https://github.com/agentclientprotocol/claude-agent-acp) | Zed Industries, Inc. and contributors | Apache-2.0; it includes Anthropic's [Claude Agent SDK](https://github.com/anthropics/claude-agent-sdk-typescript), under Anthropic's [Commercial Terms](https://www.anthropic.com/legal/commercial-terms) |
| `brnr-codex-adapter` | [codex-acp](https://github.com/agentclientprotocol/codex-acp) | JetBrains s.r.o. | Apache-2.0 |

brnr adds only a few lines in front of each (`adapters/claude.ts`,
`adapters/codex.ts`, `adapters/find.ts`): finding the user's own agent, and
`--version`. The commands have names of their own so they don't clash with
the npm packages' (`claude-agent-acp`, `codex-acp`) when both are installed.

They don't include the agents. Each runs the user's own `claude` or `codex`
(from `PATH` or the usual install locations, or `CLAUDE_CODE_EXECUTABLE` /
`CODEX_PATH`). `build.sh` copies every package's license file (`LICENSE`,
`license`, `License.txt`, …), nested packages' too, to `licenses/`. The Claude
Agent SDK's license isn't an open-source one; check Anthropic's terms before
redistributing a build of brnr-claude-adapter.

## Doctor

```sh
brnr doctor                        # checks what brnr depends on
brnr doctor --fix                  # … and repairs what it safely can
brnr doctor --report               # a bug report to read, then paste into an issue
```

It checks that the runtime directory is private, isn't a symlink, is short
enough for socket paths, and holds nothing left by processes that are gone
(their metadata, sockets and session locks), nor anything that keeps a
session from being locked; that transcripts are readable
only by you; that the config parses, with every key in its part, and each
profile's agent, cwd, MCP servers and bridges are valid; where the adapters
are found; and that every running process answers, naming the sessions one
that doesn't still serves. A process is gone when nobody listens on its
socket, even if its pid now belongs to another process (after a reboot, say);
one that is stopped is running but not answering. macOS also refuses a
connection to a stopped process whose socket's backlog is full, so there a
refusal counts only if the process with that pid started after brnr's file
was written.

It also lists the host logs (`~/.brnr/hosts/`) of processes that died
without recording it, killed by SIGKILL for instance: logs without `exited`
whose process isn't running. The line says how many, and for the latest three
when each last wrote and the sessions it had open; it is information, not a
warning. `--fix` makes the runtime directory and transcripts private (only
what you own, never through a symlink) and removes what processes that are
gone left behind; it leaves logs alone. It exits non-zero if a check fails;
`--json` prints the checks, each a `level`, `check` and `message`.

`--report` prints, instead of the checks, what a bug report needs, as
Markdown to paste into an issue: brnr's version, the OS, the adapters, the
checks that aren't ok, and the last 20 lines of the latest host log and of
the latest one of a process that panicked or died without recording it.
Secrets are redacted as brnr records them, and your home directory is `~`;
the log lines are otherwise as recorded, prompts included, so read it before
you paste it. With `--json` it is one object (`brnr`, `os`, `adapters`,
`checks`, `host_logs`) with the same data. Nothing is sent anywhere.

When brnr panics, it prints a link to a new issue with the version, the OS
and the panic's message filled in; nothing is sent unless you open it and
submit the form. The link is printed on the command's stderr; for a process
that runs detached, by `brnr start` or `brnr acp` when its start fails with
the panic, and in the foreground by the process itself.

## Environment

| Variable | Default |
|---|---|
| `BRNR_HOME` | `~/.brnr` (transcripts) |
| `BRNR_CONFIG` | `$XDG_CONFIG_HOME/brnr/config.toml`, else `~/.config/brnr/config.toml` |
| `BRNR_DIR` | `$XDG_RUNTIME_DIR/brnr`, else `$TMPDIR/brnr-<uid>` (sockets, metadata and session locks; brnr refuses one others can use) |
| `BRNR_START_TIMEOUT` | `120`: seconds a start has to commit |

## Releasing

```sh
./release.sh minor     # or patch, major, 1.2.3: checks main, opens the release pull request
./release.sh tag       # once it's merged: tags, releases, updates the Homebrew tap
./release.sh notes     # what the release pull request would say
```

A release's notes are its section of [CHANGELOG.md](CHANGELOG.md), in
[Keep a Changelog 1.1.0](https://keepachangelog.com/en/1.1.0/) form, which
pull requests add to under `[Unreleased]`. `./release.sh minor` names that
section for the version on the release branch and moves its compare links,
`./release.sh tag` won't tag a version without one, and the release workflow
publishes it as the GitHub release's notes, or fails (ADR 40).

Dependabot opens a pull request when an adapter's npm package has a new
release; the next brnr release ships it.

## Contributing

[CONTRIBUTING.md](CONTRIBUTING.md) says how to build, test and propose a
change, including the [requirements for acceptable contributions](CONTRIBUTING.md#requirements-for-acceptable-contributions)
and [coding standards](CONTRIBUTING.md#coding-standards).
brnr has five direct dependencies; CI checks their licenses and
advisories with cargo deny, and with cargo vet that every crate is audited
(by Mozilla, Google and others whose audits brnr imports) or listed as an
exemption still to audit. The [audit status](supply-chain/README.md) records
the remaining work; an exemption is not an audit. Report vulnerabilities privately, as
[SECURITY.md](SECURITY.md) says.

## License

Apache-2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE).
