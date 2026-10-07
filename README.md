# brnr

A burner phone for your coding agents.

brnr sits between your editor and an [ACP](https://agentclientprotocol.com/)
agent (Claude Code, Codex, …). The editor talks to the agent exactly as before,
but the agent now lives in a process of its own that you can also reach from
outside the editor: send its sessions messages, watch everything they do, and
answer their approvals from somewhere else. Or run sessions with no editor at
all.

```text
editor ──stdio── brnr acp ──socketpair── brnr process ──pipes── agent
                                            │
                                 control socket ── brnr send / watch / …
                                            │
                                         bridges (Slack, push, …)
```

- **brnr acp** is what the editor runs as its agent. It only relays bytes.
- **The brnr process** owns the agent for its whole life, out of the editor's
  process group and process tree. When the editor goes, it stops the agent, as
  if the editor had run it. `brnr ps` lists these processes.
- **The other commands** (`brnr send`, `brnr watch`, …) talk to a session
  through its process, over a Unix socket that only you can open.

macOS and Linux (Unix sockets only).

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
doesn't need to include it. When the editor goes away, the agent is stopped,
just as if the editor had run it; `brnr start --resume <session>` carries on
with one of its sessions headless.

## Talk to it from outside

```sh
brnr list                          # running sessions; --all, --inactive: ended ones too
brnr status $s                     # what it's doing: turn, tools, plan, tokens, last message
brnr send $s "also update the changelog"
brnr send $s --after-turn "then run the tests"
brnr send $s --interrupt "stop, wrong branch"
brnr send $s --context "the API key is in .env.local"   # added to the next prompt
brnr send $s --wait "what did you change?"              # prints the reply
brnr send $s --file src/api.rs --image screenshot.png "why does this look wrong?"
brnr cancel $s                     # stop the running turn (held messages are dropped and listed)
brnr queue $s                      # held messages and context; --drop m3, --clear
brnr watch $s                      # live: messages, tools, plan, approvals (--events all: thoughts, ACP too)
brnr log $s                        # the story so far; --last 2, --follow, and watch's flags
```

A `<session>` is the session's id, as the agent gave it: `brnr list` shows
them, with the agent's title for each, and `start --json` prints the new one
(`s=$(brnr start --json … | jq -r .session)`). `log` works on a session that
has ended too; commands that need it running say so when it isn't.

Injected messages reach the agent as ordinary user messages, and the editor
shows them as a completed tool call ("Message via brnr"), the one update
editors render anywhere in a turn. When an agent takes up a message
sent mid-turn is the agent's business: claude-agent-acp, for one, folds it into
the running turn. Every message gets an id (`m<n>`), which `send --wait` uses to
find the turn that answers it.

Every command that prints something takes `--json`, with the same data as its
text: one JSON value, or one event per line for `log`, `watch` and
`start --foreground`.

### Waiting, for scripts

```sh
brnr wait $s                       # until no turn is running and nothing is held
brnr wait $s --for permission      # until an approval is waiting
brnr wait $s --for turn            # the next turn's end; --for exit: its process's
brnr start --wait --stop-when-idle 0 --prompt "fix the failing tests" -- brnr-claude-adapter
```

`wait`, `send --wait` and `start --wait` exit 0 when the turn ended normally
(`end_turn`), 1 when it failed or stopped for another reason, and 124 on
`--timeout <s>`; `wait` on a session that is idle already exits as its last
turn ended. While they wait, approvals are announced on stderr.

### Approvals

```sh
brnr pending                       # approvals waiting, in every session
brnr show $s p1                    # one in full: the command, paths, the diff
brnr approve $s p1                 # or: brnr deny $s p1, --option <id>
```

Whatever brnr shows of the agent's text has its control characters escaped
(`\u001b`), so a command can't be dressed up as another; `show` says when a
command has any. `--json` has the text as the agent sent it.

### Settings

```sh
brnr mode $s [plan]                # list or set the session's mode
brnr model $s [<model>]            # list or set the model
brnr config $s [effort=high]       # any of the agent's config options
brnr commands $s                   # the agent's slash commands (send them as text)
```

### Sessions and processes

```sh
brnr fork $s                       # a copy of the session, in the same process
brnr close $s                      # close one; a headless process with none left stops
brnr sessions -- brnr-claude-adapter   # the agent's own list for this folder (--cwd), and what brnr knows of each
brnr start --resume <id> -- brnr-claude-adapter    # any of them, even one brnr never saw
brnr ps                            # brnr's processes: pid, owner, agent, sessions
brnr stop 4466                     # stdin closed, then SIGTERM, then SIGKILL
```

`brnr sessions` starts the agent just to ask it (`session/list`), so it
includes sessions started outside brnr and needs nothing running; its STATE
and PID columns say, as `brnr list` would, which are running and in which
process, which brnr has a transcript of (`inactive`), and which only the
agent knows (`-`). `--resume` takes an id brnr knows, or any id the
agent knows, which it resumes in `--cwd` (or here) with the agent after
`--` (or the profile's). `brnr stop` signals the agent's whole process group,
so whatever the agent started goes with it.

### Notifications

`brnr notify` runs a command for each event of a session, or of every session
in a process (`--pid`): by default `permission_request`, `turn_ended` and
`exited`; `--events` as for `watch`, where `default` means these three. The
event is in its environment (`BRNR_EVENT`, `BRNR_TEXT`, `BRNR_TITLE`,
`BRNR_MESSAGE`, `BRNR_SESSION_ID`, `BRNR_REQUEST`,
`BRNR_PID`) and, as JSON, on its stdin; nothing is put on its command line, so
what the agent writes can't become arguments. The text, title and message are
escaped as `watch` shows them and cut at 32 KiB; the event on stdin is whole.
`notify` exits when the process does, and fails if the process cuts it off
first.

```sh
brnr notify $s -- sh -c 'curl -s -d "$BRNR_TEXT" ntfy.sh/my-agents'
```

In a profile it is a bridge, for every process that profile starts, which is
given its pid in `$BRNR_PID`:

```toml
[[profiles.work.bridges]]
command = ["sh", "-c", "exec brnr notify --pid \"$BRNR_PID\" -- sh -c 'terminal-notifier -title \"$BRNR_TITLE\" -message \"$BRNR_TEXT\"'"]
```

## Headless sessions

```sh
brnr start --cwd ~/work/project --prompt "fix the failing tests" -- brnr-claude-adapter
brnr start --mode plan --model opus --prompt - < task.md       # set up before the first prompt
brnr start --resume $s                                         # carry on a session that ended
brnr start --foreground --prompt - -- brnr-codex-adapter < task.md     # in the foreground; Ctrl-C stops it
```

`brnr start` waits up to 120 seconds (`BRNR_START_TIMEOUT`) for the session
to open. If it gives up, or is interrupted, the process stops too and the
prompt is never sent. Messages still held when the agent exits are listed in
the `exited` event as `undelivered`. `--stop-when-idle <s>` closes the session
once it has been idle that many seconds (no turn running, nothing held, no
approval waiting), counting from the start; `0` is as soon as it is.

`--foreground` keeps the session in the terminal, for a supervisor such as
systemd or a container: it shows the session as it goes (`--json`: as JSON
lines; `--quiet`: not), Ctrl-C stops it (twice kills the agent), and it exits
as the agent did, or 1 if the start failed (setting `--mode`, say).

`--resume` uses the agent's `session/resume` (or `session/load`, without
recording the replayed history again), in the session's cwd, with the agent it
last had, and appends to the same transcript.

With no editor attached brnr is the agent's client: approvals wait for
`brnr approve`/`deny` or a bridge (`permission_timeout` denies what nobody
answers), elicitation is declined, and anything else is answered with
"method not found". How much the agent asks is the agent's own setting:
its mode (`--mode`, `brnr mode`). If the agent needs a login, the start
fails and says so: log in with the agent's own CLI first. The editor's `fs` and
`terminal` client capabilities are removed from `initialize` up front (ACP v2
drops them), so the agent never comes to rely on something only an editor can
provide.

## Profiles

`~/.config/brnr/config.toml`:

```toml
[profiles.work]
agent = ["brnr-claude-adapter"]
cwd = "~/work/project"              # for brnr start
permission_timeout = 600            # deny what nobody answered in 10 minutes
mode = "plan"                       # headless sessions: mode and config options
config = { model = "opus" }         #   applied before the first prompt
stop_when_idle = 600                # close a headless session idle 10 minutes
log = true

[[profiles.work.bridges]]
command = ["~/bin/slack-bridge", "--channel", "#agents"]
events = ["permission_request", "turn_ended"]

[[profiles.work.mcp_servers]]       # for sessions brnr opens
name = "github"
command = "github-mcp-server"       # or url = "https://…", type = "http" | "sse"
args = ["stdio"]
env = { GITHUB_TOKEN = "…" }
```

Tool kinds are ACP's: read, edit, delete, move, search, execute, think, fetch,
switch_mode, other.

## Bridges

A bridge is any process that speaks brnr's JSON-lines protocol: one started by
the brnr process from the profile (requests on its stdout, events on its
stdin; it should exit when its stdin closes), or anything that connects to the
control socket. Its environment has `BRNR_PID` and `BRNR_SOCKET`.

Requests: `status`, `send`, `cancel`, `queue`, `subscribe`, `pending`,
`approve`, `deny`, `set_mode`, `set_config`, `set_model`, `fork`,
`close`, `stop` (see `src/host/control.rs`); those about a session name it by
its exact id. Events: `user_message`, `agent_message`, `agent_thought`,
`tool_call`, `plan`, `usage`, `session_changed`, `permission_request`,
`permission_resolved`, `turn_ended`, `exited`, and `acp` (every ACP message
with its direction; only sent to subscribers that ask for it). Each names its
session; `exited` is the process's.

A bridge has to keep reading: one that falls 16 MiB behind is dropped (a
connection is closed, a started bridge gets SIGTERM) rather than buffered for
without limit.

## Transcripts

Like the agents' own transcripts, keyed by project folder:

```text
~/.brnr/projects/<folder>/<session id>.jsonl    # one per ACP session
~/.brnr/hosts/<run id>.jsonl                    # each process's, for what belongs to no session
```

`<folder>` is the session's cwd with every non-alphanumeric character replaced
by `-`, as in `~/.claude/projects`, so a claude-agent-acp session's file has the
same folder and name as Claude Code's own transcript. Every record carries
`host_id`, `host_pid`, `proxy_pid` and `agent_pid` for joining. Besides the raw
ACP, a session's file has its events (the same ones bridges get), which is
what `brnr log` reads; the agent exiting is in each session's
file as well as the process's. Transcripts hold prompts and tool output, so brnr
creates them readable only by you.

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

It checks that the runtime directory is private, isn't a symlink and is short
enough for socket paths; that transcripts are readable only by you; that the
config parses and each profile's agent, cwd, policies and bridges are valid;
where the adapters are found; and that every running process answers. `--fix` makes the runtime directory and transcripts
private (only what you own, never through a symlink) and removes files left by
processes that are gone. It exits non-zero if a check fails; `--json` prints
the checks.

## Environment

| Variable | Default |
|---|---|
| `BRNR_HOME` | `~/.brnr` (transcripts) |
| `BRNR_CONFIG` | `$XDG_CONFIG_HOME/brnr/config.toml`, else `~/.config/brnr/config.toml` |
| `BRNR_DIR` | `$XDG_RUNTIME_DIR/brnr`, else `$TMPDIR/brnr-<uid>` (sockets and metadata; brnr refuses one others can use) |
| `BRNR_START_TIMEOUT` | `120`: seconds `brnr start` waits for the session |

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
