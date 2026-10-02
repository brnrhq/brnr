# brnr

A burner phone for your coding agents.

brnr sits between your editor and an [ACP](https://agentclientprotocol.com/)
agent (Claude Code, Codex, …). The editor talks to the agent exactly as before,
but the agent now lives in a separate host process that you can also reach from
outside the editor: send it messages, watch everything it does, answer its
permission requests from somewhere else, and keep it running after the editor
goes away.

```text
editor ──stdio── brnr proxy ──socketpair── brnr host ──pipes── agent
                                              │
                                   control socket ── brnr send / watch / …
                                              │
                                           bridges (Slack, push, …)
```

- **brnr proxy** is what the editor runs as its agent. It only relays bytes.
- **brnr host** owns the agent for its whole life. It runs in its own session
  (out of the editor's process group and process tree), so killing the editor,
  or the proxy, can't take the agent with it unless the policy says so.
- **The control commands** (`brnr send`, `brnr watch`, …) talk to running hosts
  over a Unix socket that only you can open.

macOS and Linux (Unix sockets only).

## Install

```sh
brew install brnrhq/tap/brnr
```

Or from source, below. More at [brnrhq.github.io/brnr](https://brnrhq.github.io/brnr/).

## Build

```sh
cargo build --release          # target/release/brnr
adapters/build.sh              # optional: target/release/claude-agent-acp, codex-acp (needs bun)
```

`brnr` is one binary. The adapters are separate, single-file builds of the ACP
adapters for Claude Code and Codex; see [Adapters](#adapters).

## Use it from an editor

Configure your editor's ACP agent command as:

```sh
brnr proxy -- claude-agent-acp
brnr proxy -- codex-acp
brnr proxy --profile work          # agent and settings from a profile
```

A bare agent name is looked up next to `brnr` first, so the editor's `PATH`
doesn't need to include it, and the bundled adapters are used even if the npm
packages of the same name are installed too.

When the editor goes away without a handoff, `on_disconnect` decides what happens:

- `direct` (default): exactly what a directly spawned agent would get: its
  stdin is closed and it is killed.
- `headless`: the agent keeps running and the host becomes its client.

## Talk to it from outside

```sh
brnr list                          # running hosts and their sessions
brnr list --all                    # … plus inactive sessions, from their transcripts
brnr send demo "also update the changelog"
brnr send demo --after-turn "then run the tests"
brnr send demo --interrupt "stop, wrong branch"
brnr send demo --context "the API key is in .env.local"   # added to the next prompt
brnr watch demo                    # every event and ACP message, live
brnr pending                       # permission requests waiting for an answer
brnr approve demo                  # or: brnr deny demo p2, --option <id>
brnr stop demo                     # stdin closed, then SIGTERM, then SIGKILL
```

A target is a host id, a `--name`, or an ACP session id (or a unique prefix).
`brnr stop` signals the agent's whole process group, so whatever the agent
started goes with it.

Injected messages reach the agent as ordinary user messages, and the editor
shows them as such (`user_message_chunk`). When an agent takes up a message
sent mid-turn is the agent's business: claude-agent-acp, for one, folds it into
the running turn.

## Headless sessions

```sh
brnr start --cwd ~/work/project --prompt "fix the failing tests" -- claude-agent-acp
brnr host --name demo --prompt - -- codex-acp < task.md     # in the foreground; Ctrl-C stops it
```

`brnr start` waits up to 120 seconds (`BRNR_START_TIMEOUT`) for the session
to open. If it gives up, or is interrupted, the host stops too and the prompt
is never sent. A `--name` must be unique among running hosts. Messages still
held when the agent exits are listed in the `exited` event as `undelivered`.

With no editor attached the host is the agent's client: permission requests
follow the `permissions` policy (`ask` waits for `brnr approve`/`deny` or a
bridge), elicitation is declined, and anything else is answered with "method
not found". The editor's `fs` and `terminal` client capabilities are removed
from `initialize` up front (ACP v2 drops them), so the agent never comes to rely
on something a headless host can't provide.

## Profiles

`~/.config/brnr/config.toml`:

```toml
[profiles.default]
on_disconnect = "direct"

[profiles.work]
agent = ["claude-agent-acp"]
cwd = "~/work/project"              # for brnr start
on_disconnect = "headless"
permissions = "ask"                 # ask | auto-allow | auto-deny
log = true

[[profiles.work.bridges]]
command = ["~/bin/slack-bridge", "--channel", "#agents"]
events = ["permission_request", "turn_ended"]
```

## Bridges

A bridge is any process that speaks brnr's JSON-lines protocol: one started by
the host from the profile (requests on its stdout, events on its stdin; it
should exit when its stdin closes), or anything that connects to the control
socket. Its environment has `BRNR_HOST`, `BRNR_HOST_ID` and `BRNR_SOCKET`.

Requests: `status`, `send`, `subscribe`, `pending`, `approve`, `deny`, `stop`.
Events: `user_message`, `agent_message`, `permission_request`,
`permission_resolved`, `turn_ended`, `owner_changed`, `exited`, and `acp`
(every ACP message with its direction; only sent to subscribers that ask for it).

A bridge has to keep reading: one that falls a few thousand lines behind is
dropped (a connection is closed, a started bridge gets SIGTERM) rather than
buffered for without limit.

## Transcripts

Like the agents' own transcripts, keyed by project folder:

```text
~/.brnr/projects/<folder>/<session id>.jsonl    # one per ACP session
~/.brnr/hosts/<host id>.jsonl                   # what belongs to no session
```

`<folder>` is the session's cwd with every non-alphanumeric character replaced
by `-`, as in `~/.claude/projects`, so a claude-agent-acp session's file has the
same folder and name as Claude Code's own transcript. Every record carries
`host_id`, `host_pid`, `proxy_pid` and `agent_pid` for joining. Transcripts
hold prompts and tool output, so brnr creates them readable only by you.

## Adapters

`adapters/` builds the ACP adapters as single-file executables with
`bun build --compile`, under the same command names their npm packages install:

- **claude-agent-acp**: [@agentclientprotocol/claude-agent-acp](https://github.com/agentclientprotocol/claude-agent-acp)
- **codex-acp**: [@agentclientprotocol/codex-acp](https://github.com/agentclientprotocol/codex-acp)

They don't include the agents. Each runs the user's own `claude` or `codex`
(from `PATH` or the usual install locations, or `CLAUDE_CODE_EXECUTABLE` /
`CODEX_PATH`). `build.sh` copies the license of every package it compiles in to
`licenses/`. Note that claude-agent-acp contains the Claude Agent SDK, which is
licensed under Anthropic's Commercial Terms, not an open-source license; check
those terms before redistributing it.

## Doctor

```sh
brnr doctor                        # checks what brnr depends on
brnr doctor --fix                  # … and repairs what it safely can
```

It checks that the runtime directory is private, isn't a symlink and is short
enough for socket paths; that transcripts are readable only by you; that the
config parses and each profile's agent, cwd, policies and bridges are valid;
where the adapters are found; and that every running host answers and no two
share a name. `--fix` makes the runtime directory and transcripts private
(only what you own, never through a symlink) and removes files left by hosts
that are gone. It exits non-zero if a check fails.

## Environment

| Variable | Default |
|---|---|
| `BRNR_HOME` | `~/.brnr` (transcripts) |
| `BRNR_CONFIG` | `$XDG_CONFIG_HOME/brnr/config.toml`, else `~/.config/brnr/config.toml` |
| `BRNR_DIR` | `$XDG_RUNTIME_DIR/brnr`, else `$TMPDIR/brnr-<uid>` (sockets and metadata) |
| `BRNR_START_TIMEOUT` | `120`: seconds `brnr start` waits for the session |

## License

Apache-2.0.
