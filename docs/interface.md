# External interface reference

This reference describes brnr's command-line inputs and outputs, editor
transport, configuration, and bridge protocol. It applies to the source
revision containing it; use the same tag as your installed `brnr --version`.
Before 1.0 these interfaces can change without compatibility aliases; see
[ADR 1, P9](adr/0001-principles.md#p9-no-backwards-compatibility-before-10).
For installation and a walkthrough, start with the [README](../README.md).

## Command syntax

`<name>` is a required value, `[...]` is optional, `...` means repeatable,
and `|` separates alternatives. Do not type the brackets. Session IDs are
exact, opaque strings from the agent, not prefixes or process IDs. Approval
handles such as `p1` come from `pending`; message IDs such as `m1` come from
`send`. A PID identifies the brnr host that can own multiple sessions.

`--` separates brnr options from the agent or notification command and its
arguments. Programs are executed as argument vectors, not shell strings;
invoke a shell explicitly if shell syntax is needed. With no agent after
`--`, the selected profile supplies it. `--profile` defaults to `default`
when that profile exists. `brnr --help` prints the syntax below; `-h` is an
alias, and `-V` aliases `--version`. `brnr host` is internal, not a command
for users to start by hand.

```text
usage:
  brnr acp [--profile <p>] [--strict] [-- <agent> [args...]]
             what an editor runs as its ACP agent

  brnr start [--profile <p>] [--cwd <dir>] [--prompt <text> | -] [--file <path>]...
             [--image <path>]... [--mode <m>] [--model <m>] [--set <option>=<value>]...
             [--stop-when-idle <s>] [--auth <method>] [--resume <session> [--take-over]]
             [--strict] [--wait [--timeout <s>] | --foreground [--quiet]] [--json]
             [-- <agent> [args...]]
             a headless session, in the background (or the foreground)

processes
  brnr ps [--json]
  brnr stop <pid>

chat
  brnr send <session> [--steer | --interrupt | --context [--replace]]
            [--file <path>]... [--image <path>]... [--wait [--timeout <s>]] [--json]
            (<text>... | -)
  brnr wait <session> [--for idle|turn|permission|exit] [--timeout <s>] [--json]
  brnr cancel <session> [--keep-held] [--json]
  brnr queue <session> [--drop <message>] [--clear] [--clear-context] [--json]

approvals
  brnr pending [<session>] [--json]
  brnr show <session> <request> [--json]
  brnr approve <session> <request> [--option <id>] [--json]
  brnr deny <session> <request> [--option <id>] [--json]

sessions
  brnr list [--inactive | --all] [--json]
  brnr status <session> [--json]
  brnr sessions [--profile <p>] [--cwd <dir>] [--json] [-- <agent> [args...]]
  brnr fork <session> [--json]
  brnr close <session>

events
  brnr log <session> [--last <n>] [--follow] [--events <default|all|event>,...] [--json]
  brnr watch (<session> | --pid <pid>) [--events <default|all|event>,...] [--json]
  brnr notify (<session> | --pid <pid> | --stdin) [--events <default|all|event>,...]
              -- <command> [args...]

settings
  brnr mode <session> [<mode>] [--json]
  brnr model <session> [<model>] [--json]
  brnr config <session> [<option>=<value>...] [--json]
  brnr commands <session> [--json]

brnr
  brnr doctor [--fix | --report] [--json]
  brnr skill [<reference> | install [--dir <dir>]...]
             the skill for agents that use brnr: print it, or install it
  brnr --version

<session> is a session's id, as brnr list shows it. <request> is a pending approval's handle,
as brnr pending shows it. <pid> is a brnr process, which runs one agent for one or more
sessions, as brnr ps shows them.
--json prints the same data as the text: one JSON value, or one event per line for log, watch
and start --foreground. --strict is stable ACP only: no --steer into a running turn, no fork.
```

## CLI inputs and results

Normal commands write results to stdout and diagnostics to stderr. On
commands supporting `--json`, it selects structured data instead of the
human-readable table or summary. It does not convert usage errors or
ordinary diagnostics into JSON. Parse JSON, not human-readable columns.
Unknown or unavailable values may be null; agent-supplied objects vary
with the agent's capabilities.

| Command | Input and effect | Output |
|---|---|---|
| `brnr acp` | Editor ACP on stdin; launches the selected agent. | Agent ACP on stdout; diagnostics and agent stderr on stderr. See [editor transport](#editor-transport). |
| `brnr start` | Opens/resumes a headless session; sets cwd, authentication and settings before an optional prompt. | Startup summary; JSON object with `session`, numeric `pid`, and `message` (null without a prompt). |
| `brnr ps` | Lists discovered running hosts. | Table or JSON array of `pid`, `owner`, `agent`, `sessions`, `uptime_seconds`, `cwd`. An unresponsive host has owner `unreachable`. |
| `brnr stop` | Stops the specified host and its agent; affects all its sessions. | Stopping acknowledgment; errors on stderr. |
| `brnr send` | Sends text/attachments to one session. Positional words form the prompt; `-` reads it from stdin. | Acknowledgment with `session`, `status`, and `message`; context-only submissions have no message ID. Status is `delivered`, `held`, `steered` or `interrupting`. With `--wait`, a completed-turn result instead. |
| `brnr wait` | Waits for `idle` (default), `turn`, `permission`, or process `exit`. | Condition summary or JSON for the condition/last turn. See [exit codes](#exit-codes). |
| `brnr cancel` | Cancels the turn; drops held messages unless `--keep-held`. | Acknowledgment with session, status and dropped `{message, text}` objects. Context is retained. |
| `brnr queue` | Shows held messages/context, optionally removing entries. | JSON has `session`, `held`, `context`, `dropped`; text displays the same queue contents. |
| `brnr pending` | Lists pending approvals across hosts, optionally filtered by session. | Table or JSON array of approval objects. |
| `brnr show` | Displays one pending approval in full. | Tool/command/diff details; JSON approval object includes `request`, `session`, `owner`, `answerable`, `why_not`, `title`, `kind`, `tool_call`, `options`, `timeout_seconds`. |
| `brnr approve`, `brnr deny` | Resolves an approval, optionally choosing the agent's exact option ID. | Acknowledgment with `session`, `request`, `outcome`. An unavailable/invalid choice fails. |
| `brnr list` | Running sessions by default; `--inactive` only saved inactive sessions; `--all` both. | Session table or JSON array with `session`, `title`, `state`, `pid`, `agent`, `cwd`, `last_active`. |
| `brnr status` | Reads a running session's current state. | Detailed summary or JSON object; fields described below. |
| `brnr sessions` | Starts an agent to query its own session list for the selected cwd. | Session table or JSON array with `session`, `title`, `state`, `pid`, `last_active`, `cwd`; unknown state/PID can be null. |
| `brnr fork` | Copies a session into the same headless process, if supported. | New session acknowledgment; JSON includes the new `session`. |
| `brnr close` | Cancels and closes a session; the last headless session closing stops its host. | Closed-session acknowledgment; errors on stderr. |
| `brnr log` | Reads saved events, including inactive sessions; `--last` selects recent turns and `--follow` continues live. | Text events or newline-delimited JSON (one event per line). |
| `brnr watch` | Subscribes to live events for a session or host PID. | Text events or newline-delimited JSON; a session watch ends when that session closes. |
| `brnr notify` | Runs the supplied command for selected events; `--stdin` consumes bridge event lines instead of connecting. | Child command output; notification failures/cutoffs reported on stderr. See [notifications](../README.md#notifications). |
| `brnr mode`, `brnr model` | Lists choices/current value or sets the supplied ID. | Choices with current selection; JSON has `session`, `mode`/`model`, and `modes`/`models` when listing. |
| `brnr config` | Lists options or sets repeatable `option=value` pairs. | JSON has `session` and `options` when listing, or `set` when updating. Listed options contain `option`, `value`, `choices`, `name`. |
| `brnr commands` | Lists the agent's advertised slash commands; invoke them by sending text. | JSON has `session` and `commands`, each with `command`, `hint`, `description`. |
| `brnr doctor` | Checks configuration, permissions, processes and transcripts; `--fix` performs the documented safe repairs. | Check lines or JSON array of `level`, `check`, `message`. `--report` produces Markdown; with `--json`, an object containing `brnr`, `os`, `adapters`, `checks`, `host_logs`. |
| `brnr skill` | Reads embedded guidance, or a named reference: `orchestrate`, `approvals`, `observe`, `setup`. `install` writes it into each directory's `brnr/` subdirectory. | Markdown, or installed-path messages. Default install roots: `~/.claude/skills`, `~/.agents/skills`. No JSON mode. |
| `brnr --version`, `brnr --help` | No session required. | Version string or usage text. |

### Prompts, attachments and lifecycle flags

- `start --prompt -` and `send <session> -` read prompt text from stdin.
  `--file` and `--image` may repeat; they become ACP content blocks. See
  [files and images](adr/0032-files-and-images.md) for encoding and capability
  requirements. A failed read prevents submission.
- `send` holds a prompt while a turn runs. `--steer` injects into the running
  turn when supported; `--interrupt` cancels it and sends ahead of held
  prompts. `--context` adds context to the next prompt; `--replace` replaces
  the last context entry. These are distinct modes, not combined flags.
- `start --resume` selects an existing agent session. `--take-over` requests
  that its current owner close it first. `--auth` names a noninteractive
  authentication method; credentials/login remain the agent's responsibility.
- `--mode`, `--model` and repeatable `--set` request startup settings.
  `--stop-when-idle` is a number of seconds; zero means stop/close as soon as
  idle. `--timeout` bounds waiting, not the lifetime of the session.
- `start --wait` requires a prompt or attachment. `start --wait` and
  `send --wait` write the reply on stdout; approvals and
  startup diagnostics go to stderr. JSON is one object at completion with
  `session`, `message`, `reply` (string), `stop_reason`, `error`, `dropped`,
  and `pid` for `start`. Missing error/drop information is null. A command
  may fail before a completion object is available.
- `start --foreground` instead stays attached, displaying event lines;
  `--json` makes them JSON lines and `--quiet` suppresses the display.
  Ctrl-C stops the session. It cannot be combined with `--wait`.
- `--strict` on `acp`/`start` selects stable ACP only. Actions on an editor's
  session require the corresponding experimental profile setting. See
  [strict mode](../README.md#strict-mode-and-feature-flags) and
  [experimental actions](../README.md#experimental-actions).

### Status fields

`status --json` identifies the session with `session`, `title`, `pid`,
`agent` (`program`, optionally `name` and `version`), `owner`, and `cwd`.
`held_by`, `shared_by`, and `lock_error` describe ownership/locking.

Activity fields are `state` (`idle`, `busy`, `waiting`), `turn_seconds`,
`mode`, `model`, `tools`, `plan`, `pending`, `held`, `context`, `usage`,
`last_message`, `last_active`, `stop_when_idle`, `stopping`, and
`uptime_seconds`. `pending`, `held`, and `context` are counts; use `pending`
and `queue` for their contents. `last_message` is a preview, not the full
transcript. Tool, plan and usage data come from the agent.

### Exit codes

| Context | Result |
|---|---|
| Ordinary CLI command | 0 on success; 1 for invalid arguments or an operation failure. |
| `wait`, `start --wait`, `send --wait` | 124 when the wait timeout expires. For a turn result, 0 for `end_turn`, otherwise 1; dropped messages and premature exit/closure also fail. `wait --for permission`/`exit` succeeds when its requested condition occurs. |
| `wait --for idle` | An already idle session, or one closed while waiting, uses its last turn's result; no turn running and nothing held is idle. |
| `brnr acp` | Invalid invocation/profile resolution exits 2; host startup failures exit 1; after startup the proxy follows the agent's exit/signal status. |
| `start --foreground` | Follows the agent's exit status; startup failure is nonzero. |
| `doctor` | Nonzero if a check fails; read its checks for the cause. |

A failed control command does not imply the host or agent stopped. In
particular, a wait timeout does not cancel its turn. The detailed wait
rules are in [ADR 21](adr/0021-waiting-and-exit-status.md).

## Editor transport

Configure the editor to execute `brnr acp` with the chosen profile/agent.
Stdin and stdout carry the editor-agent ACP JSON-RPC stream, not the bridge
protocol below. Do not write human messages to this stdout. brnr relays ACP
between the editor and its agent; stderr carries diagnostics and the
agent's stderr. The editor initializes the protocol and owns its sessions.
See [editor setup and deviations](../README.md#use-it-from-an-editor) and
[ADR 2](adr/0002-the-editors-process.md). Default capability adjustments and
experimental actions are documented there; strict mode passes the editor's
capabilities through and disables experimental actions.

## Configuration and environment

Configuration is TOML. [Profiles](../README.md#profiles) gives a complete
example. Unknown keys, keys in the wrong section, and unknown feature/action
names fail to load. `doctor` validates every profile.

| Section | Keys and input types |
|---|---|
| `profiles.<name>` | `agent`: argv string array; `log`: `"all"` (default), `"events"`, or `false`; `strict`: boolean (default false); `bridges`: table array. |
| `profiles.<name>.headless` | `cwd`, `mode`, `auth`: strings; `config`: map of option IDs to strings; `permission_timeout`, `stop_when_idle`: nonnegative integer seconds; `mcp_servers`: table array. Omitted timeouts impose no configured deadline. |
| `profiles.<name>.editor` | `experimental`: array of `send`, `context`, `cancel`, `approve`, `settings`, `close`; `features`: array containing `shared_sessions` if enabled. Both default empty. |
| Bridge table | `command`: nonempty argv string array; optional `events`: event-name array (default all except `acp`). |
| MCP server table | `name` plus stdio `command`, optional `args` and string-map `env`; or remote `url`, `type` (`http` by default, or `sse`) and string-map `headers`. See [MCP servers](adr/0031-mcp-servers.md). |

[Environment variables](../README.md#environment) define `BRNR_HOME`
(transcripts), `BRNR_CONFIG` (configuration file), `BRNR_DIR` (private
runtime directory), and `BRNR_START_TIMEOUT` (startup deadline, default
120 seconds), including their default paths. Agent executables and bridge
commands inherit the launching environment; authentication variables are
agent-specific. A started bridge additionally receives `BRNR_PID` and
`BRNR_SOCKET`.

`notify` passes its child the full event JSON on stdin and environment
variables `BRNR_EVENT`, `BRNR_TEXT`, `BRNR_TITLE`, `BRNR_MESSAGE`,
`BRNR_SESSION_ID`, `BRNR_REQUEST`, `BRNR_PID`. The displayed text values
are escaped and limited to 32 KiB; JSON on stdin is not truncated. See
[notifications](../README.md#notifications) for event selection and lifetime.

## Bridge and control socket protocol

A profile-started bridge writes requests to its stdout and reads replies
and events from stdin. Its stderr goes to the host log. An external client
can instead connect to `$BRNR_DIR/<pid>.sock` and write/read the same
protocol. Access is restricted by the private runtime directory; this is
a local control interface, not an HTTP service or a sandbox boundary
against other programs running as the same user.

Send one UTF-8 JSON object followed by a newline per request. `cmd` is a
command string; optional `req_id` is echoed unchanged in the reply. Use a
unique request ID to match replies: asynchronous agent operations can finish
later, and events may arrive between replies. Success replies have `ok:true`;
failures have `ok:false` and a human-readable `error` string. CLI JSON results
omit this protocol envelope. Malformed request lines receive an error;
clients should also handle EOF/disconnection rather than wait indefinitely.

Example exchange (IDs are illustrative; use a session served by this host):

```json
{"cmd":"send","req_id":"r1","session":"sess-1","text":"Summarize the changes"}
{"ok":true,"status":"delivered","session":"sess-1","message":"m1","req_id":"r1"}
```

An accepted send is not a completed turn. Subscribe and correlate the message
ID with `turn_ended.messages` or `message_dropped.message`.

| `cmd` | Additional request fields | Success reply, in addition to `ok` and optional `req_id` |
|---|---|---|
| `status` | None | Host metadata, owner, uptime, stopping/idle settings, pending count, bridge labels, and `sessions` array. Sessions use `session_id`, not the CLI's `session`. |
| `logged` | None | Acknowledgment after earlier recorded data has been processed by the transcript writer; inspect recorded gaps/errors for lost data. |
| `send` | `session`; optional `text`, `blocks` (ACP content-block array), `mode` (`prompt`, `steer`, `interrupt`, `context`), `replace` (boolean). | `session`, `status`, `message`; context mode returns `status:"held"` without a message ID. |
| `cancel` | `session`; optional boolean `keep_held`. | `session`, `status`, `dropped` objects (`message`, `text`). |
| `queue` | `session`; optional `drop` (message ID), `clear`, `clear_context` (booleans). | `session`, `held`, `context`, `dropped`. |
| `subscribe` | Optional `events`: array of event names or string `"all"`. | `events`: actual subscribed names. Omitted/null means all except `acp`; empty array selects none. Selection is host-wide; filter session IDs on the client. |
| `pending` | None | `pending`: approval objects for the host's sessions. |
| `approve`, `deny` | `session`, `request`; optional `option` string. | `session`, `request`, `outcome`. |
| `set_mode` | `session`, `mode` string. | `session`, `mode`, or updated `config` when the agent represents modes as config options. |
| `set_config` | `session`, `option`, `value` strings. | `session`, updated `config`. |
| `set_model` | `session`, `model` string. | `session`, updated `config`. Uses the agent's config option whose category is `model`. |
| `fork` | `session`. | New `session` ID. |
| `close` | `session`; optional `take_over` destination PID, used by brnr's resume flow. | Closed `session` ID after agent acknowledgment. |
| `stop` | None. | `status:"stopping"`; does not wait for final exit. |

All session-specific commands require an exact session ID served by this
host. Unsupported agent operations, strict-mode restrictions and disabled
editor actions produce errors. `stop` is process management and is not an
experimental editor action.

### Events and connection lifetime

Events have an `event` name and timestamp `ts`, with host identity metadata;
session events have `session`. The [event catalogue](../README.md#events)
lists every event and its meaning, including message/turn correlation,
approvals, config patches and process exits. `exited` and `line_too_long`
are process events. `session_changed` carries `what` and `value`; config
and command changes use merge patches, with null removing an entry.

Socket clients must subscribe to get live events. Profile-started bridges
are subscribed from startup. Bridge defaults include all events except
`acp`; the CLI display defaults additionally omit `agent_thought`, `usage`
and `tool_progress`. CLI `--events default,...` expands a display preset;
bridge `events` arrays take explicit names, not the CLI preset syntax.

Read continuously: a slow observer can be disconnected. Profile bridges
get final events and then stdin EOF when their host stops; they should exit
on EOF and receive SIGTERM if still running two seconds later. Closing a
started bridge's stdout only ends its requests, not its event subscription.
See [buffering and cutoffs](../README.md#when-a-reader-falls-behind) for the
16 MiB observer queue, long-line handling and foreground skipped events.

## Files, output safety and compatibility

[Transcripts](../README.md#transcripts) documents the JSON-lines file paths,
host/session identity fields, logging modes, redaction and `records-skipped`
gap records. Read history through `log` where possible. A transcript is not
a guarantee of complete history: disk backlog/errors, disabled logging,
and abrupt process death can lose records. These formats also follow the
pre-1.0 compatibility policy.

Human-readable output escapes agent control characters; JSON retains the
agent's text, so escape it before displaying it in a terminal or UI. MCP
environment/header secrets are redacted from recorded ACP, but transcripts
can contain prompts and tool output. brnr writes a transcript or host log
only where the state directory, each directory below it and the file are the
user's own with no group or other access: what others can reach is made
private before the next record (a `made-private` host-log record says so),
and a symlink, another user's path, a non-regular file or one with other
hard links is refused, with no override (ADR 59). A refused host log fails
the start; a refused session file is a `session-log-failed` host-log record.
`doctor --report` is for review before
sharing; it does not upload anything. See the [threat model](threat-model.md)
for trust boundaries and the documented host-SIGKILL cleanup limitation.
