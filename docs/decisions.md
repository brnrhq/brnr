# Decision log

Decisions made while building out the headless CLI (the functional review:
seeing what happened, waiting, resuming, steering, approving with
context). Each entry lists the options considered and what was chosen, so
they can be revisited.

## 1. Where `log` and `watch` get their story from

The transcript records raw ACP; a readable history needs the same
interpretation the host already does (assembling agent messages, pairing
permission requests with answers, naming the turn's end).

- **A. Render raw ACP in the CLI.** Works for any transcript, but duplicates
  the host's interpretation in a second place, and the CLI can't know the
  host's names for things (permission handles like `p1`, message ids).
- **B. The host writes its events into the transcript too.** Every event it
  emits to bridges (`user_message`, `agent_message`, `tool_call`,
  `permission_request`, `turn_ended`, …) is also a record in the session's
  file. `log`, `watch` and `brnr host` render the same events.

**Chosen: B.** One interpretation, one renderer, and a transcript you can
read back is the same stream a bridge saw live. Cost: agent text is stored
twice (chunks in the raw ACP, and the assembled message). Transcripts written
before this change have no events, so `log` falls back to saying so and
`log --raw` still shows them.

## 2. `watch` default

- **A. Keep subscribing to everything, `acp` included** (one raw line per
  streamed chunk).
- **B. Default to the readable events; `--raw` adds the ACP stream.**

**Chosen: B.** The default should be readable; the firehose is one flag away.

## 3. Turns and message ids

`send --wait` must know which turn answers *its* message, even when the
message is held, sent mid-turn, or queued behind an interrupt.

- **A. Match on text.** Fragile.
- **B. Every message sent through the host gets an id (`m<n>`) when it is
  accepted.** The id is in the `send` response, in `user_message` when it is
  actually sent, and in the `turn_ended` of the prompt that carried it.

**Chosen: B.**

## 4. How `brnr start` delivers its prompt

- **A. As now: `--prompt` goes to the host, which sends it once the session
  opens.**
- **B. `start` waits for the session, then sends the prompt over the control
  socket like `brnr send`.**

**Chosen: B.** `start --wait`, `--file` and `--image` then share `send`'s
code, `start` can subscribe before sending so it never misses the reply, and
a `start` that dies before sending simply sends nothing. `brnr host --prompt`
(in the foreground) keeps the host-side path.

## 5. `wait` conditions and exit status

`brnr wait <target> [--for idle|turn|permission|exit] [--timeout <s>]`.

- `idle` (default): no turn running and nothing held. Returns at once if
  already idle, so `send` followed by `wait` can't race.
- `turn`: the next turn that ends.
- `permission`: a permission request is waiting (at once if one already is).
- `exit`: the host exits.

Exit status: 0 when the condition is met and the last turn ended normally
(`end_turn`), 1 if that turn failed or stopped for another reason, 124 on
`--timeout` (as timeout(1) does). The same codes for `send --wait` and
`start --wait`.

Considered: separate commands per condition (`wait-idle`, …); one command
with `--for` is smaller.

## 6. What `cancel` does with held messages

- **A. Cancel the turn; held messages then go out as usual.** Surprising:
  "stop" would start the next queued message.
- **B. Cancel the turn and pause the queue.** Adds a paused state to explain.
- **C. Cancel the turn and drop held messages, listing them.**

**Chosen: C**, with `--keep-held` for A. Nothing is lost silently: the
dropped messages are printed.

## 7. Permission details command

- **A. `pending -v`.**
- **B. `brnr show <target> [<request>]`.**

**Chosen: B**, for one request in full (tool, kind, paths, the command or
input, and the diff for edits), with `pending` staying a table. Diffs are
rendered from the tool call's `oldText`/`newText` with a small line diff;
very large ones fall back to showing the new text.

## 8. Human status

`brnr status` prints a summary by default (owner, each session's title,
mode, model, whether a turn is running and for how long, running tools,
plan progress, held messages, pending permissions, token usage, last agent
message); `--json` keeps the raw report. The host tracks the agent's
`session/update`s to have this.

## 9. Mode, model and other settings

Both adapters expose mode through `session/set_mode` and everything else
(model, effort, …) as config options (`session/set_config_option`).

- `brnr mode <target> [<mode>]` lists or sets the mode (`set_mode`, or the
  `mode` config option when the agent has no modes).
- `brnr config <target> [<option>=<value>...]` lists or sets config options.
- `brnr model <target> [<model>]` is `config` for the option whose id or
  category is `model`, falling back to `session/set_model` (codex) if there
  is no such option.
- `start` and profiles take `mode` and `config` to apply before the first
  prompt; failing to apply them fails the start (an explicit request that
  wasn't met).

Allowed while an editor is attached: the agent tells the editor about the
change (`current_mode_update`, `config_option_update`), so it stays in sync.

## 10. Resuming

`brnr start --resume <session>` (an id or unique prefix from
`list --inactive`).

- Uses `session/resume` when the agent offers it (no history replay), else
  `session/load`. While a load replays history, the host neither records nor
  emits the replayed updates: they are already in the transcript.
- The cwd, agent and name default to the session's last host (from its
  transcript and host log); `--cwd` or `-- <agent>` override.
- Refused if a running host already serves the session.

## 11. More sessions in one host

- `brnr sessions <target>`: the agent's own session list (`session/list`),
  which can include sessions brnr never saw.
- `brnr fork <target>`: forks the session (`session/fork`) into a new one in
  the same host and prints its id.
- `brnr close <target> --session <id>`: closes a session; a headless host
  with no sessions left stops.

Fork and close are refused while an editor is attached: they would create or
remove sessions behind the editor's back. A plain "new session in this host"
is not offered; `brnr start` is the way to get one.

## 12. Permission rules

- **A. Keep `permissions = "ask" | "auto-allow" | "auto-deny"`.**
- **B. Also accept a table by tool kind**:
  `permissions = { default = "ask", read = "auto-allow", search = "auto-allow" }`,
  using ACP's tool kinds (read, edit, delete, move, search, execute, think,
  fetch, switch_mode, other).

**Chosen: B**, with the string form meaning `{ default = <it> }`.
`brnr start --permissions <policy>` overrides the default for one session.

Timeout: `permission_timeout = <seconds>` denies a request nobody answered
in time. Only deny: allowing on timeout would approve whatever an
unattended agent asked for.

## 13. MCP servers for headless sessions

`[[profiles.<p>.mcp_servers]]` with `name` and either `command`/`args`/`env`
(stdio) or `url` (`type = "http"` or `"sse"`, optional `headers`), passed in
`session/new`, `session/resume`, `session/load` and `session/fork`. HTTP and
SSE servers are refused at start if the agent doesn't advertise support.

## 14. Stopping when done

`stop_when_idle = true` in a profile, or `start --stop-when-idle`: once a
turn has ended and nothing is running or held, a headless host stops. It
never applies while an editor is attached, and not before the first turn.

## 15. Rich input

- `--file <path>`: a `resource_link` to the file (the agent reads it
  itself; always allowed by ACP).
- `--image <path>`: an `image` block, base64-encoded, refused unless the
  agent advertises image prompts.

Considered: embedding file contents (`resource`), which needs the agent's
`embeddedContext` capability and duplicates what the agent can read anyway.

## 16. Authentication

The host doesn't run `authenticate`: the adapters' methods log in through a
terminal or browser, which a headless host has neither of. When the session
fails because the agent needs a login, the start fails with the agent's auth
methods and a hint to log in with the agent's own CLI first.

## 17. Notifications

- **A. Ship scripts for a few services.**
- **B. `brnr notify [<target>] [--events …] -- <command> [args...]`**: runs
  the command once per event, with the event in the environment
  (`BRNR_EVENT`, `BRNR_TEXT`, `BRNR_TITLE`, `BRNR_SESSION`, `BRNR_REQUEST`,
  `BRNR_HOST`) and as JSON on its stdin. Without a target it uses
  `$BRNR_HOST`, so it works as a profile bridge.

**Chosen: B.** Values go in the environment, never substituted into the
command line, so an agent's text can't inject arguments. Default events:
`permission_request`, `turn_ended`, `exited`.

The variables have since become `BRNR_SESSION_ID`, and `BRNR_PID` for
`BRNR_HOST` (see 37); `BRNR_MESSAGE` has the session's last agent message.
The agent's text in them is escaped as `watch` shows it and cut at 32 KiB, so
the command always starts; the event on stdin is whole.

## 18. Tool call events

A `tool_call` event goes out when a call starts (`started: true`) and on
every change of its status, so bridges see `in_progress` too. `watch`, `log`
and `brnr host` show only the start and the end (`tool done` / `tool
failed`); a line per status change was noise. Progress updates that don't
change the status (streamed output) are not events.

## 19. What `log`, `watch` and `status` leave out

`usage` events aren't shown in `watch` or `log` (one per turn, and `status`
has the latest); `session_changed` for config and commands isn't either
(`config` and `commands` show the current values). Thoughts are hidden unless
`--thoughts`. All of them are in `--json`.

## 20. `log --last <n>`

Counts messages sent to the agent (`user_message` events), and shows from the
n-th last one on. Considered: counting turns (`turn_ended`), which leaves out
a turn still running, the one you most likely want to see.

## 21. Output of the `--wait` commands

`start --wait` prints `started …` on stderr, so that stdout is the agent's
reply and nothing else (`brnr start --wait … > answer.md`). If `start` can't
send the prompt after the session opens, it stops the host rather than leave
a session nobody asked anything of.

## 22. Requests the agent answers

`set_mode`, `set_config`, `set_model`, `sessions`, `fork` and `close` are
answered when the agent answers, on the same connection (matched by
`req_id`); the host doesn't block meanwhile. `brnr` waits up to 120 seconds
for them, since switching model or forking can take a while.

## 23. `notify` runs one command at a time

Commands run in order, each waited for, with their stdout sent to stderr
(when `notify` is a bridge, its stdout is read by the host as requests). A
command that hangs makes `notify` fall behind, and the host then disconnects
it like any other slow subscriber; considered running them concurrently,
which would reorder notifications.

## 24. Not done: steering

claude-agent-acp advertises a `_session/steering` request for adding to a
running turn. `send` while a turn runs still sends another
`session/prompt`, which that adapter folds into the turn anyway, and which
works with any agent. Steering could replace it for agents that offer it.

## 25. How far behind a peer may fall

First a queue of 4096 lines per peer. On a slow CI runner a `watch --raw`
that was reading the whole time fell more than 4096 lines behind an agent
streaming 20,000 chunks at once, and was cut off as if it had stopped.

- **A. More lines.** Still a count, and lines vary from a few bytes to
  megabytes.
- **B. Drop `acp` events for a slow peer instead of disconnecting it.**
  Lossy where `--raw` promises everything.
- **C. A limit in bytes, 16 MiB per peer**, counted from when a line is
  queued until its writer has written it.

**Chosen: C**, counted as: a peer is behind once what it hasn't written is
past the limit; until then it takes the next line however big. First it was
"the next line would take it past the limit", which cut off peers that were
keeping up when one big agent message (10 MB, at the end of a burst) came
while they were a few MB behind (seen on the macOS CI runner); and before that,
one message over the limit cut off every peer, `brnr status` included. The status report now
quotes at most 4000 characters of the last message; the events have all of it.

## 26. Adapters through Homebrew

claude-agent-acp compiles in the Claude Agent SDK (Anthropic's Commercial
Terms), so the tap doesn't ship prebuilt adapters.

- **A. Tell Homebrew users to install the npm packages.** Needs Node 22 on
  the editor's PATH, and the packages likely bring their own copies of the
  agents through optional dependencies (`adapters/build.sh` leaves those out).
- **B. A `brnr-adapters` formula that compiles them on the user's machine**
  from brnr's `adapters/` at the same tag, pinned by `bun.lock`.

**Chosen: B**, with the npm packages mentioned as the alternative. Nothing
prebuilt is distributed; the user's machine fetches and compiles the
packages, as `adapters/build.sh` does for anyone building from source.

Compiler: **bun** rather than deno. `adapters/build.sh` already uses `bun
build --compile` with a frozen lockfile, its output is what brnr's tests and
smoke tests ran against, and both adapters are Node packages written for
Node; deno's npm compatibility would be one more thing to verify for code
that spawns and talks to subprocesses. Both are in homebrew-core; bun is a
build-only dependency.

## 27. Finding adapters next to a symlinked brnr

Homebrew links `bin/brnr` and the adapters into its prefix's `bin`. On macOS
the running binary's path is the link's, so an adapter next to the link is
found; on Linux it is the link's target in the Cellar, where the adapters
aren't. brnr now also looks next to the path it was started by (`argv[0]`,
when it is a path), and passes that path on to the host, which is the one that
starts the agent.

## 28. The adapters' command names

First the compiled adapters took the npm packages' command names
(`claude-agent-acp`, `codex-acp`), so docs and configs read the same either
way. Installed through Homebrew next to a global npm install, though, two
commands of the same name are on PATH, and which one runs depends on PATH
order.

**Chosen: names of their own, `brnr-claude-adapter` and `brnr-codex-adapter`.**
`brnr acp -- claude-agent-acp` still runs the npm package. `brnr doctor`
reports both kinds.

## 29. One Homebrew formula per adapter

`brnr-adapters` built both adapters. Replaced, before it was ever released,
by `brnr-claude-adapter` and `brnr-codex-adapter` (named for the command each
installs), in the same tap.

- Install what you use: each is about 60 MB and its own build.
- Licenses that fit: only the Claude adapter compiles in the Claude Agent SDK
  (`license :cannot_represent`, with a caveat); the Codex adapter is
  Apache-2.0.
- One adapter's upstream breaking doesn't stop the other installing, and a
  third agent's adapter is one more formula.

Considered: a tap per adapter. A tap is a repository of formulae; more of
them means more `brew tap`s for users and more release plumbing, for nothing
a formula per adapter doesn't give.

## 30. Which npm version an adapter was built from

- Each adapter formula's version is its npm package's (`brnr-claude-adapter
  0.85.1`), written by the release workflow from `adapters/package.json`. A
  brnr release that doesn't move the pin changes the formula's source but not
  its version, so nobody rebuilds for nothing; `brew outdated` shows an
  adapter when its package does move. When adapters/ changes without a new
  npm version, `release.sh` says so: bump the formula's `revision` by hand if
  users need the new build. The release workflow drops the revision when the
  npm version next moves, since a new version starts the count over.
- The adapters say what they were built from (`--version`, compiled in), and
  `brnr doctor` shows it, and the version of npm-installed adapters (from the
  package.json their bin link leads to). `brnr status` shows what the agent
  says it is in `initialize` (`agentInfo`).
- Dependabot opens a pull request for new releases of the two adapter
  packages, weekly. Considered: a release per upstream release; batching
  them into brnr's releases keeps the pace a person's.

## 31. `release.sh`

Two steps, because main only changes through reviewed pull requests:
`release.sh <bump>` lints and tests main and opens the release pull request
(with the changes and the adapters' versions); `release.sh tag`, after the
merge, checks CI passed on main, tags, follows the release workflow and checks
the tap. Considered: one command that pushes the bump to main and tags, which
skips review; and doing it all in a workflow (`workflow_dispatch`), which
works but is harder to run and debug than a script you can read.

## 32. `brnr acp`, not `brnr proxy`

What an editor runs as its agent was `brnr proxy`, named for how it works (it
relays bytes to the host). `brnr acp` names what it is to the editor: an ACP
agent command (`brnr acp -- brnr-claude-adapter`). `brnr proxy` is gone, not
kept as an alias (see 33): editor configs written for 0.2.0 need the new
name. Inside, the process is still the proxy (`proxy.rs`, and `proxy_pid` in
metadata and transcripts).

## 33. No backwards compatibility before 1.0.0

Until 1.0.0, a change of name, flag, format or behaviour is made outright:
no aliases for old names, no shims for old formats. Release notes say what
changed. From 1.0.0 on, compatibility is kept within a major version.

## 34. `log` takes `watch`'s flags; `--events` alone chooses events

`log --raw` printed the transcript's records as stored, and `--json` the
host events in them, so the two didn't go together, and `--raw` meant
something else than for `watch` (the ACP messages too). `log` had no
`--events`.

- **A. Keep the record dump; add `--events` over the records.** `--raw`
  keeps two meanings.
- **B. `log` reads a record as the event `watch` would have shown:** an ACP
  message becomes an `acp` event. `--events` and `--json` then mean the same
  for both, and `log --json` is what a watcher got live.

**Chosen: B.** The stored records are still in the file (`brnr list --json`
names it); `log` no longer prints them as they are (see 33).

That left `--raw` meaning "every event, `acp` included", which is a choice
of events: it was `--events all` under another name (the host's `subscribe`
already took `"all"`), and was ignored next to `--events`. So `--raw` is
gone and `--events` takes `all`.

`--thoughts` was the same kind of flag: thoughts were always chosen, and the
text output dropped them without it, so `--events agent_thought` alone
printed nothing, and `--json` had them where text didn't. Now it is gone
too, and `--events` alone chooses: an event chosen is shown. Without
`--events`, `watch` and `log` show every event but the quiet ones, `acp`
and `agent_thought` (`QUIET`, in text and JSON alike, and in `brnr host`'s
foreground output), and `default` names that set, so `--events
default,agent_thought` adds the thoughts. Bridges and the host's
`subscribe` keep their own default: every event but `acp`.

`notify` reads `--events` the same way (`events_arg`): names, `all`, and
`default`, which is each command's own default (for `notify`,
`permission_request`, `turn_ended`, `exited`), and unknown names fail in
the CLI, before connecting, with the same message.

## 35. Host-wide events in every session's transcript

`log` reads one file, the session's. Events with no session went only to the
host log, so `log` never showed `owner_changed` (the host taking over from
the editor, which decides who answers permissions), while `watch` did.
`exited` was already copied into each session's file.

- **A. `log` also reads the host log** for the session's time span. Two
  files to merge and to follow, and the host log has ACP and stderr that
  aren't the session's.
- **B. The host writes such events into every session's file too**, as it
  did for `exited` (`emit_to_sessions`).

**Chosen: B.** `log` keeps to one file. The host log's other records (ACP
before a session exists, agent stderr) stay there. The host log also had the
take-over twice, as a hyphenated `owner-changed` note and as the event; the
note is gone, and the one record of a headless start is `owner_changed` now.

## 36. `list` and `sessions` stay apart; `--resume` takes the agent's ids

`brnr list` is brnr's view of the machine: running hosts and the sessions it
has transcripts of, in any cwd, with no agent to ask. `brnr sessions` asks
one host's agent (`session/list`, in that host's cwd), which knows sessions
started outside brnr.

- **A. One command.** `list` would have to start an agent for every agent
  and cwd to ask it, and not every agent can list.
- **B. Keep both, and join them where they meet.** `sessions` has a BRNR
  column (`running (<host>)`, `inactive`, `-`), and `start --resume` takes
  an id brnr has no transcript of: it goes to the agent as given, in
  `--cwd` (or here), with the agent after `--` (or the profile's). Before,
  `sessions` listed sessions `--resume` refused.

**Chosen: B.** An id brnr does know still brings its cwd, agent and name
(see 10); a prefix only matches what brnr knows.

The usage groups the rest by what they are about: "chat" (`send`, `wait`,
`cancel`, `queue`), "approvals", "sessions" (`list`, `status`, `sessions`,
`fork`, `close`: brnr's sessions and the agent's, side by side), "events"
(`log`, `watch`, `notify`: the events so far, live, and a command per
event), "settings" (`mode`, `model`, `config`, `commands`) and "brnr"
(`doctor`, `--version`), and says once, at the end, what a `<target>` and
`--session` are. `stop` is with the commands that start a host (`acp`,
`start`, `host`): it ends one, however it started.

## 37. Commands take a session, or a process by pid

A `<target>` was a host id (the host's pid), a host's `--name`, a session id
or a unique prefix of one, and `--session` then picked a session inside the
host, or brnr picked the only one. Too many meanings, and the picking hid
that settings and messages belong to a session until a `fork` broke it.

- **A. Keep `<target>`, document it better.**
- **B. Two arguments, each meaning one thing:** `<session>`, a session's
  exact id, for everything about a session; and `--pid <pid>` (or a `<pid>`
  argument), a brnr process as `brnr ps` shows it, for what is about the
  process.

**Chosen: B.** No prefixes, no `--session`, no picking. `watch` and
`notify` take one or the other and have no default. Bridges stay per
process: a started bridge's environment has `BRNR_PID` instead of
`BRNR_HOST` (sessions come and go in a process, and it has none when its
bridges start; each event names its session), so as a bridge `notify` is
run as `sh -c 'exec brnr notify --pid "$BRNR_PID" -- …'`. "Host" leaves the
CLI and its help; it stays the name of the process inside brnr.

## 38. No names: a session is its id

Sessions had names for a while: `start --name`, `fork --name`, `brnr name`
for any session, at most one running session per name, a name meaning the
latest session that had it (the running one, else the one most recently
started or resumed with it), the name coming back on `--resume` unless in
use, and an index of it all under `~/.brnr`.

**Chosen: none of it.** It was brnr's own layer over what ACP has, and a way
of working it imposed: a policy to learn (which session a name means, when
one is taken, what resuming does to it) rather than a description of the
agent's sessions. `<session>` is the id the agent gave the session, running
or not; `start --json` prints it for a script to keep
(`s=$(brnr start --json … | jq -r .session)`), and anyone who wants shorter
handles can keep them in their shell. The agent's own title for a session
(`session_info_update`) is shown in `list` and `status`, to tell sessions
apart, and is never something to type.

## 39. `ps` and `stop`

`brnr ps` lists brnr's processes: pid, agent, owner (`editor` or
`headless`), cwd, uptime and its sessions. `brnr stop <pid>` stops a process
and its agent, as `stop` did. Ending one session is `brnr close <session>`;
a headless process with no session left stops. `list` is about sessions,
`ps` about processes.

## 40. No `on_disconnect`: an editor's agent goes with the editor

`--on-disconnect direct|headless` and the profile's `on_disconnect` are
gone, and `direct` is what happens: when the editor goes away, the agent's
stdin is closed and it is stopped, as if the editor had run it. To carry
on with the session, `brnr start --resume <session>`; the agents keep their
sessions. Lost: a turn still running when the editor closes. Gone with it:
the host taking over (`go_headless`, taking over the editor's pending agent
requests) and the `owner_changed` event (see 35); a process's owner is set
when it starts.

## 41. `stop_when_idle` takes seconds

`--stop-when-idle <s>` on `start` (and its `--foreground`), and
`stop_when_idle = <s>` in a profile, replace the switch (see 14). A session
is idle while no turn is running, nothing is held and no approval is
waiting; a message or prompt starts the count again, and it counts from the
start too, so a `start` without a prompt doesn't run forever. A `start` with
a prompt sends it over the socket once the session is open, so the process
is told one is coming (`--awaiting-prompt`) and counts from when it arrives.
When it runs out the session closes (`session/close`), and the process
stops with its last session; an agent that can't close sessions keeps one
that ran out while it has others. `0` is as soon as it's idle, what the
switch did.

## 42. Same data in text and JSON; `list` is an index

`list --json` gave every host's whole status report while the text showed
six columns, and `status --json` was the raw report behind a summary (see 8).

**Chosen:** a command's text and JSON carry the same data. `list` is an
index, one row per session: id, title (the agent's, while it runs), state
(`busy`, `idle`, `waiting` for an approval, `inactive`), pid, agent, cwd and
when it was last active.
`status <session>` is the detail, one session's, as a flat object. `ps`
follows the same rule. The control socket's `status` stays the full report
(bridges use it); brnr shapes what it prints from it.

## 43. `sessions` without a running agent

`brnr sessions [--profile <p>] [--cwd <dir>] [-- <agent>]` starts the agent
just to ask it (`initialize`, `session/list` for the cwd, then it exits): no
process of brnr's, no target, and the folder is explicit, here unless
`--cwd` says otherwise. The BRNR column stays (see 36). The control socket's
`sessions` request is gone with it.

## 44. `--json` wherever a command prints data

Only `list`, `status`, `watch` and `log` had it. Now every command that
prints data does (`start`, `send`, `wait`, `cancel`, `queue`, `pending`,
`show`, `approve`, `deny`, `ps`, `sessions`, `fork`, `mode`, `model`,
`config`, `commands`, `doctor`): one JSON value, or one event per line for
`log`, `watch` and `start --foreground`; errors stay on stderr with the same
exit status. Commands whose answer is their exit status (`stop`, `close`,
`notify`) don't. The usage says it once, at the end, rather than on
every line. `--wait` with `--json` prints the turn as one object at its end:
`{session, message, reply, stop_reason, error}`, and for `start` its
`pid`.

A usage error prints that command's lines of the usage, not all of it.

## 45. `start --foreground`, not `brnr host`; approvals name their request

`brnr host` was the foreground session by hand, and the process `acp` and
`start` start. Its flags lagged `start`'s (no `--file`, `--image`, `--model`,
`--set`, `--permissions`, `--wait`).

**Chosen:** `start --foreground [--quiet]` runs the same process as `start`
as its child, in a process group of its own, passing it the signals it gets
(Ctrl-C once stops the session, twice kills the agent), shows the session as
it goes (`--json`: as JSON lines), and exits as the agent did; for a
supervisor such as systemd, or a container. `brnr host` stays, for brnr
itself and by hand, out of the usage. `--wait` and `--foreground` don't go
together: the foreground shows the whole session.

`show`, `approve` and `deny` take `<session> <request>`: brnr no longer
picks the only request waiting, as it no longer picks a session (see 37).
`pending [<session>]` keeps its filter, and `--option` its default (allow, or
reject, once).

## 46. One row shape for session tables; `ps` is about the process

`list` and `sessions` each had their own columns (`UPDATED` vs `LAST
ACTIVE`, a `BRNR` column folding state and pid into one cell, timestamps
formatted two ways) and their own order (hosts as discovered vs the agent's
order). And `ps` had a CWD column showing the *process's* cwd — where the
editor happened to start brnr, often `/` — beside `list`'s per-session cwds.

**Chosen:** every table of sessions has the same columns, in the same order,
formatted the same way: SESSION, TITLE, STATE, PID, (AGENT,) LAST ACTIVE,
CWD — `sessions` drops AGENT, which it was given on the command line. The
`BRNR` column (see 36) becomes STATE and PID in `list`'s terms: a running
session's live state (`idle`, `busy`, `waiting`), `inactive` for a
transcript, `-` for one only the agent knows; `last_active` is the agent's
`updatedAt`, or brnr's own when the agent gives none. Rows are ordered most
recently active first, everywhere. `ps` loses its CWD column (sessions have
cwds; the JSON keeps the process's).

## 47. No client-side approval policy; the agent's mode is the policy

brnr had its own permissions policy for headless sessions (`--permissions
ask|auto-allow|auto-deny`, per ACP tool kind in the profile): how the host
answers `session/request_permission` when no editor is attached. It overlapped
the agent's own session mode — Claude's `acceptEdits` or `bypassPermissions`
says when to ask at all — and the two composed confusingly: mode `default`
plus `auto-allow` approved everything one round-trip at a time, `plan` plus
`auto-allow` approved leaving plan mode, and neither setting knew of the
other.

- **A. Keep both layers.** Agent-agnostic rules by tool kind, auditable
  auto-answers in the transcript; but two policies for one question.
- **B. One policy: the agent's.** How much to ask is the agent's mode, set at
  start (`brnr start --mode acceptEdits`, `mode` in the profile, applied
  before the first prompt) or later (`brnr mode`). brnr always asks when the
  agent does: a request waits for `brnr approve`/`deny` or a bridge, and
  `permission_timeout` denies what nobody answers.

**Chosen: B.** The `permissions` profile key, the `--permissions` flag and
the by-kind rules are gone; a config that still has them fails to load (and
`brnr doctor` says so). What brnr does headless is no longer configuration,
so it needs no querying either.

## 48. Injected messages are shown to the editor as a tool call

The editor was shown an injected message (`brnr send`, attached context) as
a synthesized `user_message_chunk`. Editors don't render one that arrives
out of turn: in ACP's model user messages are what the *client* sends, so an
unsolicited chunk mid-turn (or between turns) has no place in their
transcript and is dropped or misplaced.

- **A. `user_message_chunk`.** Semantically honest, but not rendered.
- **B. `agent_message_chunk` / `agent_thought_chunk`.** Rendered, but the
  text appears as the agent's own words.
- **C. A completed `tool_call`.** The one update editors render as a block
  of its own at any point in a turn: title, status, content.

**Chosen: C.** One `session/update` with `sessionUpdate: tool_call`,
`toolCallId: brnr-echo-<n>`, kind `other`, status `completed`, titled
`Message via brnr` (or `Context via brnr`), the message's content blocks as
its content. Only the editor sees it: events, bridges and the transcript's
own story keep `user_message` with `by: control`. The echo still lands at
send time — when the agent takes a mid-turn message up remains its business
(see the acp.rs module notes).
