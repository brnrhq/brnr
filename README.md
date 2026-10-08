# brnr

A burner phone for your coding agents.

brnr sits between your editor and an [ACP](https://agentclientprotocol.com/)
agent (Claude Code, Codex, …). The editor talks to the agent exactly as before,
but the agent now lives in a process of its own that you can also reach from
outside the editor: watch everything its sessions do, and carry them on once
the editor has gone. Or run sessions with no editor at all, and send them
messages and answer their approvals from somewhere else. (Doing that to an
editor's session is [experimental](#experimental-actions).)

```text
editor ──stdio── brnr acp ──socketpair── brnr process ──pipes── agent
                                            │
                                 control socket ── brnr send / watch / …
                                            │
                                         bridges (Slack, push, …)
```

- **brnr acp** is what the editor runs as its agent. It only relays bytes.
- **The brnr process** owns the agent for its whole life, out of the editor's
  process group and process tree. When the editor goes, the agent goes, as if
  the editor had run it. `brnr ps` lists these processes.
- **The other commands** (`brnr send`, `brnr watch`, …) talk to a session
  through its process, over a Unix socket that only you can open.

macOS and Linux (Unix sockets only). Why brnr does what it does, and what else
was considered, is in [docs/adr](docs/adr/README.md).

## Install

```sh
brew install brnrhq/tap/brnr
brew install brnrhq/tap/brnr-claude-adapter   # for Claude Code, compiled on your machine
brew install brnrhq/tap/brnr-codex-adapter    # for Codex
```

The adapter formulae compile the community's ACP adapters for Claude Code and
Codex into standalone executables that need no Node.js ([Adapters](#adapters)
says whose they are), and link them next to `brnr`, where brnr finds them even when an
editor's `PATH` doesn't include Homebrew. Each is versioned by the npm package
it builds, so `brew upgrade` rebuilds an adapter when that package moves.
`brnr doctor` shows which version each adapter was built from. Or from source, below. More at [brnrhq.github.io/brnr](https://brnrhq.github.io/brnr/).

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
doesn't need to include it.

`brnr acp` passes bytes, signals, the agent's stderr, stdin's EOF and the exit
status through unchanged. It changes the stream in three ways, each below: the
editor's `fs` and `terminal` capabilities are dropped, a session another brnr
process serves can't be loaded, and the experimental actions a profile enables
act on the editor's session. When the editor goes away, so does the agent;
`brnr start --resume <session>` carries on with one of its sessions headless
(a turn still running is lost).

Watching an editor's session (`watch`, `log`, `status`, `pending`, `show`,
`notify`, bridges) always works. Acting on it doesn't, by default: ACP has one
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
| `send` | `send` | Sent only while no turn runs: the editor controls its turns, so nothing is held, steered or interrupts. Shown as a completed tool call, "Message via brnr". |
| `context` | `send --context`, `queue --clear-context` | Added to the editor's next prompt, shown as "Context via brnr". |
| `cancel` | `cancel` | The agent's pending approvals are answered `cancelled`, and withdrawn from the editor (`$/cancel_request`). |
| `approve` | `approve`, `deny` | The request is withdrawn from the editor, its tool call set `in_progress` or `failed`, and the editor told who approved or denied it ("Approved via brnr"). If the editor answers anyway, its answer is dropped and it is told who answered first. |
| `settings` | `mode`, `model`, `config` | The editor is sent the `current_mode_update` or `config_option_update` the agent sends only to whoever asked. |
| `close` | `close`, `start --resume --take-over` | The turn is cancelled, the editor is told ("Session closed via brnr", "Session taken over by brnr (process 4466)"), and its later requests for the session get an error saying where it continues. |

`fork` is never available in an editor's process, and in strict mode none of
these is, whatever the profile enables.

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
  JSON-RPC error naming the process and `brnr close <session>`: the editor
  can't pass `--take-over`. `shared_sessions` lets it through
  ([feature flags](#strict-mode-and-feature-flags)).
- A message sent from outside reaches the agent as an ordinary user message,
  but the editor is shown a completed tool call, the one update editors render
  anywhere in a turn; brnr's notes to it take the same form. A turn `send`
  starts is one the editor didn't start.
- The editor's acknowledgement of a `$/cancel_request` (an error) is dropped
  without telling it.
- None of the compensations above has been tried against real editors yet
  (Zed, IntelliJ's AIR plugin): how each shows them is still to be seen.

## Talk to it from outside

```sh
brnr list                          # running sessions; --all, --inactive: ended ones too
brnr status $s                     # what it's doing: turn, tools, plan, usage, last message
brnr send $s "also update the changelog"                # held while a turn runs
brnr send $s --steer "and the tests too"                # into the running turn
brnr send $s --interrupt "stop, wrong branch"
brnr send $s --context "the API key is in .env.local"   # added to the next prompt
brnr send $s --wait "what did you change?"              # prints the reply
brnr send $s --file src/api.rs --image screenshot.png "why does this look wrong?"
brnr cancel $s                     # stop the running turn (held messages are dropped and listed)
brnr queue $s                      # held messages and context; --drop m3, --clear
brnr watch $s                      # live: messages, tools, plan, approvals (--events all: everything)
brnr log $s                        # the story so far; --last 2, --follow, and watch's flags
```

A `<session>` is the session's id, as the agent gave it: `brnr list` shows
them, with the agent's title for each, and `start --json` prints the new one
(`s=$(brnr start --json … | jq -r .session)`). `log` works on a session that
has ended too; commands that need it running say so when it isn't. On an
editor's session, those that act on it are [experimental](#experimental-actions).

brnr never sends a second prompt while a turn runs: ACP doesn't say what one
means, and agents differ. `send` holds the message and sends it as a turn of
its own when the running one ends, in order. `--steer` puts it into the
running turn instead (`_session/steering`, refused if the agent doesn't offer
it), and `--interrupt` cancels the turn and sends the message ahead of what is
held. On an idle session each is sent at once. `send` says which: `delivered`,
`held`, `steered` or `interrupting`. Every message gets an id (`m<n>`), which
`send --wait` uses to find the turn that answers it.

A message that never goes out is a `message_dropped` event, saying why:
`cancel` (unless `--keep-held`), `queue --drop` or `--clear`, its session
closing, the agent exiting, or a steer the agent didn't take.

Every command that prints something takes `--json`, with the same data as its
text: one JSON value, or one event per line for `log`, `watch` and
`start --foreground`.

### Waiting, for scripts

```sh
brnr wait $s                       # until no turn is running and nothing is held
brnr wait $s --for permission      # until an approval is waiting
brnr wait $s --for turn            # the next turn's end; --for exit: its process's
brnr start --wait --stop-when-idle 0 --prompt "fix the failing tests" -- brnr-claude-adapter > answer.md
```

`wait`, `send --wait` and `start --wait` exit 0 when the turn ended normally
(`end_turn`); 1 when it failed or stopped for another reason, when its message
was dropped (`brnr: m3 was dropped (cancel)`), or when the session closed
before the turn or approval waited for; and 124 on `--timeout <s>`. `wait` on a
session that is idle already, or has closed, exits as its last turn ended.
While they wait, approvals are announced on stderr; `start --wait` says
`started …` there too, so stdout is the reply and nothing else. With `--json`,
the turn is one object at its end: `session`, `message`, `reply`,
`stop_reason`, `error`, `dropped`.

### Approvals

```sh
brnr pending                       # approvals waiting, in every session
brnr show $s p1                    # one in full: the command, paths, the diff
brnr approve $s p1                 # or: brnr deny $s p1, --option <id>
```

Whatever brnr shows of the agent's text has its control characters escaped
(`\u001b`), so a command can't be dressed up as another; `show` says when a
command has any. `--json` has the text as the agent sent it. On an editor's
session `show` says where the request can be answered: in the editor, and
here only where `approve` is enabled (`answerable` and `why_not` in
`--json`).

### Settings

```sh
brnr mode $s [plan]                # list or set the session's mode
brnr model $s [<model>]            # list or set the model (the config option of category model)
brnr config $s [effort=high]       # any of the agent's config options
brnr commands $s                   # the agent's slash commands (send them as text)
```

### Sessions and processes

```sh
brnr fork $s                       # a copy of the session, in the same process
brnr close $s                      # cancel a running turn, then close the session
brnr sessions -- brnr-claude-adapter   # the agent's own list for this folder (--cwd), and what brnr knows of each
brnr start --resume <id> -- brnr-claude-adapter    # any of them, even one brnr never saw
brnr start --resume $s --take-over # one another process serves: closed there, resumed here
brnr ps                            # brnr's processes: pid, owner, agent, sessions
brnr stop 4466                     # stdin closed, then SIGTERM, then SIGKILL
```

A process holds a lock for each session it serves
(`$BRNR_DIR/sessions/<id>.lock`), so which process has a session is known
without asking it. A process that doesn't answer still has its sessions:
`list` and `ps` show them `unreachable`, commands on them say the process isn't
answering, and `start --resume` refuses them, naming the process, as it does
any session that is running. `--take-over` asks the owner to close the session
(an editor's process does only if its profile enables `close`) and resumes it
in a new headless process; sessions beside it keep running.

`close` ends one session; a headless process with none left stops. `fork` is
refused in an editor's process, in strict mode, when the agent can't fork, and
with `stop_when_idle` when the agent can't close sessions.

`brnr sessions` starts the agent just to ask it (`session/list`), so it
includes sessions started outside brnr and needs nothing running; its STATE
and PID columns say, as `brnr list` would, which are running and in which
process, which brnr has a transcript of (`inactive`), and which only the
agent knows (`-`). `--resume` takes an id brnr knows, or any id the
agent knows, which it resumes in `--cwd` (or here) with the agent after
`--` (or the profile's). `brnr stop` signals the agent's whole process group,
so whatever the agent started goes with it.

### Events

`watch`, `log`, `notify`, the foreground and bridges all show the same events,
live or read back from the transcript:

| Event | |
|---|---|
| `user_message` | a message sent to the agent, `by` the `editor` or `control`, with its id |
| `agent_message`, `agent_thought` | the agent's text, whole |
| `tool_call` | a tool call starting (`started`) and ending (`completed`, `failed`) |
| `tool_progress` | a tool call's status changing in between (`in_progress`) |
| `plan`, `usage` | the plan; the context window and cost, once a turn |
| `session_changed` | the title, mode, config options or commands: what changed |
| `permission_request`, `permission_resolved` | an approval waiting; its answer, and who gave it |
| `turn_ended` | its `stop_reason` or `error`, and the `messages` it carried |
| `message_dropped` | a message that never went, `by` `cancel`, `queue`, `close`, `exit` or `steer` |
| `session_closed` | `by` `close` (`brnr close`, `--take-over`), `idle` or `editor` |
| `exited` | the agent exited: its `status`, and a `reason` when brnr itself crashed |
| `acp` | every ACP message, with its direction |

Each names its session; `exited` is the process's, and is in every session's
transcript. For config options and commands, `session_changed` is a JSON merge
patch by id or name: `{"model": "opus"}`, a command added, `null` for one gone.

`--events` chooses, for `watch`, `log` and `notify` alike: names, `all`, and
`default` (`--events default,agent_thought` adds thoughts). Without it, `watch`,
`log` and the foreground leave out the quiet ones, `acp`, `agent_thought`,
`usage` and `tool_progress`, in text and JSON alike. Bridges get every event
but `acp` unless they choose. Watching a session ends when it closes.

### Notifications

`brnr notify` runs a command for each event of a session, or of every session
in a process (`--pid`): by default `permission_request`, `turn_ended` and
`exited`; `--events` as for `watch`, where `default` means these three. The
event is in its environment (`BRNR_EVENT`, `BRNR_TEXT`, `BRNR_TITLE`,
`BRNR_MESSAGE`, `BRNR_SESSION_ID`, `BRNR_REQUEST`, `BRNR_PID`) and, as JSON, on
its stdin; nothing is put on its command line, so what the agent writes can't
become arguments. The text, title and message are escaped as `watch` shows
them and cut at 32 KiB; the event on stdin is whole. Commands run one at a
time, in order. `notify` exits when the process does (or the session closes),
and fails if the process cuts it off first, as it does one that falls behind.

```sh
brnr notify $s -- sh -c 'curl -s -d "$BRNR_TEXT" ntfy.sh/my-agents'
```

In a profile it is a bridge, for every process the profile starts, and reads
the events the process writes on its stdin:

```toml
[[profiles.work.bridges]]
command = ["brnr", "notify", "--stdin", "--",
           "sh", "-c", "terminal-notifier -title \"$BRNR_TITLE\" -message \"$BRNR_TEXT\""]
```

If the bridge's `events` limit what it gets, they must include
`agent_message`, `session_changed` and `exited`, which `notify` needs for its
environment and to know when to stop.

## Headless sessions

```sh
brnr start --cwd ~/work/project --prompt "fix the failing tests" -- brnr-claude-adapter
brnr start --mode plan --model opus --prompt - < task.md       # set up before the first prompt
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

`--stop-when-idle <s>` closes the session once it has been idle that many
seconds (no turn running, nothing held, no approval waiting), counting from
the start; `0` is as soon as it is. The process stops with its last session.

`--foreground` keeps the session in the terminal, for a supervisor such as
systemd or a container. It shows the session's events on stdout (`--json`: as
JSON lines; `--quiet`: not) and brnr's messages and the agent's own stderr on
stderr, and exits as the agent did, non-zero if the start failed. Ctrl-C stops
the session (twice kills the agent). A display that can't keep up skips
events, saying how many (`… 120 events not shown`); stdout closing
(`| head -1`) ends the display, not the session.

`--resume` uses the agent's `session/resume` (or `session/load`, without
recording the replayed history again), in the session's cwd, with the agent
and profile it last had, and appends to the same transcript.

With no editor attached brnr is the agent's client: approvals wait for
`brnr approve`/`deny` or a bridge (`permission_timeout` denies what nobody
answers), elicitation is declined, and anything else is answered with
"method not found". How much the agent asks is the agent's own setting: its
mode (`--mode`, `brnr mode`). brnr doesn't log in for you. If the agent needs
a login, the start fails with the agent's methods: log in with the agent's own
CLI first, or name a method that needs no terminal with `--auth <id>` (`auth`
in the profile), such as codex-acp's `api-key`.

## Strict mode and feature flags

By default brnr speaks stable ACP plus the conventions agents and editors
already implement alike, ahead of the spec: `_session/steering` (for
`send --steer`), `session/fork`, unstable in ACP v1 (for `brnr fork`), and
dropping the editor's `fs` and `terminal` capabilities, as ACP v2 does. Strict
mode (`strict = true` in a profile, `--strict` on `acp` and `start`) is stable
ACP to the letter: no steering, no fork, the editor's capabilities passed
through, and no experimental actions. What it refuses says why.

How brnr's processes share sessions isn't protocol, and strict mode doesn't
change it: it has defaults that protect you, and feature flags in a profile's
editor part to change them. The one so far is `shared_sessions`: the editor
may load a session another brnr process serves. The agent then serves it to
both and the conversation splits; the process holding the session's lock still
writes its transcript, and `status` says the session is shared.

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
stdin, its stderr in the host log), or anything that connects to the control
socket. Its environment has `BRNR_PID` and `BRNR_SOCKET`.

Requests: `status`, `send`, `cancel`, `queue`, `subscribe`, `pending`,
`approve`, `deny`, `set_mode`, `set_config`, `set_model`, `fork`, `close`,
`stop` (see `src/host/control.rs`); those about a session name it by its exact
id, and those that act on an editor's session are
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
writes; a signal to `brnr acp` still reaches the agent at once. Observers never
slow the session: a bridge or watcher 16 MiB behind is cut off (a connection
closed, a started bridge sent SIGTERM), and the foreground's display skips
events.

## Transcripts

Like the agents' own transcripts, keyed by project folder:

```text
~/.brnr/projects/<folder>/<session id>.jsonl       # a session's events
~/.brnr/projects/<folder>/<session id>.acp.jsonl   # its raw ACP
~/.brnr/hosts/<run id>.jsonl                       # each process's, for what belongs to no session
```

`<folder>` is the session's cwd with every non-alphanumeric character replaced
by `-`, as in `~/.claude/projects`, so a claude-agent-acp session's events
file has the same folder and name as Claude Code's own transcript. The events are the
ones bridges get, `exited` included, and are what `brnr log`, `list --all` and
`--resume` read; `log` reads the raw file too when `acp` events are chosen.
Every record carries `host_id`, `host_pid`, `proxy_pid` and `agent_pid` for
joining. `log = "events"` leaves out the raw file, most of the space;
`log = false` writes nothing.

The values of MCP servers' `env` and `headers` are recorded as
`"<redacted>"`, in the raw ACP, the host log and `acp` events alike; the agent
gets them as given. Transcripts hold prompts and tool output, so brnr creates
them readable only by you.

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
`adapters/codex.ts`): finding the user's own agent, and `--version`. The
commands have names of their own so they don't clash with the npm packages'
(`claude-agent-acp`, `codex-acp`) when both are installed.

They don't include the agents. Each runs the user's own `claude` or `codex`
(from `PATH` or the usual install locations, or `CLAUDE_CODE_EXECUTABLE` /
`CODEX_PATH`). `build.sh` copies every package's license file (`LICENSE`,
`license`, `License.txt`, …), nested packages' too, to `licenses/`. The Claude Agent SDK's license isn't an open-source one; check
Anthropic's terms before redistributing a build of brnr-claude-adapter.

## Doctor

```sh
brnr doctor                        # checks what brnr depends on
brnr doctor --fix                  # … and repairs what it safely can
```

It checks that the runtime directory is private, isn't a symlink, is short
enough for socket paths, and holds nothing left by processes that are gone
(their metadata, their session locks); that transcripts are readable only by
you; that the config parses, with every key in its part, and each profile's
agent, cwd, MCP servers and bridges are valid; where the adapters are found;
and that every running process answers, naming the sessions one that doesn't
still serves. `--fix` makes the runtime directory and transcripts private
(only what you own, never through a symlink) and removes what processes that
are gone left behind. It exits non-zero if a check fails; `--json` prints the
checks.

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

Dependabot opens a pull request when an adapter's npm package has a new
release; the next brnr release ships it.

## License

Apache-2.0.
