# External interface reference

This reference describes brnr's command-line inputs and outputs, editor
transport, configuration, and bridge protocol. It applies to the source
revision containing it; use the same tag as your installed `brnr --version`.
Before 1.0 these interfaces can change without compatibility aliases; see
[ADR 1, P9](adr/0001-principles.md#p9-no-backwards-compatibility-before-10).
From 1.0.0, what is marked **Covered** here is kept compatible within a
major version, and what is marked **Not covered** can change in any release
([ADR 64](adr/0064-what-the-compatibility-promise-covers.md));
[compatibility](#compatibility) has the rules.
For installation and a walkthrough, start with the [README](../README.md).

The command line is the interface. The `brnr` crate's Rust library is not
a second one: its one public item is `brnr::run(args)`, `brnr <args>` as a
function, `args` being the words after `brnr`, which returns the exit status
the command would exit with. Its modules are private; a typed facade, one
module per command group and one function per verb returning the data
`--json` prints, will follow #150. The `internals` feature, which the
crate's own tests and fuzz targets use, is not covered by any compatibility
promise ([ADR 66](adr/0066-the-library-is-private-and-its-facade-mirrors-the-cli.md)).

## Command syntax

`<name>` is a required value, `[...]` is optional, `...` means repeatable,
and `|` separates alternatives. Do not type the brackets. Session IDs are
exact, opaque strings from the agent, not prefixes or process IDs. Approval
handles such as `p1` come from `permission requests`; message IDs such as `m1`
come from `prompt send`. A PID identifies the brnr host that can own multiple
sessions.

Most commands are `brnr <group> <verb>`, grouped by what they act on:
`process`, `session`, `prompt`, `queue`, `permission`, `config` and `event`
([ADR 63](adr/0063-commands-are-grouped-by-what-they-act-on.md)).
`brnr <group> --help` prints one group's syntax, as does a group without a
verb (exiting 1). `brnr <group> <verb> --help` (or `-h`, anywhere before a
`--` and, for `prompt send`, before the prompt) prints that command's. An
option a command doesn't take fails as unknown, never read as its
`<session>` or `<pid>`; a word with one dash, such as `-x`, is an argument
(but for `brnr skill`). A command brnr doesn't have, such as the names these
replaced (`brnr send`), fails as unknown.

`--` separates brnr options from the agent or notification command and its
arguments. Programs are executed as argument vectors, not shell strings;
invoke a shell explicitly if shell syntax is needed. With no agent after
`--`, the selected profile supplies it; `session list` asks an agent only when
`--profile` or `-- <agent>` names one. `--profile` defaults to `default`
when that profile exists. `brnr --help` prints the syntax below, a section
per group; `-h` is an alias, and `-V` aliases `--version`. `brnr host` is internal, not a command
for users to start by hand.

**Covered** from 1.0.0: the commands, their flags and positional arguments,
and what each means. **Not covered**: the usage text itself, as `--help`
prints it, and `brnr host`.

```text
usage:
  brnr acp [--profile <p>] [--strict] [-- <agent> [args...]]
             what an editor runs as its ACP agent

process
  brnr process list [--json]
  brnr process stop <pid> [--json]

session
  brnr session new [--pid <pid>] [<new flags>] [-- <agent> [args...]]
                   a process for a headless session, in the background (or the foreground);
                   --pid: a session in that running process, without the process flags
  brnr session resume [--pid <pid>] <session> [--take-over] [<new flags>] [-- <agent> [args...]]
                   the same, carrying on a session
  brnr session status <session> [--json]
  brnr session fork <session> [--json]
  brnr session close <session> [--json]
  brnr session delete <session> [--purge] [--profile <p>] [--json] [-- <agent> [args...]]
  brnr session list [--include active,inactive] [--profile <p>] [--cwd <dir>] [--json]
                    [-- <agent> [args...]]

<new flags>
  process: [--profile <p>] [--auth <method>] [--strict] [--stop-when-idle <s>]
           [--permission-timeout <s>] [--foreground [--quiet]]
  session: [--cwd <dir>] [--mode <m>] [--model <m>] [--thought-level <l>]
           [--option <o>=<v>]...
  prompt:  [--prompt <text> | -] [--file <path>]... [--image <path>]...
           [--wait [--timeout <s>]] [--json]

prompt
  brnr prompt send <session> [--steer | --interrupt | --context [--replace]]
                   [--file <path>]... [--image <path>]... [--wait [--timeout <s>]] [--json]
                   (<text>... | -)
  brnr prompt cancel <session> [--keep-held] [--json]
  brnr prompt commands <session> [--json]

queue
  brnr queue list <session> [--json]
  brnr queue show <session> <message> [--json]
  brnr queue drop <session> <message> [--json]
  brnr queue clear <session> [--messages] [--context] [--json]

permission
  brnr permission requests [<session>] [--json]
  brnr permission show <session> <request> [--json]
  brnr permission allow <session> <request> [--always] [--option <id>] [--json]
  brnr permission reject <session> <request> [--always] [--option <id>] [--json]

config
  brnr config get <session> [--mode] [--model] [--thought-level] [--option <o>]... [--json]
  brnr config set <session> [--mode <m>] [--model <m>] [--thought-level <l>]
                  [--option <o>=<v>]... [--json]

event
  brnr event log <session> [--last <n>] [--follow] [--events <default|all|event>,...] [--json]
  brnr event watch (<session> | --pid <pid>) [--events <default|all|event>,...] [--json]
  brnr event notify (<session> | --pid <pid> | --stdin) [--events <default|all|event>,...]
                    -- <command> [args...]
  brnr event wait <session> [--for idle|turn|permission|exit] [--timeout <s>] [--json]

brnr
  brnr doctor [--fix | --report] [--json]
  brnr skill [<reference> | install [--dir <dir>]...]
             the skill for agents that use brnr: print it, or install it
  brnr --version

brnr <group> --help lists a group's commands. <session> is a session's id, as brnr session list
shows it. <request> is a pending approval's handle, as brnr permission requests shows it. <pid>
is a brnr process, which runs one agent for one or more sessions, as brnr process list shows
them. --json prints the same data as the text: one JSON value, or one event per line for event
log, event watch and session new --foreground. --strict is stable ACP only: no --steer into a
running turn, no session fork.
```

## CLI inputs and results

Normal commands write results to stdout and diagnostics to stderr. On
commands supporting `--json`, it selects structured data instead of the
human-readable table or summary. It does not convert usage errors or
ordinary diagnostics into JSON. Parse JSON, not human-readable columns.
Unknown or unavailable values may be null; agent-supplied objects vary
with the agent's capabilities.

**Covered** from 1.0.0: each command's `--json` structure, its fields, their
types and what they mean. **Not covered**: the human-readable tables and
summaries, and the wording of diagnostics and errors, on stderr or in a JSON
field.

| Command | Input and effect | Output |
|---|---|---|
| `brnr acp` | Editor ACP on stdin; launches the selected agent. | Agent ACP on stdout; diagnostics and agent stderr on stderr. See [editor transport](#editor-transport). |
| `brnr session new`, `brnr session resume` | Starts a process for a headless session, new or resumed; sets cwd, authentication and settings before an optional prompt. With `--pid <pid>`, opens the session in that running process instead: the session and prompt flags only. | Startup summary; JSON object with `session`, numeric `pid`, and `message` (null without a prompt). |
| `brnr process list` | Lists discovered running hosts. | Table or JSON array of `pid`, `owner`, `agent`, `sessions`, `uptime_seconds`, `cwd`. An unresponsive host has owner `unreachable`. |
| `brnr process stop` | Stops the specified host and its agent; affects all its sessions. | Stopping acknowledgment; JSON object with numeric `pid` and `status` (`stopping`). Errors on stderr. |
| `brnr prompt send` | Sends text/attachments to one session. Positional words form the prompt; `-` reads it from stdin. | Acknowledgment with `session`, `status`, and `message`; context-only submissions have no message ID. Status is `delivered`, `held`, `steered` or `interrupting`. With `--wait`, a completed-turn result instead. |
| `brnr event wait` | Waits for `idle` (default), `turn`, `permission`, or process `exit`. | Condition summary or JSON for the condition/last turn. See [exit codes](#exit-codes). |
| `brnr prompt cancel` | Cancels the turn; drops held messages unless `--keep-held`. | Acknowledgment with session, status and dropped `{message, text}` objects. Context is retained. |
| `brnr queue list` | Shows held messages and context. | JSON has `session`, `held` (`message`, `text`, `interrupt`, `attachments`, a count), `context`, `dropped`; text displays the same queue contents. |
| `brnr queue show` | Shows one held message in full. | Its ID, whether it interrupts, its text, and each attachment (a file's URI, an image's type and size); JSON has `session`, `message`, `text`, `interrupt`, `attachments` and `blocks`, the attachments as ACP content blocks. A message not held fails. |
| `brnr queue drop` | Drops one held message by ID. | As `queue list`, with the message in `dropped`. |
| `brnr queue clear` | Drops the held messages (`--messages`), the held context (`--context`), or with neither flag both. | As `queue list`, with the messages in `dropped`; each dropped message or context is an event. |
| `brnr permission requests` | Lists pending approvals across the hosts that answer, optionally filtered by session; a host that doesn't answer is passed over, as `process list` and `session list` pass over it (they show it `unreachable`), with `brnr: process <pid> is not answering; its approvals aren't listed` on stderr, in text and `--json` alike; the exit status stays 0. | Table or JSON array of approval objects. |
| `brnr permission show` | Displays one pending approval in full. | Tool/command/diff details; JSON approval object includes `request`, `session`, `owner`, `answerable`, `why_not`, `title`, `kind`, `tool_call`, `options`, `timeout_seconds`. |
| `brnr permission allow`, `brnr permission reject` | Answers an approval with its option of kind `allow_once` (`allow_always` with `--always`) or `reject_once` (`reject_always`); `--option` names the agent's option ID instead, of the verb's side if its kind is ACP's (and the always kind with `--always`). | Acknowledgment with `session`, `request`, `outcome` (ACP's). A missing or doubled kind, or an option of the other side, fails, listing the options; nothing is answered. |
| `brnr session list` | brnr's sessions, open and with a transcript, in every cwd (`--cwd`: one), with nothing started. With `--profile` or `-- <agent>`, only the cwd's (here unless `--cwd`), joined on the session id with the agent's own (`session/list`, every page, from the agent started just for that), with a session open in a process that the agent lists there whatever its recorded cwd; an agent that can't list fails. A cwd is the same however it is spelled (a trailing slash, a symlink); an `unreachable` session is the cwd's if the agent lists it there or brnr has its transcript there. `--include active` keeps sessions open in a process (`idle`, `busy`, `waiting`, `unreachable`), `--include inactive` the rest; both by default. | Session table or JSON array, most recently active first, with `session`, `title`, `state`, `pid`, `agent`, `source` (`brnr`, `agent` or `both`), `last_active`, `cwd`. `last_active` is an RFC 3339 time in UTC to the microsecond (`2026-10-11T06:00:00.000000Z`; the text shows it to the second), an agent's `updatedAt` converted from its offset, and rows are ordered by the time it names; one that isn't an RFC 3339 time is as the agent gave it, said on stderr, after the rows with a time. A session only the agent knows is `inactive`, with `pid` null. Title, PID, time and cwd can be null. |
| `brnr session status` | Reads a running session's current state. | Detailed summary or JSON object; fields described below. |
| `brnr session fork` | Copies a session into the same headless process, if supported. | New session acknowledgment; JSON includes the new `session`. |
| `brnr session close` | Cancels and closes a session; the last headless session closing stops its host. | Closed-session acknowledgment; JSON object with the closed `session`. Errors on stderr. |
| `brnr session delete` | Starts the agent recorded for an inactive session (or `--profile`/`-- <agent>`) to delete its copy (`session/delete`); refused for a session open in a process or an agent without `sessionCapabilities.delete`. brnr's transcript stays, with a `session_deleted` record, unless `--purge`, which deletes the session's events and raw ACP files (not the host logs) whatever the agent answered. | Summary, or JSON with `session`, `deleted` (whether the agent did), `error` (the agent's, else null), `recorded` and `purged` (file paths), `failed` (why a file wasn't deleted, or, without `--purge`, wasn't recorded in; exit 1, the others done all the same). A transcript made private first is `made-private: <path> (its mode was …)` on stderr. Exit 1 too if the agent didn't delete the session, unless, with `--purge`, it doesn't have it (`resource_not_found`). |
| `brnr event log` | Reads saved events, including inactive sessions; `--last` selects recent turns and `--follow` continues live. | Text events or newline-delimited JSON (one event per line). |
| `brnr event watch` | Subscribes to live events for a session or host PID. | Text events or newline-delimited JSON; a session watch ends when that session closes. |
| `brnr event notify` | Runs the supplied command for selected events; `--stdin` consumes bridge event lines instead of connecting. | Child command output; notification failures/cutoffs reported on stderr. See [notifications](../README.md#notifications). |
| `brnr config get` | Lists the config options and the agent's v1 modes; `--mode`, `--model`, `--thought-level` (by category) and repeatable `--option <id>` narrow it. It lists only the options the agent advertised, so one the agent doesn't have fails. | Table of each option, then its choices (the current one marked `*`), or JSON with `session` and `options`, each with `option` (null for the v1 modes), `category`, `value`, `name`, `description`, and `choices`, each with `value`, `name`, `description`. |
| `brnr config set` | Sets `--mode`, `--model`, `--thought-level` (the option of that category; a mode is `session/set_mode` only with v1 modes and no mode option) and repeatable `--option <id>=<value>`, resolved as `session new`'s settings are: two values for one setting fail, or a mode, model or thought level the agent has no option for, before anything is sent; each is sent once, the mode first. An `--option` id the agent hasn't advertised is sent too, for the agent to take or refuse; a refusal names what was already set. | One `<option>=<value>` line per setting sent (`mode=` for v1 modes); JSON has `session` and `set`, each with `option`, `category`, `value`. |
| `brnr prompt commands` | Lists the agent's advertised slash commands; invoke them by sending text. | JSON has `session` and `commands`, each with `command`, `hint`, `description`. |
| `brnr doctor` | Checks configuration, permissions, processes and transcripts; `--fix` performs the documented safe repairs. | Check lines or JSON array of `level`, `check`, `message`. `--report` produces Markdown; with `--json`, an object containing `brnr`, `os`, `adapters`, `checks`, `host_logs`. |
| `brnr skill` | Reads embedded guidance, or a named reference: `orchestrate`, `approvals`, `observe`, `setup`. `install` writes it into each directory's `brnr/` subdirectory. | Markdown, or installed-path messages. Default install roots: `~/.claude/skills`, `~/.agents/skills`. No JSON mode. |
| `brnr --version`, `brnr --help` | No session required. | Version string or usage text. |

### Prompts, attachments and lifecycle flags

- `prompt send`'s options go before its prompt: from the prompt's first
  word on, every word is the prompt's, as after `--` (`prompt send $s
  explain ls -h` sends `explain ls -h`).
- `session new --prompt -` and `prompt send <session> -` read prompt text from
  stdin. `prompt send`'s `-` goes alone: a word after it fails the command,
  rather than being sent; quote a prompt that starts `- `.
  `--file` and `--image` may repeat; they become ACP content blocks. See
  [files and images](adr/0032-files-and-images.md) for encoding and capability
  requirements. A failed read prevents submission.
- `prompt send` holds a prompt while a turn runs. `--steer` injects into the running
  turn when supported; `--interrupt` cancels it and sends ahead of held
  prompts. `--context` adds context to the next prompt; `--replace` replaces
  the last context entry. These are distinct modes, not combined flags.
- `session resume <session>` resumes an existing agent session (`session new`
  opens a new one); they take the same flags. `--take-over`, on `resume`
  only, requests that its current owner close it first. `--auth` names a noninteractive
  authentication method; credentials/login remain the agent's responsibility.
- `--pid <pid>` opens the session in a running headless process, with its
  agent, profile and MCP servers, in `--cwd` (a resumed session's recorded
  cwd, or here). The process flags (`--profile`, `--auth`, `--strict`,
  `--stop-when-idle`, `--permission-timeout`, `--foreground`, `--quiet`) and
  `-- <agent>` are an error with it. An editor's process refuses, and so
  does one with `stop_when_idle` whose agent can't close sessions. A session
  open in that process already is refused; one another process holds is
  refused unless `--take-over`, which first checks that the process would
  take it (it isn't starting or stopping, and its agent can resume
  sessions). The session, its settings (over the profile's) and the prompt
  commit as one: a setting that fails after the agent opened the session
  closes it again, and the error says so, or, for an agent that can't close
  sessions, names the session left open. The start timeout goes to the
  process, which abandons the opening the same way once it passes; the
  command waits 10 s longer. A process stopping meanwhile fails it. A
  command gone first (Ctrl-C) leaves no session. None of these sends the
  prompt. A process with a session being opened doesn't stop as its other
  sessions close or go idle meanwhile.
- `--mode`, `--model`, `--thought-level` and repeatable `--option <id>=<value>`
  request startup settings, winning over the profile's, setting by setting.
  `--stop-when-idle` and `--permission-timeout` are numbers of seconds,
  winning over the profile's `stop_when_idle` and `permission_timeout`;
  `--stop-when-idle 0` closes as soon as idle. `--timeout` bounds waiting, not the lifetime of the session, and goes with `--wait`: without it, it fails the command.
- `--wait` requires a prompt or attachment. `session new --wait`,
  `session resume --wait` and `prompt send --wait` write the reply on stdout; approvals and
  startup diagnostics go to stderr. JSON is one object at completion with
  `session`, `message`, `reply` (string), `stop_reason`, `error`, `dropped`,
  and `pid` for `session new` and `session resume`. Missing error/drop information is null. A command
  may fail before a completion object is available.
- `--foreground` instead stays attached, displaying event lines;
  `--json` makes them JSON lines and `--quiet` suppresses the display.
  Ctrl-C stops the session. It cannot be combined with `--wait`.
- `--strict` on `acp`, `session new` and `session resume` selects stable ACP only. Actions on an editor's
  session require the corresponding experimental profile setting. See
  [strict mode](../README.md#strict-mode-and-feature-flags) and
  [experimental actions](../README.md#experimental-actions).

### Status fields

`session status --json` identifies the session with `session`, `title`, `pid`,
`agent` (`program`, optionally `name` and `version`), `owner`, and `cwd`.
`held_by`, `shared_by`, and `lock_error` describe ownership/locking.

Activity fields are `state` (`idle`, `busy`, `waiting`), `turn_seconds`,
`mode`, `model`, `tools`, `plan`, `pending`, `held`, `context`, `usage`,
`last_message`, `last_active`, `stop_when_idle`, `stopping`, and
`uptime_seconds`. `pending`, `held`, and `context` are counts; use
`permission requests` and `queue list` for their contents. `last_message` is a preview, not the full
transcript. Tool, plan and usage data come from the agent. `tools` has the
running turn's tool calls the agent hasn't finished, one per id; once a turn
ends, cancelled or failed too, none of its calls is listed.

### Exit codes

**Covered** from 1.0.0: the code of each situation below. A minor release
can give a situation that has none a code of its own.

| Context | Result |
|---|---|
| Ordinary CLI command | 0 on success; 1 for invalid arguments or an operation failure. |
| `event wait`, `session new --wait`, `session resume --wait`, `prompt send --wait` | 124 when the wait timeout expires. For a turn result, 0 for `end_turn`, otherwise 1; dropped messages and premature exit/closure also fail. `event wait --for permission`/`exit` succeeds when its requested condition occurs. |
| `event wait --for idle` | An already idle session, or one closed while waiting, uses its last turn's result; no turn running and nothing held is idle. |
| `brnr acp` | Invalid invocation/profile resolution exits 2; host startup failures exit 1; after startup the proxy follows the agent's exit/signal status. |
| `session new --foreground`, `session resume --foreground` | Follows the agent's exit status; startup failure is nonzero. |
| `session delete` | 1 if the agent didn't delete the session (its error, or no answer), with `--purge` too, but for an agent that doesn't have it (`resource_not_found`, which the pinned `brnr-claude-adapter` ([adapters/package.json](../adapters/package.json)) doesn't answer: it answers `-32603`): then 0, brnr's transcript deleted. 1 too if a file couldn't be deleted or recorded in. |
| `doctor` | Nonzero if a check fails; read its checks for the cause. |

A failed control command does not imply the host or agent stopped. In
particular, a wait timeout does not cancel its turn. The detailed wait
rules are in [ADR 21](adr/0021-waiting-and-exit-status.md).

## Editor transport

Configure the editor to execute `brnr acp` with the chosen profile/agent.
Stdin and stdout carry the editor-agent ACP JSON-RPC stream, not the bridge
protocol below. Do not write human messages to this stdout. brnr relays ACP
between the editor and its agent; stderr carries diagnostics and the
agent's stderr, as it is written. Stdout ends when the agent's stdout does,
even if the agent runs on; brnr writes nothing more there itself. Stderr
ends when `brnr acp` exits. Both streams share one connection and a 16 MiB
buffer, so an editor that stops reading one eventually holds up the other
([ADR 62](adr/0062-stdout-ends-and-stderr-comes-as-written.md)). The editor initializes the protocol and owns its
sessions.
See [editor setup and deviations](../README.md#use-it-from-an-editor) and
[ADR 2](adr/0002-the-editors-process.md). The editor's `initialize` reaches
the agent with its client capabilities as sent, `fs` and `terminal`
included, and the agent's `fs/*` and `terminal/*` requests go to the editor,
which answers them; a headless process offers neither, and answers them
with "method not found"
([ADR 67](adr/0067-the-editors-file-system-and-terminal-reach-the-agent.md)).
Experimental actions on an editor's session, experimental for all of 1.x,
are documented there too; strict mode disables them.

**Covered** from 1.0.0: ACP version 1
([ADR 54](adr/0054-acp-version-1.md)), and what passes between the editor
and the agent: unchanged, but for the changes the README and ADRs 2, 3 and
4 name, with the defaults they have at 1.0.0.

## Configuration and environment

Configuration is TOML. [Profiles](../README.md#profiles) gives a complete
example. Unknown keys, keys in the wrong section, and unknown feature/action
names fail to load. `doctor` validates every profile.

**Covered** from 1.0.0: the TOML below, its sections, keys, values and
names; the environment variables below, `BRNR_DIR` as where the runtime
directory is, not what is in it; the `BRNR_*` variables `event notify`
gives its command; and `BRNR_PID` in a started bridge's environment, but not
`BRNR_SOCKET`. A configuration that uses a key added in a later 1.x release
doesn't load in an earlier one.

| Section | Keys and input types |
|---|---|
| `profiles.<name>` | `agent`: argv string array; `log`: `"all"` (default), `"events"`, or `false`; `strict`: boolean (default false); `bridges`: table array. |
| `profiles.<name>.headless` | `cwd`, `mode`, `model`, `thought_level`, `auth`: strings; `options`: map of option IDs to strings; `permission_timeout`, `stop_when_idle`: nonnegative integer seconds; `mcp_servers`: table array. Omitted timeouts impose no configured deadline. A permission timeout answers with the `reject_once` option, else cancels the turn. |
| `profiles.<name>.editor` | `experimental`: array of `send`, `context`, `cancel`, `permission`, `config`, `close`; `features`: array containing `shared_sessions` if enabled. Both default empty. |
| Bridge table | `command`: nonempty argv string array; optional `events`: event-name array (default all except `acp`). |
| MCP server table | `name` plus stdio `command`, optional `args` and string-map `env`; or remote `url`, `type` (`http` by default, or `sse`) and string-map `headers`. See [MCP servers](adr/0031-mcp-servers.md). |

[Environment variables](../README.md#environment) define `BRNR_HOME`
(transcripts and session locks), `BRNR_CONFIG` (configuration file),
`BRNR_DIR` (private runtime directory), and `BRNR_START_TIMEOUT` (startup
deadline, default 120 seconds), including their default paths. How
`BRNR_HOME` and `BRNR_DIR` are found, and the state directory's layout, are
fixed for 1.x ([ADR 65](adr/0065-session-locks-live-with-the-transcripts.md));
what host logs and lock files contain isn't. Agent executables and bridge
commands inherit the launching environment; authentication variables are
agent-specific. A started bridge additionally receives `BRNR_PID` and
`BRNR_SOCKET`.

`event notify` passes its child the full event JSON on stdin and environment
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

**Covered** from 1.0.0: the protocol as a bridge brnr starts from a profile
speaks it, on its stdout and stdin: each `cmd`, its fields and its reply,
the events, and how long the bridge lasts. **Not covered**: the control
socket, which is in the private runtime directory and is not an interface:
its path, how processes and their metadata files are found, and a client
connecting to it directly, which speaks this protocol today but has no
promise and can change in any release; and the wording of `error`.

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
| `status` | None | Host metadata, owner, uptime, `starting` (until the first session commits), `stopping`, `closing` (the sessions it has closed, or failed to open, and holds the locks of until their records are written, ADR 48), the idle setting, `capabilities`, pending count, bridge labels, and `sessions` array. Sessions use `session_id`, not the CLI's `session`. |
| `logged` | None | Acknowledgment after earlier recorded data has been processed by the transcript writer; inspect recorded gaps/errors for lost data. |
| `send` | `session`; optional `text`, `blocks` (ACP content-block array), `mode` (`prompt`, `steer`, `interrupt`, `context`), `replace` (boolean). | `session`, `status`, `message`; context mode returns `status:"held"` without a message ID. |
| `cancel` | `session`; optional boolean `keep_held`. | `session`, `status`, `dropped` objects (`message`, `text`). |
| `queue` | `session`; optional `drop` (message ID), `clear`, `clear_context` (booleans); or `show` (message ID) alone; a `show` that is not a message ID is refused. | `session`, `held`, `context`, `dropped`; with `show`, that message: `session`, `message`, `text`, `interrupt`, `attachments`, `blocks`. |
| `subscribe` | Optional `events`: array of event names or string `"all"`. | `events`: actual subscribed names. Omitted/null means all except `acp`; empty array selects none. Selection is host-wide; filter session IDs on the client. |
| `pending` | None | `pending`: approval objects for the host's sessions. |
| `allow`, `reject` | `session`, `request`; optional `always` boolean, `option` string. | `session`, `request`, `outcome`. |
| `set_config` | `session`; optional `mode`, `model`, `thought_level` strings and `options` (object of option IDs to strings), at least one. Resolved and sent as `config set` does; answered once all are set, or with the first failure, which names what was already set. | `session`, `set` (`option`, `category`, `value` for each setting sent), `mode`, updated `config`. |
| `fork` | `session`. | New `session` ID. |
| `new` | Optional `cwd` (absolute; the process's by default); `mode`, `model`, `thought_level`, `options` as for `set_config`; `text`, `blocks` as for `send`, the first prompt; `timeout`, seconds. Opens a session (`session/new`, with the process's MCP servers), sets its settings over the profile's, then sends the prompt, if any. Refused by an editor's process, and with `stop_when_idle` when the agent can't close sessions. | `session`, `pid`, `message` (null without a prompt): answered before the prompt goes. A failure after the agent opened the session (a setting, `timeout` passing, the process stopping) fails it, and the prompt isn't sent; the session is closed again (or ends with a stopping process), and the error says so (or names it, left open, when the agent can't close sessions). |
| `resume` | `session`, and the fields of `new`, `cwd` being the session's. `session/resume`, or `session/load` where the agent has only that; the session's lock is taken first, so one another process holds, or this one serves, is refused. | As `new`. |
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
`session_reopened` follows a `session_closed` when the agent answered an
editor's load of the session with success after its close: the session is
open again, and the load's `history` follows it. `history` carries
`updates`, `recorded`, `recorded_updates` and `loads`. `recorded` says
whether the replay was taken into the transcript at all;
`recorded_updates`, how many of the `updates` are in it: all when
`recorded`, none when not, fewer when the session closed while its load was
unanswered, as what came meanwhile had no file to go to. An editor's loads
of a session that overlap share one replay and one `history` event, so
`loads` can be more than 1, and a recorded history is then in the
transcript once per load.

**Covered** from 1.0.0: each event's name and fields, wherever it goes: a
bridge's stdin, `--json` lines, `event notify`'s stdin and its variables.
Ignore the events and fields you don't know: a minor release can add them.

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
gap records. Read history through `event log` where possible. A transcript is not
a guarantee of complete history: disk backlog/errors, disabled logging,
and abrupt process death can lose records.

**Covered** from 1.0.0: each session's events and raw ACP files, where they
are under `$BRNR_HOME` and their records; a 1.x brnr reads what an earlier
1.x wrote; and where each other file under `$BRNR_HOME` is. **Not
covered**: what host logs (`hosts/<run id>.jsonl`) and session lock files
(`sessions/<id>.lock`) contain, and what is in the runtime directory:
sockets and metadata files.

Human-readable output escapes agent control characters; JSON retains the
agent's text, so escape it before displaying it in a terminal or UI. MCP
environment/header secrets are redacted from recorded ACP, but transcripts
can contain prompts and tool output. brnr writes a transcript or host log
only where the state directory, each directory below it and the file are the
user's own with no group or other access: what others can reach is made
private before the next record (a `made-private` host-log record says so,
or, from `session delete`, a line on stderr), and a symlink, another user's
path, a non-regular file or one with other hard links is refused, with no
override (ADR 59). A refused host log fails the start; a refused session
file is a `session-log-failed` host-log record.
`doctor --report` is for review before
sharing; it does not upload anything. See the [threat model](threat-model.md)
for trust boundaries and the documented host-SIGKILL cleanup limitation.

## Compatibility

Before 1.0.0 every interface here can change outright, with no aliases or
shims (P9). From 1.0.0, what the sections above mark **Covered** is kept
compatible within a major version, and anything they don't name is not
covered ([ADR 64](adr/0064-what-the-compatibility-promise-covers.md)). The
library on crates.io is covered as part of the command line: its public API
is a facade that mirrors it (#147, ADR 66); its internals, behind a feature that is off by
default, are not. Stable error codes (#205) are covered once they land.

- A minor release can add a command, flag, JSON field, event, event field,
  bridge request, config key, or an exit code for a situation that had
  none.
- Only a major release removes or renames any of those, changes a field's
  type or what it means, or changes the exit code of a situation that has
  one.
- Ignore the fields and events you don't know.
- A deprecated form keeps working until the next major version; the
  [changelog](../CHANGELOG.md) says what to use instead.

Security fixes are made on the latest major version; see
[SECURITY.md](../SECURITY.md). Before 1.0.0-rc.1, the names and shapes of
`--json` output (#150), events and transcript records (#151), the exit code
of a usage error (#153) and flags and config keys spelled two ways (#154)
are still being settled.
