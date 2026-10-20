# External interface reference

This reference describes brnr's command-line inputs and outputs, editor
transport, configuration, and bridge protocol. It applies to the source
revision containing it; use the same tag as your installed `brnr --version`.
Before 1.0 these interfaces can change without compatibility aliases; see
[ADR 1, P9](adr/0001-principles.md#p9-no-backwards-compatibility-before-10).
From 1.0.0, what is marked **Covered** here is kept compatible within a
major version, and what is marked **Not covered** can change in any release
([ADR 8.4](adr/0008-the-command-line-and-compatibility.md#84-what-the-compatibility-promise-covers));
[compatibility](#compatibility) has the rules.
For installation and a walkthrough, start with the [README](../README.md).

The command line is the interface. The `brnr` crate's Rust library is not
a second one: its one public item is `brnr::run(args)`, `brnr <args>` as a
function, `args` being the words after `brnr`, which returns the exit status
the command would exit with. Its modules are private; a typed facade, one
module per command group and one function per verb returning the data
`--json` prints, is to come (#215). The `internals` feature, which the
crate's own tests and fuzz targets use, is not covered by any compatibility
promise ([ADR 8.5](adr/0008-the-command-line-and-compatibility.md#85-the-library-is-private-and-its-facade-mirrors-the-command-line)).

## Command syntax

`<name>` is a required value, `[...]` is optional, `...` means repeatable,
and `|` separates alternatives. Do not type the brackets. Session IDs are
exact, opaque strings from the agent, not prefixes or process IDs. Approval
handles such as `p1` come from `permission requests`, elicitation handles such
as `e1` from `elicitation requests`; message IDs such as `m1` come from
`prompt send`. A PID identifies the brnr host that can own multiple
sessions.

Most commands are `brnr <group> <verb>`, grouped by what they act on:
`process`, `session`, `prompt`, `queue`, `permission`, `elicitation`, `config`
and `event`
([ADR 8.1](adr/0008-the-command-line-and-compatibility.md#81-commands-are-grouped-by-what-they-act-on-with-acps-verbs)).
`brnr <group> --help` prints one group's syntax; a group without a verb is
a usage error. `brnr <group> <verb> --help` (or `-h`, anywhere before a
`--`, and not as a flag's value) prints that command's. Help
goes to stdout and exits 0 in every command, `brnr acp`, `doctor` and
`skill` too. A usage error (an unknown command, verb or option, an argument
missing or one too many, a value a flag can't take, flags that don't go
together) does nothing, says what was wrong and the command's usage on
stderr, and exits 2
([ADR 8.3](adr/0008-the-command-line-and-compatibility.md#83-waiting-and-exit-status)). An
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
           [--response-timeout <s>] [--foreground [--quiet]]
  session: [--cwd <dir>] [--mode <m>] [--model <m>] [--thought-level <l>]
           [--option <o>=<v>]...
  prompt:  [--prompt <text> | -] [--file <path>]... [--image <path>]...
           [--wait [--timeout <s>]] [--json]

prompt
  brnr prompt send <session> [--steer | --interrupt | --context [--replace]]
                   [--prompt <text> | -] [--file <path>]... [--image <path>]...
                   [--wait [--timeout <s>]] [--json]
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
  brnr permission allow <session> <request> [--always] [--option-id <id>] [--json]
  brnr permission reject <session> <request> [--always] [--option-id <id>] [--json]

elicitation
  brnr elicitation requests [<session>] [--json]
  brnr elicitation show (<session> | --pid <pid>) <request> [--json]
  brnr elicitation accept (<session> | --pid <pid>) <request> [--field <k>=<v>]... [-] [--json]
  brnr elicitation decline (<session> | --pid <pid>) <request> [--json]
  brnr elicitation cancel (<session> | --pid <pid>) <request> [--json]

config
  brnr config get <session> [--mode] [--model] [--thought-level] [--option <o>]... [--json]
  brnr config set <session> [--mode <m>] [--model <m>] [--thought-level <l>]
                  [--option <o>=<v>]... [--json]

event
  brnr event log <session> [--last <n>] [--follow] [--events <default|all|event>,...] [--json]
  brnr event watch (<session> | --pid <pid>) [--events <default|all|event>,...] [--json]
  brnr event notify (<session> | --pid <pid> | --stdin) [--events <default|all|event>,...]
                    -- <command> [args...]
  brnr event wait <session> [--for idle|turn|permission|elicitation|exit] [--timeout <s>]
                  [--json]

brnr
  brnr doctor [--fix | --report] [--json]
  brnr skill [<reference> | install [--dir <dir>]...]
             the skill for agents that use brnr: print it, or install it
  brnr --version

brnr <group> --help lists a group's commands. <session> is a session's id, as brnr session list
shows it. <request> is a pending approval's or elicitation's handle, as brnr permission requests
and brnr elicitation requests show them. <pid> is a brnr process, which runs one agent for one or
more sessions, as brnr process list shows them. --json prints the same data as the text: one JSON value, or one event per line for event
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

A concept has one name and one shape in every command's JSON
([ADR 8.2](adr/0008-the-command-line-and-compatibility.md#82-same-data-in-text-and-json)):

- A list is an object, the list under the plural of what it lists
  (`{"sessions": [...]}`), never a bare array.
- A row is the object its command's detail prints, with fewer fields:
  `session list` rows are `session status`'s fields, `permission requests`
  rows `permission show`'s, `elicitation requests` rows `elicitation
  show`'s.
- A name is a count or a list, never both: a count is `<what>_count`.
- `dropped` is a list of the messages dropped, each `{message, text,
  reason}`, as its `message_dropped` event has them.
- `agent` is `{program, name, version}`: `program` the command brnr ran
  (its file name), `name` and `version` what the agent says it is in
  `initialize` (ACP's `agentInfo`), each null where brnr doesn't know it.
- `owner` is `editor` or `headless` (null where unknown); a process that
  doesn't answer is `state: "unreachable"`.
- ACP's objects keep ACP's names: a permission request's `options` are
  `{optionId, name, kind}`.

**Covered** from 1.0.0: each command's `--json` structure, its fields, their
types and what they mean. **Not covered**: the human-readable tables and
summaries, and the wording of diagnostics and errors, on stderr or in a JSON
field.

| Command | Input and effect | Output |
|---|---|---|
| `brnr acp` | Editor ACP on stdin; launches the selected agent. | Agent ACP on stdout; diagnostics and agent stderr on stderr. See [editor transport](#editor-transport). |
| `brnr session new`, `brnr session resume` | Starts a process for a headless session, new or resumed; sets cwd, authentication and settings before an optional prompt. With `--pid <pid>`, opens the session in that running process instead: the session and prompt flags only. | Startup summary; JSON object with `session`, numeric `pid`, and `message` (null without a prompt). |
| `brnr process list` | Lists discovered running hosts. | Table, or JSON `{processes}`, each with `pid`, `state` (`starting`, `running`, `stopping`, or `unreachable` for a host that doesn't answer), `owner` (`editor`, `headless`, or null when unreachable), `agent`, `sessions` (each `{session, state}`, as `session list` has them; an unreachable host's are those it holds the locks of), `uptime`, `cwd`. |
| `brnr process stop` | Stops the specified host and its agent; affects all its sessions. | Stopping acknowledgment; JSON object with numeric `pid` and `status` (`stopping`). Errors on stderr. |
| `brnr prompt send` | Sends text/attachments to one session. `--prompt <text>` is the prompt; `--prompt -` reads it from stdin. | Acknowledgment with `session`, `status`, and `message`; context-only submissions have no message ID. Status is `delivered`, `held`, `steered` or `interrupting`. With `--wait`, a completed-turn result instead. |
| `brnr event wait` | Waits for `idle` (default), `turn`, `permission`, `elicitation`, or process `exit`. | Condition summary, or JSON in one shape for each `--for`, however the wait ended: `idle` `{session, state, last_turn}` (`state` `idle`, or `inactive` for a session that closed; `last_turn` as `session status` has it); `turn` the `turn_ended` event; `permission` and `elicitation` the request as `permission show` and `elicitation show` print it; `exit` the `process_exited` event (for a process gone without one, `exit_status` and `seq` null and `error` saying so). See [exit codes](#exit-codes). |
| `brnr prompt cancel` | Cancels the turn; drops held messages unless `--keep-held`. | Acknowledgment with `session`, `status` and `dropped`, each `{message, text, reason}` (`cancel`). Context is retained. |
| `brnr queue list` | Shows held messages and context. | JSON has `session`, `held` (`message`, `text`, `interrupt`, `attachments`, a count), `context` (the held context's text), `dropped`; text displays the same queue contents. |
| `brnr queue show` | Shows one held message in full. | Its ID, whether it interrupts, its text, and each attachment (a file's URI, an image's type and size); JSON has `session`, `message`, `text`, `interrupt`, `attachments` and `blocks`, the attachments as ACP content blocks. A message not held fails. |
| `brnr queue drop` | Drops one held message by ID. | As `queue list`, with the message in `dropped` (`reason` `queue`). |
| `brnr queue clear` | Drops the held messages (`--messages`), the held context (`--context`), or with neither flag both. | As `queue list`, with the messages in `dropped`; each dropped message or context is an event. |
| `brnr permission requests` | Lists pending approvals across the hosts that answer, optionally filtered by session; a host that doesn't answer is passed over, as `process list` and `session list` pass over it (they show it `unreachable`), with `brnr: process <pid> is not answering; its approvals aren't listed` on stderr, in text and `--json` alike; the exit status stays 0. | Table, or JSON `{requests}`, each `permission show`'s object with `session`, `request`, `owner`, `kind`, `title`, and `options`, the agent's (`{optionId, name, kind}`). |
| `brnr permission show` | Displays one pending approval in full. | Tool/command/diff details; JSON approval object includes `request`, `session`, `owner`, `answerable`, `why_not`, `title`, `kind`, `tool_call`, `options`, `timeout_left`. |
| `brnr permission allow`, `brnr permission reject` | Answers an approval with its option of kind `allow_once` (`allow_always` with `--always`) or `reject_once` (`reject_always`); `--option-id` names the agent's option ID instead, of the verb's side if its kind is ACP's (and the always kind with `--always`). | Acknowledgment with `session`, `request`, `outcome` (ACP's). A missing or doubled kind, or an option of the other side, fails, listing the options; nothing is answered. |
| `brnr elicitation requests` | Lists pending elicitations (the agent asking for input, ADR 4.5) across the hosts that answer, optionally filtered by session; a host that doesn't answer is passed over and said to be on stderr, as for `permission requests`. | Table (`SESSION`, `REQUEST`, `OWNER`, `MODE`, `MESSAGE`; one outside any session has `--pid <pid>` as its session), or JSON `{requests}`, each `elicitation show`'s object with `session` (null outside any session), numeric `pid`, `request`, `owner`, `mode`, `scope`, `text` (the agent's message). |
| `brnr elicitation show` | Displays one pending elicitation in full, named by `<session>` or, outside any session, `--pid <pid>`. A URL is printed, never opened. | Message, mode, scope, the form's fields with their types, bounds and choices, or the URL; JSON object with `request`, `session`, numeric `pid`, `owner`, `answerable`, `why_not`, `mode`, `text` (the agent's message), `scope` (`session`, `tool_call` or `request`), `tool_call_id`, `scope_request_id`, `schema` (the `requestedSchema` as sent), `url`, `elicitation_id`, `timeout_left`. |
| `brnr elicitation accept`, `brnr elicitation decline`, `brnr elicitation cancel` | Answers an elicitation with ACP's action. `accept` takes a form's values as repeatable `--field <name>=<value>` (repeated for a list's values) or one JSON object on stdin (`-`), checked by the host against the form's schema before anything is sent: fields in the schema, required ones given, types (a `--field` string read as the type asks), bounds and choices; `pattern` and `format` are not checked. A URL's accept takes no fields; a mode brnr doesn't know can't be accepted. | `<request> accepted` (`declined`, `cancelled`); JSON with `session`, `request`, `action`, `content` (an accept's, with ACP's value types, else null). A value that doesn't fit fails, naming the field; nothing is sent. |
| `brnr session list` | brnr's sessions, open and with a transcript, in every cwd (`--cwd`: one), with nothing started. With `--profile` or `-- <agent>`, only the cwd's (here unless `--cwd`), joined on the session id with the agent's own (`session/list`, every page, from the agent started just for that), with a session open in a process that the agent lists there whatever its recorded cwd; an agent that can't list fails. A cwd is the same however it is spelled (a trailing slash, a symlink); an `unreachable` session is the cwd's if the agent lists it there or brnr has its transcript there. `--include active` keeps sessions open in a process (`idle`, `busy`, `waiting`, `unreachable`), `--include inactive` the rest; both by default. | Session table, or JSON `{sessions}`, most recently active first, each with `session`, `title`, `state`, `pid`, `agent` (`{program, name, version}`; `name` and `version` only for a session open in a process that answers), `source` (`brnr`, `agent` or `both`), `last_active`, `forked_from`, `cwd`. `forked_from` is the session a fork came from (`session fork`, or an editor's `session/fork` through `brnr acp`), from its transcript's first record once its process has ended; null for one that isn't a fork, and for a row only the agent knows. `last_active` is an RFC 3339 time in UTC to the microsecond (`2026-10-11T06:00:00.000000Z`; the text shows it to the second), an agent's `updatedAt` converted from its offset, and rows are ordered by the time it names; one that isn't an RFC 3339 time is as the agent gave it, said on stderr, after the rows with a time. A session only the agent knows is `inactive`, with `pid` null. Title, PID, time and cwd can be null. |
| `brnr session status` | Reads a running session's current state. | Detailed summary or JSON object; fields described below. |
| `brnr session fork` | Copies a session into the same headless process, if supported. | New session acknowledgment; JSON has the new `session` and `forked_from`, the session it was forked from. |
| `brnr session close` | Cancels and closes a session; the last headless session closing stops its host. | Closed-session acknowledgment; JSON object with the closed `session`. Errors on stderr. |
| `brnr session delete` | Starts the agent recorded for an inactive session (or `--profile`/`-- <agent>`) to delete its copy (`session/delete`); refused for a session open in a process or an agent without `sessionCapabilities.delete`. brnr's transcript stays, with a `session_deleted` record, unless `--purge`, which deletes the session's events and raw ACP files (not the host logs) whatever the agent answered. | Summary, or JSON with `session`, `deleted` (whether the agent did), `error` (the agent's, else null), `recorded_in` and `purged` (file paths), `failed` (why a file wasn't deleted, or, without `--purge`, wasn't recorded in; exit 1, the others done all the same). A transcript made private first is `path_made_private: <path> (its mode was …)` on stderr. Exit 1 too if the agent didn't delete the session, unless, with `--purge`, it doesn't have it (`resource_not_found`). |
| `brnr event log` | Reads saved events, including inactive sessions, from the session's files in every project folder, merged by time; `--last` selects recent turns of what is merged and `--follow` continues live. | Text events or newline-delimited JSON (one event per line). |
| `brnr event watch` | Subscribes to live events for a session or host PID. | Text events or newline-delimited JSON; a session watch ends when that session closes. |
| `brnr event notify` | Runs the supplied command for selected events; `--stdin` consumes bridge event lines instead of connecting. | Child command output; notification failures/cutoffs reported on stderr. See [notifications](../README.md#notifications). |
| `brnr config get` | Lists the config options and the agent's v1 modes; `--mode`, `--model`, `--thought-level` (by category) and repeatable `--option <id>` narrow it. It lists only the options the agent advertised, so one the agent doesn't have fails. | Table of each option, then its choices (the current one marked `*`), or JSON with `session` and `options`, each with `option` (null for the v1 modes), `category`, `value`, `name`, `description`, and `choices`, each with `value`, `name`, `description`. |
| `brnr config set` | Sets `--mode`, `--model`, `--thought-level` (the option of that category; a mode is `session/set_mode` only with v1 modes and no mode option) and repeatable `--option <id>=<value>`, resolved as `session new`'s settings are: two values for one setting fail, or a mode, model or thought level the agent has no option for, before anything is sent; each is sent once, the mode first. An `--option` id the agent hasn't advertised is sent too, for the agent to take or refuse; a refusal names what was already set. | One `<option>=<value>` line per setting sent (`mode=` for v1 modes); JSON has `session` and `set`, each with `option`, `category`, `value`. |
| `brnr prompt commands` | Lists the agent's advertised slash commands; invoke them by sending text. | JSON has `session` and `commands`, each with `command`, `hint`, `description`. |
| `brnr doctor` | Checks configuration, permissions, processes and transcripts; `--fix` performs the documented safe repairs. | Check lines, or JSON `{checks}`, each with `level`, `check`, `message`. `--report` produces Markdown; with `--json`, an object containing `brnr`, `os`, `adapters`, `checks`, `host_logs`. |
| `brnr skill` | Reads embedded guidance, or a named reference: `orchestrate`, `approvals`, `observe`, `setup`. `install` writes it into each directory's `brnr/` subdirectory. | Markdown, or installed-path messages. Default install roots: `~/.claude/skills`, `~/.agents/skills`. No JSON mode. |
| `brnr --version`, `brnr --help` | No session required. | Version string or usage text. |

### Prompts, attachments and lifecycle flags

- The prompt is `--prompt <text>`, for `session new`, `session resume` and
  `prompt send` alike, its value sent as given (`--prompt -h` sends `-h`).
  A word left over, such as a prompt given as positional words, is a usage
  error. An empty `--prompt` is a usage error too.
- `--prompt -` reads prompt text from stdin; an empty one fails (exit 1).
  `--file` and `--image` may repeat; they become ACP content blocks. See
  [files and images](adr/0006-settings-and-profiles.md#64-files-and-images-in-a-message) for encoding and capability
  requirements. A failed read prevents submission.
- `prompt send` holds a prompt while a turn runs. `--steer` injects into the running
  turn when supported; `--interrupt` cancels it and sends ahead of held
  prompts. `--context` adds context to the next prompt; `--replace` replaces
  the last context entry. These are distinct modes, not combined flags.
- `session resume <session>` resumes an existing agent session (`session new`
  opens a new one); they take the same flags. A session brnr has a
  transcript of resumes with the cwd, agent and profile of the newest
  record of all its files, in every project folder; `--cwd`, `--profile`
  and `-- <agent>` override them. `--take-over`, on `resume`
  only, requests that its current owner close it first. `--auth` names a noninteractive
  authentication method; credentials/login remain the agent's responsibility.
- `--pid <pid>` opens the session in a running headless process, with its
  agent, profile and MCP servers, in `--cwd` (a resumed session's recorded
  cwd, or here). The process flags (`--profile`, `--auth`, `--strict`,
  `--stop-when-idle`, `--response-timeout`, `--foreground`, `--quiet`) and
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
  `--stop-when-idle` and `--response-timeout` are numbers of seconds,
  winning over the profile's `stop_when_idle` and `response_timeout`;
  `--stop-when-idle 0` closes as soon as idle. `--timeout` bounds waiting, not the lifetime of the session, and goes with `--wait`: without it, it fails the command.
- `--wait` requires a prompt or attachment. `session new --wait`,
  `session resume --wait` and `prompt send --wait` write the reply on stdout; approvals and
  startup diagnostics go to stderr. JSON is one object at completion with
  `session`, `message`, `reply` (string), `stop_reason`, `error`, `tokens`,
  `dropped`, and `pid` for `session new` and `session resume`. A missing
  error is null; `dropped` is empty, or the message, dropped before it was
  sent, as `{message, text, reason}`. A command
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
`agent` (`program`, and `name` and `version` as the agent gives them in
`initialize`'s `agentInfo`, else null), `owner`, and `cwd`.
`forked_from` is the session it was forked from, or null.
`lock_pid` (the process holding its lock, or null), `shared_pids` (the
processes serving it without its lock), and `lock_error` describe
ownership/locking.

Activity fields are `state` (`idle`, `busy`, `waiting`), `turn_elapsed`,
`mode`, `model`, `tools`, `plan`, `pending_count`, `elicitation_count`,
`held_count`, `context_count`, `usage`, `last_turn`, `last_message`,
`last_active`, `stop_when_idle`, `stopping`, and `uptime`. The `_count`
fields are counts; use `permission requests`, `elicitation requests` and
`queue list` for their contents. A session with an approval or an
elicitation waiting is `waiting`. `last_message` is a preview, not the full
transcript. `last_turn` is how the last turn ended, `{stop_reason, error,
tokens}` as its `turn_ended` has them, or null before the first. Tool, plan,
usage and token data come from the agent. `tools` has the
running turn's tool calls the agent hasn't finished, one per id; once a turn
ends, cancelled or failed too, none of its calls is listed.

### Exit codes

**Covered** from 1.0.0: the code of each situation below. A minor release
can give a situation that has none a code of its own.

| Context | Result |
|---|---|
| Every command | 2 for a usage error: what the command line alone shows wrong, with nothing done and the usage on stderr. 0 for `--help` or `-h`, the usage on stdout. |
| Ordinary CLI command | 0 on success; 1 for an operation failure (a session, process or profile that isn't there, an agent's error, a file that can't be read). |
| `event wait`, `session new --wait`, `session resume --wait`, `prompt send --wait` | 124 when the wait timeout expires. For a turn result, 0 for `end_turn`, otherwise 1; dropped messages and premature exit/closure also fail. `event wait --for permission`/`elicitation`/`exit` succeeds when its requested condition occurs. |
| `event wait --for idle` | An already idle session, or one closed while waiting, uses its last turn's result; no turn running and nothing held is idle. |
| `event watch`, `event notify`, `event wait`, `session new --wait`, `session resume --wait`, `prompt send --wait` | 3 when the process cut the command off for falling behind and it couldn't catch up from the transcript: what it missed is an `events_missed` note, with `ts`, `host_id` and `pid` as an event has them, `after_seq` the last event it got, `until_seq` where it subscribed again, or, if subscribing again failed each time, where the process said it cut the command off (null only for a process gone, or one that said no position, as `error` then says), and `error`, why ([ADR 10.2](adr/0010-bounded-resources.md#102-a-peer-cut-off-catches-up-from-the-transcript)). `event watch` prints it as its last event; the others say it on stderr. |
| `brnr acp` | A usage error exits 2; a config error or a host startup failure exits 1; after startup the proxy follows the agent's exit/signal status. |
| `session new --foreground`, `session resume --foreground` | Follows the agent's exit status; startup failure is nonzero. |
| `session delete` | 1 if the agent didn't delete the session (its error, or no answer), with `--purge` too, but for an agent that doesn't have it (`resource_not_found`, which the pinned `brnr-claude-adapter` ([adapters/package.json](../adapters/package.json)) doesn't answer: it answers `-32603`): then 0, brnr's transcript deleted. 1 too if a file couldn't be deleted or recorded in. |
| `doctor` | Nonzero if a check fails; read its checks for the cause. |

A failed control command does not imply the host or agent stopped. In
particular, a wait timeout does not cancel its turn. The detailed wait
rules are in [ADR 8.3](adr/0008-the-command-line-and-compatibility.md#83-waiting-and-exit-status).

## Editor transport

Configure the editor to execute `brnr acp` with the chosen profile/agent.
Stdin and stdout carry the editor-agent ACP JSON-RPC stream, not the bridge
protocol below. Do not write human messages to this stdout. brnr relays ACP
between the editor and its agent; stderr carries diagnostics and the
agent's stderr, as it is written. Stdout ends when the agent's stdout does,
even if the agent runs on; brnr writes nothing more there itself. Stderr
ends when `brnr acp` exits. Both streams share one connection and a 16 MiB
buffer, so an editor that stops reading one eventually holds up the other
([ADR 2.6](adr/0002-acp-faithfully.md#26-the-editors-stdout-ends-when-the-agents-does-and-its-stderr-comes-as-written)). The editor initializes the protocol and owns its
sessions.
See [editor setup and deviations](../README.md#use-it-from-an-editor) and
[ADR 3.1](adr/0003-processes-sessions-and-ownership.md#31-the-editors-process-brnr-acp-relays-the-host-owns-the-agent). The editor's `initialize` reaches
the agent without its `fs` and `terminal` client capabilities, as ACP v2
has neither, but those the profile's `client_capabilities` names; in strict
mode both reach it. The agent's `fs/*` and `terminal/*` requests, where it
was offered them, go to the editor, which answers them; a headless process
offers neither, and answers them with "method not found"
([ADR 2.7](adr/0002-acp-faithfully.md#27-the-editors-file-system-and-terminal-reach-the-agent-in-strict-mode-or-when-its-profile-names-them)).
Experimental actions on an editor's session, experimental for all of 1.x,
are documented there too; strict mode disables them.

**Covered** from 1.0.0: ACP version 1
([ADR 2.2](adr/0002-acp-faithfully.md#22-brnr-speaks-acp-version-1)), and what passes between the editor
and the agent: unchanged, but for the changes the README and ADRs 3.1 and 3.9 and
4 name, with the defaults they have at 1.0.0.

## Configuration and environment

Configuration is TOML. [Profiles](../README.md#profiles) gives a complete
example. Unknown keys, keys in the wrong section, and unknown feature/action
names fail to load. `doctor` validates every profile.

The file loads only if it is the user's own and neither the group nor others
can read or write it, and its folder is the user's own and neither can write
it; a symlink is followed, and the file it resolves to and that file's folder
are checked. Otherwise every command that reads it fails, naming the path,
the owner or mode, and the `chmod` or `chown` that fixes it, and `doctor`
fails its config check the same way; brnr never changes the file's mode. A
missing file is no configuration
([ADR 9.4](adr/0009-security-and-privacy.md#94-the-config-file-is-read-only-if-only-you-can-change-or-read-it)).

**Covered** from 1.0.0: the TOML below, its sections, keys, values and
names; the environment variables below, `BRNR_DIR` as where the runtime
directory is, not what is in it; the `BRNR_*` variables `event notify`
gives its command; and `BRNR_PID` in a started bridge's environment, but not
`BRNR_SOCKET`. A configuration that uses a key added in a later 1.x release
doesn't load in an earlier one.

| Section | Keys and input types |
|---|---|
| `profiles.<name>` | `agent`: argv string array; `log`: `"all"` (default), `"events"`, or `"off"`, a string only; `strict`: boolean (default false); `bridges`: table array. |
| `profiles.<name>.headless` | `cwd`, `mode`, `model`, `thought_level`, `auth`: strings; `options`: map of option IDs to strings; `response_timeout`, `stop_when_idle`: nonnegative integer seconds; `mcp_servers`: table array. Omitted timeouts impose no configured deadline, and neither has a default: an unattended run must set `response_timeout`, or a request nobody answers waits as long as the process runs. A response timeout answers with the `reject_once` option, else cancels the turn; an elicitation it answers `cancel`. |
| `profiles.<name>.editor` | `experimental`: array of `send`, `context`, `cancel`, `permission`, `elicitation`, `config`, `close`; `client_capabilities`: array of `fs`, `terminal`, the editor's capabilities that reach the agent outside strict mode ([ADR 2.7](adr/0002-acp-faithfully.md#27-the-editors-file-system-and-terminal-reach-the-agent-in-strict-mode-or-when-its-profile-names-them)); `features`: array containing `shared_sessions` if enabled. All three default empty. `echo_as`: `"tool_call"` (default) or `"user_message_chunk"`, how a message or context sent through brnr is shown to the editor ([ADR 7.2](adr/0007-experimental-actions-on-an-editors-session.md#72-how-a-message-sent-through-brnr-is-shown-to-the-editor)). |
| Bridge table | `command`: the program, a nonempty string (a list fails to load); optional `args`: string array, as an MCP server's; optional `events`: event-name array (default all except `acp`). |
| MCP server table | `name` plus stdio `command`, optional `args` and string-map `env`; or remote `url`, `type` (`http` by default, or `sse`) and string-map `headers`. See [MCP servers](adr/0006-settings-and-profiles.md#63-mcp-servers-for-headless-sessions). |

[Environment variables](../README.md#environment) define `BRNR_HOME`
(transcripts and session locks), `BRNR_CONFIG` (configuration file),
`BRNR_DIR` (private runtime directory), and `BRNR_START_TIMEOUT` (startup
deadline, default 120 seconds), including their default paths. How
`BRNR_HOME` and `BRNR_DIR` are found, and the state directory's layout, are
fixed for 1.x ([ADR 3.9](adr/0003-processes-sessions-and-ownership.md#39-ownership-is-a-per-session-lock));
what host logs and lock files contain isn't. Agent executables and bridge
commands inherit the launching environment; authentication variables are
agent-specific. A started bridge additionally receives `BRNR_PID` and
`BRNR_SOCKET`.

`event notify` passes its child the full event JSON on stdin and environment
variables `BRNR_EVENT`, `BRNR_TEXT`, `BRNR_TITLE`, `BRNR_MESSAGE`,
`BRNR_SESSION`, `BRNR_REQUEST`, `BRNR_PID`. The displayed text values
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
| `status` | None | Host metadata, owner, uptime, `starting` (until the first session commits), `stopping`, `closing` (the sessions it has closed, or failed to open, and holds the locks of until their records are written, ADR 5.5), the idle setting, `capabilities`, `pending_count` and `elicitation_count`, bridge labels, and `sessions` array, each with `session`, its transcript's `events_log` and `acp_log` (null where not kept), and its state, with `held_count`, `context_count`, `pending_count` and `elicitation_count`. |
| `logged` | None | Acknowledgment after earlier recorded data has been processed by the transcript writer, with `lost`: true when a file has lost records it hasn't noted yet (a full disk); inspect recorded gaps/errors for lost data. |
| `send` | `session`; optional `text`, `blocks` (ACP content-block array), `mode` (`prompt`, `steer`, `interrupt`, `context`), `replace` (boolean). | `session`, `status`, `message`; context mode returns `status:"held"` without a message ID. |
| `cancel` | `session`; optional boolean `keep_held`. | `session`, `status`, `dropped` objects (`message`, `text`, `reason`). |
| `queue` | `session`; optional `drop` (message ID), `clear`, `clear_context` (booleans); or `show` (message ID) alone; a `show` that is not a message ID is refused. | `session`, `held`, `context`, `dropped`; with `show`, that message: `session`, `message`, `text`, `interrupt`, `attachments`, `blocks`. |
| `subscribe` | Optional `events`: array of event names or string `"all"`; a note's name (`records_skipped`, `events_missed`) is refused, as the host never sends one. | `events`: actual subscribed names. Omitted/null means all except `acp`; empty array selects none. Selection is host-wide; filter session IDs on the client. `peer`, this connection's name (`socket#7`); `host_id`; `pid`; `seq`, the last event's position before the subscription. |
| `was_cut_off` | `peer`, a connection's name as `subscribe` gave it. | `peer`; `cut_off`, whether the process cut that connection off for falling behind (of its last 64 cut-offs); `last_queued_seq`, the last position queued for it then, else null. A position only, never an event: catch up from the transcript ([ADR 10.2](adr/0010-bounded-resources.md#102-a-peer-cut-off-catches-up-from-the-transcript)). |
| `pending` | None | `pending`: approval objects for the host's sessions; `elicitations`: elicitation objects, as `elicitation show --json` has them, each with the process's `pid`. |
| `allow`, `reject` | `session`, `request`; optional `always` boolean, `option_id` string (the option's `optionId`; `option` is refused). | `session`, `request`, `outcome`. |
| `elicitation` | `session` (null for one outside any session), `request`, `action` (`accept`, `decline` or `cancel`); for an accept of a form, `content`: an object of the form's fields, checked against its schema before anything is sent. | `session`, `request`, `result`: the response sent to the agent (`action`, and an accept's `content`). |
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

Events have an `event` name and timestamp `ts`, the run `host_id` and the
process's `pid`, and `seq`, their position: one more than the last event's or
ACP message's of the host, the same in the transcript (a raw ACP record has
it before `dir`); session events have `session`. Every name and fixed value
is snake_case, named subject first (ADR 5.9). Who acted is `by` (`editor`,
`agent` or `control`), the connection that asked `peer`, and why `reason`.
`by: agent` is on a `permission_resolved` or `elicitation_resolved` whose
request the agent cancelled with `$/cancel_request` (ADR 4.5): headless,
brnr answered it `cancelled` or `cancel`; an editor answers its own. It is no
longer pending, and an `allow`, `reject` or `elicitation` for it fails.
`peer_cut_off` (`peer`, `last_queued_seq`) tells the other peers that one was
cut off. The [event catalogue](../README.md#events)
lists every event and its meaning, including message/turn correlation,
approvals, elicitations, config patches and process exits. `process_exited`, `line_too_long`
and `line_too_deep` are process events. Notes are what brnr adds to a stream of events, and are
never sent on the socket: `records_skipped` (a gap in a transcript, in `event
log`) and `events_missed` (what a command couldn't catch up on, on its
stdout). `--events` takes them, and shows them whatever it chose. `session_changed` carries `what` and `value`; config
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

Read continuously: a slow observer can be disconnected, and `peer_dropped`
in the host log says so, with `last_queued_seq`. To catch up, connect again,
ask `was_cut_off`, `subscribe` again (its `seq` is where live events start),
send `logged`, and read the host's records between your last event's `seq`
and that one from the transcript (the host log names each session's files),
as brnr's own commands do. A profile-started bridge can't: it is sent SIGTERM.
Profile bridges
get final events and then stdin EOF when their host stops; they should exit
on EOF and receive SIGTERM if still running two seconds later. Closing a
started bridge's stdout only ends its requests, not its event subscription.
See [buffering and cutoffs](../README.md#when-a-reader-falls-behind) for the
16 MiB observer queue, long-line handling and foreground skipped events.

## Files, output safety and compatibility

[Transcripts](../README.md#transcripts) documents the JSON-lines file paths,
host/session identity fields, logging modes, redaction and `records_skipped`
gap notes. Every record starts with its `format`, 1 for all of 1.x; a
record in another format, an older brnr's without one or a newer one's,
isn't read, and `event log` and `doctor` say so (ADR 5.10). Read history through `event log` where possible. A transcript is not
a guarantee of complete history: disk backlog/errors, disabled logging,
and abrupt process death can lose records.

**Covered** from 1.0.0: each session's events and raw ACP files, where they
are under `$BRNR_HOME` and their records, in format 1; a 1.x brnr reads
what an earlier 1.x wrote (ADR 5.10); and where each other file under `$BRNR_HOME` is. **Not
covered**: what host logs (`hosts/<run id>.jsonl`) and session lock files
(`sessions/<id>.lock`) contain, and what is in the runtime directory:
sockets and metadata files.

Human-readable output escapes agent control characters; JSON retains the
agent's text, so escape it before displaying it in a terminal or UI. MCP
environment/header secrets are redacted from recorded ACP, but transcripts
can contain prompts and tool output. brnr writes a transcript or host log
only where the state directory, each directory below it and the file are the
user's own with no group or other access: what others can reach is made
private before the next record (a `path_made_private` host-log record says so,
or, from `session delete`, a line on stderr), and a symlink, another user's
path, a non-regular file or one with other hard links is refused, with no
override (ADR 9.3). A refused host log fails the start; a refused session
file is a `session_log_failed` host-log record.
`doctor --report` is for review before
sharing; it does not upload anything. See the [threat model](threat-model.md)
for trust boundaries and the documented host-SIGKILL cleanup limitation.

## Compatibility

Before 1.0.0 every interface here can change outright, with no aliases or
shims (P9). From 1.0.0, what the sections above mark **Covered** is kept
compatible within a major version, and anything they don't name is not
covered ([ADR 8.4](adr/0008-the-command-line-and-compatibility.md#84-what-the-compatibility-promise-covers)). The
library on crates.io is covered as part of the command line: its public API
is a facade that mirrors it (#147, ADR 8.5); its internals, behind a feature that is off by
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
`--json` output (#150) are still being settled.
