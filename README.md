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
  a group's ([ADR 8.1](docs/adr/0008-the-command-line-and-compatibility.md#81-commands-are-grouped-by-what-they-act-on-with-acps-verbs)).
  `--help` exits 0, and a command used wrongly exits 2 with its usage on
  stderr, having done nothing ([ADR 8.3](docs/adr/0008-the-command-line-and-compatibility.md#83-waiting-and-exit-status)).

macOS and Linux (Unix sockets only). Why brnr does what it does, and what else
was considered, is in [docs/adr](docs/adr/README.md).

brnr never phones home: it makes no network requests of its own and sends
no telemetry ([P15](docs/adr/0001-principles.md)). Who can
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

The crate is the binary. Its library isn't an API to build on: its one
public item is `brnr::run`, the command line as a function
([ADR 8.5](docs/adr/0008-the-command-line-and-compatibility.md#85-the-library-is-private-and-its-facade-mirrors-the-command-line)).

Or from source, below.

The formulae build from the source tarball on each
[release](https://github.com/brnrhq/brnr/releases), `brnr-X.tar.gz` for
version X, which the release workflow makes from the tag and attests. To
check one you downloaded was made that way, with its version for X:

```sh
gh attestation verify brnr-X.tar.gz -R brnrhq/brnr
```

Each release also carries `brnr-X.cdx.json`, an attested CycloneDX SBOM of
brnr's Rust dependencies across all targets (not the adapters' npm packages).
CI checks that two clean release builds, in different directories and at
different times, have the same SHA-256 on each of Linux and macOS. This
checks reproducibility within one toolchain and runner, not across Rust or
OS versions. After publishing and updating the tap, a fresh macOS runner
installs the formula, runs its `brew test`, checks its version against the
tag, and runs `brnr doctor`
([ADR 11.7](docs/adr/0011-building-releasing-and-support.md#117-reproducible-builds-and-an-sbom)).

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
unchanged. It changes the stream in three ways: every request reaches the
agent with an id of brnr's, and its answer comes back to the editor with the
editor's own id, so that the editor's requests and brnr's are never confused
(ADR 2.4); and, each below, a session another brnr process serves can't be
loaded, and the experimental actions a profile enables act on the editor's
session. By default the editor's `fs` and `terminal` capabilities don't
reach the agent, as ACP v2 has neither: the agent reads and writes the files
on disk and runs commands itself, and doesn't see the editor's unsaved
buffers. A profile passes them by name (`client_capabilities = ["fs",
"terminal"]`), and strict mode passes both, as stable ACP v1 has them; a
headless session offers neither
([ADR 2.7](docs/adr/0002-acp-faithfully.md#27-the-editors-file-system-and-terminal-reach-the-agent-in-strict-mode-or-when-its-profile-names-them)). When the editor goes away, so does the agent;
`brnr session resume <session>` carries on with one of its sessions headless
(a turn still running is lost).

Watching an editor's session (`event watch`, `event log`, `session status`,
`permission requests`, `permission show`, `elicitation requests`,
`elicitation show`, `event notify`, bridges) always works. Acting on it doesn't, by default: ACP has one
client per session, the editor, and every action from outside leaves the
editor's view behind the agent's in some way.

### Experimental actions

Each is refused on an editor's session unless the editor's part of its profile
enables it by name (without `--profile`, the profile is `default`):

```toml
[profiles.work.editor]
experimental = ["send", "permission"]
```

| Action | Commands | What brnr does for the editor |
|---|---|---|
| `send` | `prompt send` | Sent only while no turn runs: the editor controls its turns, so nothing is held, steered or interrupts. Shown as a completed tool call, "Message via brnr", or as the user's message (`echo_as`, below). |
| `context` | `prompt send --context`, `queue clear --context` (or neither flag) | Added to the editor's next prompt, shown as "Context via brnr", or as the user's message (`echo_as`). |
| `cancel` | `prompt cancel` | The agent's pending approvals are answered `cancelled` and its elicitations `cancel`, and withdrawn from the editor (`$/cancel_request`). |
| `permission` | `permission allow`, `permission reject` | The request is withdrawn from the editor, its tool call set `in_progress` or `failed`, and the editor told at once who answered ("Allowed via brnr", "Rejected via brnr"). If the editor answers anyway, its answer is dropped and it is told who answered first. |
| `elicitation` | `elicitation accept`, `elicitation decline`, `elicitation cancel` | The request is withdrawn from the editor, and the editor told at once who answered ("Accepted via brnr", "Declined via brnr"). If the editor answers anyway, its answer is dropped and it is told who answered first. |
| `config` | `config set` | The editor is sent the `current_mode_update` or `config_option_update` the agent sends only to whoever asked. |
| `close` | `session close`, `session resume --take-over` | The turn is cancelled, the editor is told ("Session closed via brnr", "Session taken over by brnr (process 4466)"), and its later requests for the session get an error saying where it continues. |

A message or context is a tool call because not every editor renders a user
message that arrives outside a turn. For an editor that does, `echo_as =
"user_message_chunk"` in the editor part shows it as the user's message
instead, one `user_message_chunk` per content block
([ADR 7.2](docs/adr/0007-experimental-actions-on-an-editors-session.md#72-how-a-message-sent-through-brnr-is-shown-to-the-editor)).
brnr's own notes ("Allowed via brnr" and the rest) stay tool calls.

`session fork` is never available in an editor's process, and in strict mode
none of these is, whatever the profile enables. `brnr process stop` isn't an
action on a session: it stops an editor's process as it does any other.

### What the editor doesn't see

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
  says which ([ADR 2.6](docs/adr/0002-acp-faithfully.md#26-the-editors-stdout-ends-when-the-agents-does-and-its-stderr-comes-as-written)).
- Both streams come through one connection, in order, with 16 MiB in flight
  rather than a pipe's 64 KiB: an editor that stops reading one of them holds
  up the other too, once that much is waiting. The editor's stderr is also
  `brnr acp`'s own, for its errors, so it ends when `brnr acp` exits rather
  than when the agent closes its stderr, and an editor closing it doesn't give
  the agent EPIPE.
- None of the compensations above has been tried against real editors yet
  (Zed, IntelliJ's AIR plugin): how each shows them is still to be seen.
  Each action stays experimental through 1.x, and leaves it only by an ADR
  that brings what was seen in real editors; ADR 2.7 lists the checks.

## Talk to it from outside

```sh
brnr session list                  # brnr's sessions: open ones and ended ones (--include active: open only)
brnr session status $s             # what it's doing: turn, tools, plan, usage, last turn and message
brnr prompt send $s --prompt "also update the changelog"               # held while a turn runs
brnr prompt send $s --steer --prompt "and the tests too"               # into the running turn
brnr prompt send $s --interrupt --prompt "stop, wrong branch"
brnr prompt send $s --context --prompt "the API key is in .env.local"  # added to the next prompt; --replace: instead of the last
brnr prompt send $s --wait --prompt "what did you change?"             # prints the reply
brnr prompt send $s --file src/api.rs --image screenshot.png --prompt "why does this look wrong?"
brnr prompt cancel $s              # stop the running turn (held messages are dropped and listed)
brnr queue list $s                 # held messages and context; brnr queue show $s m3: one in full
brnr queue clear $s                # drop them (--messages, --context: only those); brnr queue drop $s m3: one
brnr event watch $s                # live: messages, tools, plan, approvals (--events all: everything)
brnr event log $s                  # the story so far; --last 2, --follow, and event watch's flags
```

A `<session>` is the session's id, as the agent gave it: `brnr session list`
shows them, with the agent's title for each, and `session new --json` prints
the new one, with its process's `pid` and the prompt's id as `message`
(`s=$(brnr session new --json … | jq -r .session)`). `event log` works on a session
that has ended too; commands that need it running say so when it isn't. On an
editor's session, those that act on it are [experimental](#experimental-actions).

The message is `--prompt`'s value, as for `session new`, taken as given:
`brnr prompt send $s --prompt -h` sends `-h`. `--prompt -` reads it from
stdin. A word that is no flag's is a usage error.

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
`prompt cancel` (unless `--keep-held`), `queue drop` or `queue clear`,
its session closing, the agent exiting, or a steer the agent didn't take.
Context that never joins a prompt is a `context_dropped` event the same way:
`queue clear`, its session closing, or the agent exiting
(`prompt cancel` keeps it for the next prompt).

Every command that prints data takes `--json`, with the same data as its
text: one JSON value, or one event per line for `event log`, `event watch`
and `session new --foreground`. `session close --json` is `{session}`, and
`process stop --json` `{pid, status}`; `event notify`, whose output is its
command's, has none.

### Waiting, for scripts

```sh
brnr event wait $s                 # until no turn is running and nothing is held
brnr event wait $s --for permission   # until an approval is waiting
brnr event wait $s --for elicitation  # until the agent asks for input
brnr event wait $s --for turn      # the next turn's end; --for exit: its process's
brnr session new --wait --stop-when-idle 0 --prompt "fix the failing tests" -- brnr-claude-adapter > answer.md
```

`event wait`, `prompt send --wait` and `session new --wait` exit 0 when the turn
ended normally (`end_turn`); 1 when it failed or stopped for another reason,
when its message was dropped (`brnr: m3 was dropped (cancel)`), when the agent
exited first, or when the session closed before the turn, approval or
elicitation waited for; 124 on `--timeout <s>`; and 3 when brnr cut the
command off for falling behind and the transcript can't say what it missed
(see [when a reader falls behind](#when-a-reader-falls-behind)). `event wait`
on a session that is idle already, or that closes while it waits, exits as
its last turn ended. While `prompt send --wait` and `session new --wait`
wait, approvals and elicitations are announced on stderr; `session new --wait`
says `started …` there too, so stdout is the reply and nothing else. With `--json`, the turn is one object at its end:
`session`, `message`, `reply`, `stop_reason`, `error`, `tokens`, `dropped`
(and `pid`, for `session new` and `resume`).

### Approvals

```sh
brnr permission requests           # approvals waiting, in every session
brnr permission show $s p1         # one in full: the command, paths, the diff
brnr permission allow $s p1        # with its allow_once option; --always: allow_always
brnr permission reject $s p1       # with its reject_once option; --always: reject_always
```

`allow` and `reject` answer with the request's option of that kind, whatever
the order the agent gives them in. No other kind stands in for it: a request
without one, or with two, isn't answered, and the command fails, listing the
options. `--option-id <id>` names the option instead (for two of one kind, or a
kind ACP doesn't have): one of an ACP kind must be on the verb's side, and the
always kind with `--always`; one of a kind brnr doesn't know goes with either
verb. `reject` never answers `cancelled`, which ACP keeps for a cancelled
turn: `prompt cancel` is what does that.

A headless request waits until it is answered, or for `response_timeout`
(in the profile's headless part), if set: then it is answered with its
`reject_once` option, or, without exactly one, its turn is cancelled
(`session/cancel`), which answers it and every request pending in the session
`cancelled`. A timeout never allows, nor answers `reject_always`: nobody chose
that. `response_timeout` has no default (P4): without it, a request nobody
answers waits as long as the process runs. An unattended run, with nobody to
answer (a script, CI, a worker left alone), must set it.

The agent can give up on a request itself (`$/cancel_request`): it is no
longer waiting, `permission_resolved` says `by: agent`, and brnr answers it
`cancelled`, as ACP has it; an answer to it after that fails, and nothing is
sent. On an editor's session the editor gets the agent's notification and
answers it. A turn that ends with its request unanswered doesn't end the
request: it waits as any does, and the session stays `waiting`.

Whatever brnr shows of the agent's text has its control characters escaped
(`\u001b`), so a command can't be dressed up as another; `permission show`
says when a command has any. `--json` has the text as the agent sent it. On an
editor's session `permission show` says the request is waiting in the editor,
and that it can be answered here too, experimentally, where `permission` is
enabled, or else why it can't (`answerable` and `why_not` in `--json`).

### Elicitations

```sh
brnr elicitation requests          # the agent asking for input, in every session
brnr elicitation show $s e1        # one in full: the message, the form's fields, or the URL
brnr elicitation accept $s e1 --field name=Ada --field color=green   # checked against the form
brnr elicitation decline $s e1     # the user's no
brnr elicitation cancel $s e1      # no answer given, as a timeout sends
```

An elicitation is the agent asking for input (ACP's `elicitation/create`, ADR
68): a form, or a URL to open, such as an MCP server's login. Headless, brnr
advertises both modes, and holds each request until it is answered, as it
holds an approval. `accept` takes the form's values as `--field name=value`
(again for each value of a list) or as one JSON object on stdin (`-`), and the
process checks them against the form before anything is sent: each field is
one of the form's, the required ones are given, and each value is of its type
(`42` for an integer, `true` for a boolean), within its bounds and one of its
choices; `pattern` and `format` are left to the agent. What doesn't fit fails
the command, naming the field, and nothing is sent. A URL is shown, never
opened: open it yourself, then `accept` it, with no fields. A mode brnr
doesn't know can only be declined or cancelled. One asked outside any session
(a login, while the agent authenticates) is named by its process, `--pid
<pid>` in place of the session.

`response_timeout` covers elicitations too: one nobody answered is answered
`cancel`, never `decline` or `accept`, and `elicitation show` says when. `prompt
cancel` answers the session's elicitations `cancel` too, and one the agent
gives up on (`$/cancel_request`) is answered `cancel`, `by: agent`, as an
approval is. On an editor's session the request reaches the editor as the
agent sent it, and is seen (`elicitation_request`, `elicitation_resolved`) as
approvals are; answering it here is the experimental `elicitation`.

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
brnr session fork $s               # a copy of the session, in the same process; status and list name its source
brnr session new --pid 4466 --prompt "review the diff"   # a session of its own, in a running process
brnr session resume --pid 4466 <id>                      # or one carried on there
brnr session close $s              # cancel a running turn, then close the session
brnr session delete $s             # the agent deletes its copy; brnr's transcript stays
brnr session delete $s --purge     # and brnr's transcript of it goes too
brnr session list -- brnr-claude-adapter   # this folder's (--cwd), joined with the agent's own list
brnr session resume <id> -- brnr-claude-adapter   # any of them, even one brnr never saw
brnr session resume $s --take-over # one another process serves: closed there, resumed here
brnr process list                  # brnr's processes: pid, owner, agent, sessions
brnr process stop 4466             # stdin closed, then SIGTERM, then SIGKILL, 5 s apart
```

A process holds a lock for each session it serves
(`~/.brnr/sessions/<id>.lock`, beside the transcripts, so two processes that
write one transcript contend for one lock however each found its runtime
directory), so which process has a session is known without asking it. A
process that doesn't answer (within 5 seconds) still has its sessions:
`session list` and `process list` show them `unreachable`, commands on them
say the process isn't answering, and `session resume` refuses them, naming
the process, as it does any session that is running. A process started with
another runtime directory (`BRNR_DIR`; a cron job without `XDG_RUNTIME_DIR`,
say) can't be reached from this one: `session list` shows its sessions
`unreachable` too, and a refusal names its runtime directory and that
`BRNR_DIR` set to it reaches the process. A process that is gone, even one
whose pid another process has now (nobody listens on its socket), isn't shown
or counted as running, and its metadata and socket are removed. `--take-over`
asks the owner to close the session (an editor's process does only if its
profile enables `close`) and resumes it in a new headless process; sessions
beside it keep running.

A session whose lock can't be taken (`sessions/` isn't private, or a directory
is where its lock file goes) isn't served headless: `session new` fails with the
cause and the process stops before the agent gets any prompt, and a
`session fork` into it is refused. An editor's process serves it all the same,
as the editor would be served without brnr; `session status` says it isn't
locked, and `doctor` what is in the way.

`session close` ends one session; a headless process with none left stops.
`session fork` is refused in an editor's process, in strict mode, when the
agent can't fork, and with `stop_when_idle` when the agent can't close
sessions.

`session new --pid <pid>` opens a session in a running process, with its
agent, profile and MCP servers, in `--cwd` (or here), and
`session resume --pid <pid> <id>` resumes one there (in its recorded cwd),
taking its lock as any resume does, with `--take-over` as without `--pid`.
What starts a process (`--profile`, `--auth`, `--strict`, `--stop-when-idle`,
`--response-timeout`, `--foreground`, `--quiet`, `-- <agent>`) is an error
with it. The session, its settings (over the profile's) and its prompt commit
as one, as a start does, and the prompt goes only then. A setting that fails
after the agent opened the session closes it again, and the error says so (an
agent that can't close sessions keeps it, and the error names it); so does
the start timeout passing first, which the process keeps too. A process
stopping meanwhile fails it, and the session ends with the process. A command
gone first (Ctrl-C) leaves no session. A session being opened counts as one:
the process doesn't stop as its others close or go idle meanwhile. It is
refused where `session fork` is, but for strict mode:
`session/new` and `session/resume` are stable ACP. With `--take-over`, what
the process would refuse (it is still starting or stopping, or its agent
can't resume sessions) is refused before the session is closed where it
runs.

`brnr session list` on its own is brnr's index, with nothing started: every
session open in brnr's processes and every one it has a transcript of, in
every folder (`--cwd` for one). With an agent named (`-- <agent>` or
`--profile`), it shows only this folder's (or `--cwd`'s), starts the agent
just to ask it (`session/list`, every page) and joins the two on the session
id, so it includes sessions started outside brnr; an agent that can't list
fails it. A folder is the same however it is spelled (a trailing slash, a
symlink), and an `unreachable` session is a folder's if the agent lists it
there or brnr has its transcript there. SESSION, TITLE, STATE, PID, AGENT, SOURCE, LAST ACTIVE, FORKED FROM and CWD, in
text and `--json` alike, most recently active first: STATE is `idle`, `busy`
or `waiting` for one open in a process, `unreachable` for one whose process
holds it but doesn't answer, `inactive` for one no process has open; SOURCE
says who knows it, `brnr`, `agent` or `both`; FORKED FROM is the session a
fork came from (`forked_from`), also once it has ended, as its transcript
records it; TITLE and LAST ACTIVE are the agent's where it gives them. LAST ACTIVE is in UTC, an agent's converted
from its offset to brnr's own form (`2026-10-11T06:00:00.000000Z` in
`--json`, `2026-10-11 06:00:00Z` in text), and the order is by the time it
names; one that isn't an RFC 3339 time is shown as the agent gave it, said
on stderr, after the sessions with a time and before those with none. `--include active` keeps the sessions open in a
process, `--include inactive` the others; both by default.

`session resume` takes an id brnr knows, or any id the agent knows, which it
resumes in `--cwd` (or here) with the agent after `--` (or the profile's).

`session delete` is `session/delete`, asked of the agent brnr recorded for
the session (or `--profile` or `-- <agent>`, for a session brnr has no
transcript of), started for that as `session list` starts one. A session
open in a process is refused (close it first), as is an agent that can't
delete sessions (`sessionCapabilities.delete`). Only the agent's copy goes:
brnr's transcript stays, still listed by `session list`, with a
`session_deleted` in it, a record of no process's (its `host_id` null).
`--purge` deletes the transcript too, the session's events file and raw ACP
in each project folder under `BRNR_HOME`, an older brnr's too, with those set
aside beside them (`<id>.older.jsonl`, …), and nothing else: the host logs
are shared and stay, and does so whatever the agent answered. With
`--purge`, the command exits 0 only if the agent deleted the session or
doesn't have it (ACP's `resource_not_found`, said on stderr); any other
error, or no answer, is said and exits non-zero: the agent may still have
the session. Only `resource_not_found` counts as not having it, and the
adapters, at the versions brnr pins
([adapters/package.json](adapters/package.json)), differ:
`brnr-codex-adapter` (codex-acp) answers a session it doesn't have as
deleted, so `--purge` succeeds, but `brnr-claude-adapter` (claude-agent-acp)
answers with another error (`-32603`, internal error), so `--purge` of a
session it has already deleted exits non-zero, brnr's transcript deleted all
the same. A file that can't be deleted, or have `session_deleted` recorded
in it, is said (`not deleted: …`, `session_deleted isn't recorded: …`) and
exits non-zero, the others done all the same; a transcript made private
first is said too on stderr (`path_made_private: <path> (its mode was …)`): there
is no host log for it. An editor's `session/delete` through `brnr acp` goes
to the agent as it is; once the agent has deleted the session, the editor's
process records `session_deleted` too, and closes the session if it had it
open.

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
| `permission_request`, `permission_resolved` | an approval waiting; its answer (`allowed`, `rejected` or `cancelled`), the chosen option's kind, and who gave it |
| `elicitation_request`, `elicitation_resolved`, `elicitation_completed` | the agent asking for input: its `text`, `mode`, `scope` (with its `tool_call_id` or `scope_request_id`), a form's `schema` or a `url`; the `action` sent (`accept`, with its `content`, `decline` or `cancel`) and who sent it; the agent saying a URL's flow is done |
| `turn_ended` | its `stop_reason` or `error`, the `tokens` the agent reported for it, and the `messages` it carried |
| `message_dropped` | a message that never went, its `reason` `cancel`, `queue`, `close`, `exit` or `steer` |
| `context_dropped` | context that never joined a prompt, its `reason` `queue`, `close` or `exit` |
| `session_closed` | `by` `control`, its `reason` `close` (`brnr session close`, `--take-over`) or `idle`; or `by` `editor`, `reason` `close` |
| `session_reopened` | an editor's load the agent answered with success after it answered the session's close: the session is open again, `by` `editor`, and the load's `history` follows |
| `session_deleted` | the agent deleted the session, `by` `control` (`brnr session delete`) or the `editor`; brnr's transcript stays |
| `history` | a load's replay: how many `updates`, whether they were `recorded` (as events with `replayed`), how many of them are (`recorded_updates`: fewer only when the session closed while its load was unanswered), and over how many `loads` (more than 1 only for an editor's loads that overlap) |
| `line_too_long` | a line `from` the `agent` or `editor` past the `limit` (32 MiB), unread: `relayed` to the other side with an editor, dropped headless |
| `line_too_deep` | a line `from` the `agent` or `editor` nested past `max_depth` (128 levels), unread and unrecorded: `relayed` to the other side with an editor, dropped headless |
| `peer_cut_off` | a `peer` (a watcher, a bridge) cut off for falling behind, and the last event it was sent (`last_queued_seq`) |
| `process_exited` | the agent exited, and the process with it: its `exit_status`, and an `error` when brnr itself crashed |
| `acp` | every ACP message, with its direction |

Each has `ts`, `host_id` (the process's run) and `pid` (the process), and
names its session; `process_exited`, `line_too_long` and `line_too_deep` are
the process's, and are in every session's transcript; `peer_cut_off` is the process's too,
in its host log. Each has its position, `seq`: one more than the last
event's or ACP message's of its process, live and in the transcript. Who
acted is `by`: `editor`, `agent` or `control` (brnr's commands, bridges and
timers); beside it, `peer` is the connection that asked, where brnr knows
one (`socket#7`, a bridge's name), and `reason` is why, a word
(`permission_resolved` by `control`: `peer` for `brnr permission allow`,
`reason` `timeout` or `cancel` otherwise; by `agent` when the agent gave the
request up with `$/cancel_request`). Every name is snake_case (ADR
5.9). For config options and commands, `session_changed` is a JSON merge
patch by id or name: `{"model": "opus"}`, a command added, `null` for one gone.

A turn's `tokens` are the counts the agent put in its answer to the prompt
(`usage`, unstable in ACP): `{"total", "input", "output", "thought",
"cached_read", "cached_write"}`, each the agent's number, or `null` where it
gave none that is a count (a JSON integer that fits a `u64`; not `1e3`), and
`tokens` itself `null` when the answer has no `usage` object. brnr adds
nothing up and doesn't say what they cover: whether subagents or earlier model
calls are in them is the agent's to say. `session status` has the last turn's,
in `last_turn` (ADR 4.4).

`--events` chooses, for `event watch`, `event log` and `event notify` alike:
names, `all`, and `default` (`--events default,agent_thought` adds thoughts).
It takes the names of brnr's two notes too, `records_skipped` (a gap in a
transcript) and `events_missed` (what a command cut off couldn't catch up
on), which are shown whatever it chose: `--events records_skipped` alone
shows only a transcript's gaps. A bridge's `events` and `subscribe` take
events only; a note's name fails a profile's bridge, as the process never
sends one.
Without it, `event watch`, `event log` and the foreground leave out the quiet
ones, `acp`, `agent_thought`, `usage` and `tool_progress`, in text and JSON
alike. Bridges get every event but `acp` unless they choose. Watching a
session ends when it closes.

### Notifications

`brnr event notify` runs a command for each event of a session, or of every
session in a process (`--pid`): by default `permission_request`,
`elicitation_request`, `turn_ended` and `process_exited`; `--events` as for `event
watch`, where `default` means these four. The event is in its environment (`BRNR_EVENT`, `BRNR_TEXT`,
`BRNR_TITLE`, `BRNR_MESSAGE`, `BRNR_SESSION`, `BRNR_REQUEST`, `BRNR_PID`)
and, as JSON, on its stdin; nothing is put on its command line, so what the
agent writes can't become arguments. The text, title and message are escaped
as `event watch` shows them and cut at 32 KiB; the event on stdin is whole.
Commands run one at a time, in order, each in a process group of its own.
`event notify` exits when the process does (or the session closes). Cut off
for falling behind, it lets the command finish, then catches up, running the
command for each event it missed, as `event watch` does; unable to, it says
what it missed and exits 3. Its connection ending otherwise first, or
stopped by a signal, it stops the command it is running, says on stderr that
no more notifications come, and exits non-zero.

```sh
brnr event notify $s -- sh -c 'curl -s -d "$BRNR_TEXT" ntfy.sh/my-agents'
```

In a profile it is a bridge, for every process the profile starts, and reads
the events the process writes on its stdin:

```toml
[[profiles.work.bridges]]
command = "brnr"
args = ["event", "notify", "--stdin", "--",
        "sh", "-c", "terminal-notifier -title \"$BRNR_TITLE\" -message \"$BRNR_TEXT\""]
```

If the bridge's `events` limit what it gets, they must include
`agent_message`, `session_changed` and `process_exited`, which `event notify` needs
for its environment and to know when to stop. What it says on stderr, that it
was cut off included, is in the host log (`bridge_stderr`). A started bridge
doesn't catch up: its stdin is closed, and it is sent SIGTERM. For
notifications that never miss a turn, run `brnr event notify <session>` on
the socket instead.

## Headless sessions

```sh
brnr session new --cwd ~/work/project --prompt "fix the failing tests" -- brnr-claude-adapter
brnr session new --mode plan --model opus --prompt - < task.md      # set up before the first prompt
brnr session new --option effort=high --prompt - < task.md          # any of the agent's config options, likewise
brnr session resume $s                                              # carry on a session that ended
brnr session new --auth api-key --prompt - -- brnr-codex-adapter < task.md   # log in first (OPENAI_API_KEY)
brnr session new --foreground --prompt - -- brnr-codex-adapter < task.md     # in the foreground; Ctrl-C stops it
```

`session new` opens a new session, and `session resume` an existing one;
each starts a process for it, and they take the same flags. A start is
atomic. It reads the prompt and attachments first, hands the process
everything in one request, and the start commits once the session is open
with its mode and config options set; only then does the agent get the
prompt. Until then, a step failing, the timeout (120 seconds,
`BRNR_START_TIMEOUT`) or the command being interrupted stops the process, and
the prompt is never sent. A start that fails ends its error with the agent's last
lines on stderr (`claude CLI not found`).

`--mode`, `--model`, `--thought-level` and `--option` win over the profile's
`mode`, `model`, `thought_level` and `options`, setting by setting. The model
is the agent's config option of category `model`, whatever its id, so
`--model large` replaces a profile's `options = { model = "small" }`; the mode
and the thought level (category `thought_level`) likewise. Two different
values for one setting from the flags (`--model large --option model=small`),
or from the profile, fail the start before anything is set, as does a
setting the agent has no option for.

`--stop-when-idle <s>` closes a session once it has been idle that many
seconds (no turn running, nothing held, no approval or elicitation waiting),
counting from the start; `0` is as soon as it is. Its last session isn't closed: the
process stops instead. `--response-timeout <s>` answers an approval nobody
answered in that many seconds as `response_timeout` does: with its
`reject_once` option, or, without exactly one, by cancelling its turn, which
answers every request pending in the session `cancelled`; an elicitation, by
answering it `cancel`. Each wins over the profile's `stop_when_idle` and
`response_timeout`.

`--foreground` keeps the session in the terminal, for a supervisor such as
systemd or a container. It shows the session's events on stdout (`--json`: as
JSON lines; `--quiet`: not) and brnr's messages and the agent's own stderr on
stderr, and exits as the agent did, non-zero if the start failed. Ctrl-C stops
the session (twice kills the agent). A display that can't keep up skips
events, saying how many (`… 120 events not shown`); stdout closing
(`| head -1`) ends the display, not the session.

`session resume` uses the agent's `session/resume` (or `session/load`), in the
session's cwd, with the agent and profile it last had (where it was last
served, the newest record of all its files), and appends to the same
transcript. `--profile p` runs p's agent instead, when p names one, and
`-- <agent>` runs that agent, whatever the profile. A load replays the session's history: brnr records it, as
events marked `replayed`, only when it has no transcript of the session (one
it has never seen, whose transcript is gone, or whose loads all failed,
leaving only their openings), so repeated loads don't record it twice. A
`history` event says how many updates were replayed and whether they were
recorded. An editor's `session/load` through `brnr acp` is
recorded the same way, and the editor gets the replay as the agent sent it.
Its loads of a session that overlap share one replay and one `history`
event, with how many `loads`; a recorded replay then has the history once per
load, each copy marked. brnr's transcript has only what happened through
brnr: turns taken in the agent's own client, or with `log = "off"`, aren't
in it, and a later load doesn't add them.

With no editor attached brnr is the agent's client: approvals wait for
`brnr permission allow`/`reject` or a bridge, elicitations for `brnr elicitation
accept`/`decline`/`cancel` or a bridge, and anything else is answered with
"method not found". How much the agent asks is the agent's own setting: its
mode (`--mode`, `brnr config set --mode`). brnr doesn't log in for you. If the agent needs
a login, the start fails with the agent's methods: log in with the agent's own
CLI first, or name a method that needs no terminal with `--auth <id>` (`auth`
in the profile), such as codex-acp's `api-key`.

## For agents

An agent can run other agents through brnr: start headless workers, send
them work, wait for their turns, read their logs and bring their approvals
to you. brnr ships a skill that teaches it how
([skills/brnr](skills/brnr/SKILL.md), [ADR 8.6](docs/adr/0008-the-command-line-and-compatibility.md#86-a-skill-for-agents-that-use-brnr)):
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
`prompt send --steer`) and `session/fork`, unstable in ACP v1 (for
`brnr session fork`). Strict mode (`strict = true` in a profile,
`--strict` on `acp`, `session new` and `session resume`) is stable ACP to
the letter: no steering, no fork, and no experimental actions. What it
refuses says why.

Either way, brnr speaks ACP version 1. A `brnr session new` (or `brnr session list` with an agent)
whose agent answers `initialize` with another version, or one brnr can't
read, fails with that, before anything else is sent, and the agent is
stopped ([ADR 2.2](docs/adr/0002-acp-faithfully.md#22-brnr-speaks-acp-version-1)). An editor's process
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
log = "all"                         # "events": no raw ACP; "off": no transcripts
strict = false                      # stable ACP only

[[profiles.work.bridges]]
command = "~/bin/slack-bridge"      # a program and its arguments, as an MCP server's
args = ["--channel", "#agents"]
events = ["permission_request", "turn_ended"]   # omit for every event but acp

[profiles.work.headless]            # brnr session new and resume
cwd = "~/work/project"
mode = "plan"                       # the agent's mode, model, thought level
model = "opus"                      #   and config options by id, set before
thought_level = "high"              #   the first prompt; the flags win
options = { effort = "high" }
response_timeout = 600              # reject once (else cancel the turn) what
                                    #   nobody answered in 10 minutes
stop_when_idle = 600                # close a session idle 10 minutes
# auth = "api-key"                  # a login method to run first (codex-acp's)

[[profiles.work.headless.mcp_servers]]   # for sessions brnr opens; an editor brings its own
name = "github"
command = "github-mcp-server"       # or url = "https://…", type = "http" | "sse", headers
args = ["stdio"]
env = { GITHUB_TOKEN = "…" }

[profiles.work.editor]              # brnr acp
experimental = ["send", "permission"] # actions on the editor's session
client_capabilities = ["fs"]        # the editor's that reach the agent: "fs", "terminal"
echo_as = "tool_call"               # or "user_message_chunk": how a message is shown
features = ["shared_sessions"]      # process management
```

Without `--profile`, the `default` profile applies, if there is one. A key in
the wrong part, an unknown key and an unknown name fail to load, saying where
the key goes; `brnr doctor` checks every profile.

The file names the commands brnr runs and can hold secrets, so brnr reads it
only if it is yours and only you can read and write it, and its folder is
yours and only you can write it: `chmod 600` the file, `chmod 700` the
folder. Otherwise every command that reads it fails, saying which path, why,
and the `chmod` or `chown` that fixes it; brnr doesn't change it for you. A
symlink is followed, and the file it points at, and its folder, are checked
([ADR 9.4](docs/adr/0009-security-and-privacy.md#94-the-config-file-is-read-only-if-only-you-can-change-or-read-it)).

## Bridges

A bridge is any process that speaks brnr's JSON-lines protocol: one started by
the brnr process from the profile (requests on its stdout, events on its
stdin, its stderr in the host log; a bare command found next to `brnr` first,
as an agent's is), or anything that connects to the control socket. Its
environment has `BRNR_PID` and `BRNR_SOCKET`.

Requests: `status`, `logged`, `send`, `cancel`, `queue`, `subscribe`,
`was_cut_off`, `pending`, `allow`, `reject`, `set_config`, `fork`, `new`,
`resume`, `close`, `stop` (see the [request and response reference](docs/interface.md#bridge-and-control-socket-protocol)); those about a session name it by
its exact id, and those that act on an editor's session are
[experimental](#experimental-actions), as the CLI's are. Events:
[as above](#events). A started bridge gets them from the process's start.

A started bridge lasts until its process exits; its stdout closing only means
it has no more requests. When the process stops, the bridge gets the last
events (`process_exited`), then its stdin closes; one still running 2 seconds later
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

A burst bigger still (a thought and a message each over 16 MiB, to a
watcher that asked for `acp`) can cut off a peer that keeps up. brnr's
commands catch up: `event watch`, `event notify`, `event wait` and the
`--wait`s, cut off, ask the process whether it cut them off
(`was_cut_off`), subscribe again, and read what they missed from the
transcript, by the events' positions, before going on live; a peer cut off
is a `peer_cut_off` event to the others. Where the transcript can't have
it all (`log = "off"`, `log = "events"` for `acp`, records skipped), or
when cut off 4 times in a row while catching up, the command says what it
missed, an `events_missed` note (`after_seq`, `until_seq`, `error`), and exits 3.
Started bridges are cut off as before (ADR 10.2).

However long the lines: the session's pipes hold back past 16 MiB queued,
and brnr reads a line of up to 32 MiB, held beside that while it waits for
its newline. A longer one goes on as it comes,
unread and unrecorded, to the editor or the agent unchanged, or is dropped
headless, and a `line_too_long` event says so. The agent's stderr
goes on as it is written, newline or not, 64 KiB at most at a time, every
byte.

However deep the lines: brnr reads a line nested up to 128 levels (`[[1]]`
is 2), about what serde_json, and so ACP's Rust SDK, reads. A deeper one is
treated as a long one: it goes on unread and unrecorded with an editor, is
dropped headless, and a `line_too_deep` event says so; brnr tracks nothing
of it, so a permission request that deep isn't listed or answerable from
outside. brnr's commands read what brnr itself wrote (answers, events,
transcripts) up to 256 levels, and say so, and fail, past that (ADR 2.5).

## Transcripts

Like the agents' own transcripts, keyed by project folder:

```text
~/.brnr/projects/<folder>/<session id>.jsonl       # a session's events
~/.brnr/projects/<folder>/<session id>.acp.jsonl   # its raw ACP
~/.brnr/hosts/<run id>.jsonl                       # each process's, for what belongs to no session
~/.brnr/sessions/<session id>.lock                 # held by the process serving the session
```

This layout, how `~/.brnr` and the runtime directory are found, and how a cwd
and a session id become names (below) are fixed for 1.x (ADR 3.9); what the
host logs and the locks contain isn't (ADR 8.4).

`<folder>` is the session's cwd with every non-alphanumeric character replaced
by `-`, as in `~/.claude/projects`, then `-` and 8 hex digits, a hash of the
cwd (`-Users-me-src-3db3a304` for `/Users/me/src`), so `/a-b` and `/a/b` have
a folder each. A claude-agent-acp session's events file has the same name as
Claude Code's own transcript, in a folder whose name starts with Claude
Code's. A transcript an earlier brnr wrote in a folder without the hash stays
there, and isn't read (below); a session resumed goes on in its cwd's folder
(ADR 9.2). One resumed in another cwd (`--cwd`) so has a file in each
folder, and they are one transcript: `event log` reads them all, merged by
time, `session list` has one row of it, and `session resume` takes the cwd,
agent and profile of the newest record of them all (ADR 5.1). In
`<session id>`, as in a lock's name, every byte but lowercase ASCII letters,
digits, `-` and `_` is `%` and two hex digits (`a/b` is `a%2fb`), so two
sessions never share a file. The events are the ones bridges get, `process_exited`
included, and are what `brnr event log`, `session list` and `session resume`
read; `event log` reads the raw file too when `acp` events are chosen. A
thread of the process's own writes them, so the session never waits for the
disk; `event log` of a running session first waits until its process has
written what it recorded until then (up to 5 s, then it says so and shows what
is there), so a turn `session new --wait` just reported is in it, and a
process that exits is listed until its transcript has its `process_exited` (2 s at
most). A process killed (SIGKILL) loses what it hadn't written yet, and may
leave its last record cut short: the session is still listed, logged and
resumed as of its last whole record, `event log` says on stderr which lines it
couldn't read, and the next process starts on a new line; brnr never rewrites
a transcript. Every record carries `ts`, `host_id`, `pid` (the brnr
process), `proxy_pid`, `agent_pid` and, in a session's file, `session`, for
joining: `proxy_pid` is null for a headless process, and the
`session_deleted` that `session delete` records is no process's, its
`host_id`, `pid` and `proxy_pid` null. A session's events file starts with
`session_opened`, as the host log records it too: the `session`, its `cwd`,
its `events_log`, `acp_log` and `host_log`, and `forked_from` for a fork. A
transcript an older brnr wrote, its records naming the session `session_id`
and the process `host_pid`, isn't read: brnr reads only its own names (ADR
5.9). `session list` and `session resume` pass it over, `event log` of it
says an older brnr wrote it, and `brnr doctor` lists it. Nothing is appended to
it: a session opened where one is in the way renames it `<name>.older.jsonl`
(`<name>.older.acp.jsonl` for its raw file), which the host log's
`session_log_moved` says, and starts a file of its own. `log = "events"` leaves out the
raw file, most of the space; `log = "off"` writes nothing but the session
locks.

Records skipped behind a slow disk are counted, and a `records_skipped` note
(`count`, `acp_count` of them raw ACP, `since`, `until`, and the positions of the
events among them, `first_seq` and `last_seq`, with `ts`, `host_id`, `pid`
and `session` as an event has them) marks the gap in the host
log and in each session's events file that lost some. Records a full disk
didn't take are noted likewise, with the `error`, in the file that lost them
(a raw file's in its events file) once it takes records again. How a process
ended (`process_exited`, or `process_panicked`) is never skipped, so `brnr doctor` can tell an
exit from a death. A process that exits waits for its records to be written,
however long the disk takes, once it is no longer listed and has let go of
its sessions; a signal (Ctrl-C, `kill`) gives up on them, and it says on
stderr how much it didn't write. `brnr event log` always shows a gap,
whatever `--events` chose.

The values of MCP servers' `env` and `headers` are recorded as
`"<redacted>"`, in the raw ACP, the host log and `acp` events alike; the agent
gets them as given.

Transcripts hold prompts and tool output, so brnr writes them only where only
you can read them (ADR 9.3): `~/.brnr` (or `$BRNR_HOME`), each directory below
it and the file must be yours, with no group or other access, before a record
goes in, whether brnr made them or found them there. It creates them 0700 and
0600; one restored, copied or chmod'ed so that others can reach it is made
private when a process opens it, and the host log has a `path_made_private`
record (`path`, and the `mode` it had; `session delete`, which has no host
log, says so on stderr). Symlinks are never followed, the
state directory included (set `BRNR_HOME` to where one points instead), and
what isn't your own directory or file, or has another hard link, is refused:
a process whose host log can't be opened doesn't start (`log: <path>: …`),
and a session whose file can't be is served without it, with a
`session_log_failed` record in the host log saying why. What reads them
(`session list`, `event log`, `doctor`, …) keeps the same rules: nothing is
read through a symlink, nor what isn't yours or has another hard link. There
is no override; `log = "off"` writes no transcripts.

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
(their metadata and sockets); that nothing keeps a session from being locked,
and no lock is left by a process that is gone; that transcripts are readable
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
without recording it, killed by SIGKILL for instance: logs without `process_exited`
whose process isn't running. The line says how many, and for the latest three
when each last wrote and the sessions it had open; it is information, not a
warning. So is its list of the files an older brnr wrote, transcripts and
host logs, which this one doesn't read (ADR 5.9). `--fix` makes the runtime directory and transcripts private (only
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
that runs detached, by `brnr session new` or `brnr acp` when its start fails with
the panic, and in the foreground by the process itself.

## Environment

| Variable | Default |
|---|---|
| `BRNR_HOME` | `~/.brnr` (transcripts and session locks) |
| `BRNR_CONFIG` | `$XDG_CONFIG_HOME/brnr/config.toml`, else `~/.config/brnr/config.toml` (brnr refuses one others can read or write) |
| `BRNR_DIR` | `$XDG_RUNTIME_DIR/brnr`, else `$TMPDIR/brnr-<uid>` (sockets and metadata; brnr refuses one others can use) |
| `BRNR_START_TIMEOUT` | `120`: seconds a start has to commit |

## Releasing

Generating release notes requires Git 2.13 or newer, so a final release's
comparison can exclude release candidates (ADR 11.4).

```sh
./release.sh minor     # or patch, major, 1.2.3: checks main, opens the release pull request
./release.sh tag       # once it's merged: tags, releases, updates the Homebrew tap
./release.sh notes     # what the release pull request would say
./release.sh 1.0.0-rc.1  # a release candidate; then rc for 1.0.0-rc.2, and 1.0.0 to release it
```

A release's notes are its section of [CHANGELOG.md](CHANGELOG.md), in
[Keep a Changelog 1.1.0](https://keepachangelog.com/en/1.1.0/) form, which
pull requests add to under `[Unreleased]`. `./release.sh minor` names that
section for the version on the release branch and moves its compare links,
`./release.sh tag` won't tag a version without one, and the release workflow
publishes it as the GitHub release's notes, or fails (ADR 11.4).

A release candidate, `X.Y.Z-rc.N`, is a GitHub pre-release and is on
crates.io (`cargo install brnr --version X.Y.Z-rc.N --locked`), but the
Homebrew formulae stay at the last release. `X.Y.Z`'s section of
CHANGELOG.md takes in its release candidates' sections, and its pull request
lists the commits since the last release, so its notes say everything since.
A version must come after the current one (ADR 11.4).

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
