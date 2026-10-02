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
