# session

A session is the agent's conversation, named by the id the agent gave it. A
headless session is in a process that `session new` or `session resume`
starts; an editor's is in the process its `brnr acp` starts.

## `brnr session new`

Starts a process for a headless session, in the background (or the
foreground), and opens a new session in it. It sets the cwd, logs in and sets
the session's settings, then sends the prompt, if there is one. With
`--pid <pid>`, it opens the session in that running process instead.

    brnr session new [--pid <pid>] [<new flags>] [-- <agent> [args...]]

`<new flags>` are the [process flags](#process-flags), the
[session flags](#session-flags) and the [prompt flags](#prompt-flags) below.
`session resume` takes the same flags.

### Process flags

| Flag | Meaning |
|---|---|
| `--profile <p>` | The profile to use. `default` when there is one and this isn't given. |
| `--auth <method>` | A noninteractive login method of the agent's, run first. Credentials and login stay the agent's. |
| `--strict` | Stable ACP only ([ADR 2.1](../adr/0002-acp-faithfully.md)): no `--steer` into a running turn, no `session fork`. |
| `--stop-when-idle <s>` | Close the session once it has been idle for this many seconds; `0` closes it as soon as it is idle. Wins over the profile's `stop_when_idle`. The process stops with its last session, which isn't closed first. |
| `--response-timeout <s>` | Seconds an approval or elicitation waits for an answer. Wins over the profile's `response_timeout`. Then an approval is answered with its `reject_once` option, else its turn is cancelled; an elicitation is answered `cancel`. |
| `--foreground` | Stay attached and show the events as they come, as `event watch` shows them. Ctrl-C stops the session. Not with `--wait`. |
| `--quiet` | With `--foreground`: show nothing. |
| `-- <agent> [args...]` | The agent to run, in place of the profile's. |

`--pid <pid>` opens the session in a running headless process, with its
agent, profile and MCP servers. The process flags and `-- <agent>` are a
usage error with it.

- An editor's process refuses (`process_owned_by_editor`), and so does a
  process with `stop_when_idle` whose agent can't close sessions
  (`capability_missing`).
- The session, its settings (over the profile's) and the prompt commit as
  one. A setting that fails after the agent opened the session closes it
  again, and the error says so; for an agent that can't close sessions, the
  error names the session left open.
- The start timeout goes to the process, which abandons the opening the same
  way once it passes; the command waits 10 s longer. A process stopping
  meanwhile fails it. A command gone first (Ctrl-C) leaves no session. None
  of these sends the prompt.
- A process with a session being opened doesn't stop as its other sessions
  close or go idle meanwhile.

### Session flags

| Flag | Meaning |
|---|---|
| `--cwd <dir>` | The session's working directory. By default the profile's `cwd`, else here; with `--pid`, here (`resume`: the session's recorded cwd first). |
| `--mode <m>` | The agent's mode: its config option of category `mode`, else `session/set_mode` with ACP's v1 modes. |
| `--model <m>` | The config option of category `model`. |
| `--thought-level <l>` | The config option of category `thought_level`. |
| `--option <o>=<v>` | Config option `<o>`, by its id. Repeatable. |

These are set before the first prompt, and win over the profile's, setting
by setting. Two values for one setting fail before anything is sent, as do a
mode, model or thought level the agent has no option for
([`config set`](config.md#brnr-config-set) has the rules).

### Prompt flags

| Flag | Meaning |
|---|---|
| `--prompt <text>` | The prompt, sent as given (`--prompt -h` sends `-h`). An empty one is a usage error. |
| `--prompt -` | Read the prompt from stdin. An empty one fails, 1 (`input_invalid`). |
| `--file <path>` | Attach a file, as an ACP `resource_link`. Repeatable. |
| `--image <path>` | Attach a PNG, JPEG, GIF or WebP image, as an ACP `image` block. Repeatable. |
| `--wait` | Wait for the turn to end, and print the agent's reply on stdout. Needs a prompt or an attachment. |
| `--timeout <s>` | With `--wait`: give up waiting after this many seconds, exit 124. Without `--wait` it is a usage error. It doesn't end the session or its turn. |
| `--json` | Print the result as JSON. |

A word left over, such as a prompt given as words without `--prompt`, is a
usage error. A file or image that can't be read stops the command before
anything is sent (`input_invalid`).
[Files and images](../adr/0006-settings-and-profiles.md#64-files-and-images-in-a-message)
has their encoding and what the agent must support.

With `--wait`, approvals, elicitations and the startup line (`started
sess-1 (process 4466)`) go to stderr, so that stdout is the agent's reply and
nothing else (`brnr session new --wait … > answer.md`).

**Output** (`--json`, without `--wait`):

| Field | Type | Meaning |
|---|---|---|
| `session` | string | The new session's id. |
| `pid` | number | Its process. |
| `message` | string or null | The prompt's message id; null without a prompt. |

<!-- example: brnr session new --json --prompt "reply hi" -- brnr-claude-adapter; before: Nothing -->
```json
{
  "session": "sess-1",
  "pid": 4466,
  "message": "m1"
}
```

**Output** (`--wait --json`): one object, once the turn has ended. A command
can fail before there is one.

| Field | Type | Meaning |
|---|---|---|
| `session` | string | The session. |
| `pid` | number | Its process. |
| `message` | string | The prompt's message id. |
| `reply` | string | The agent's messages in the turn, joined by a blank line. |
| `stop_reason` | string or null | ACP's stop reason, `end_turn` for a turn that ended normally; null if the turn failed or the message was dropped. |
| `error` | object or null | The turn's failure, `{code, message}`: `agent_failed` with the agent's as `rpc_error` ([errors](errors.md)); null if it didn't fail. |
| `tokens` | object or null | What the agent reported the turn used, as [`turn_ended`](events.md#turn_ended) has it; null if it reported nothing. |
| `dropped` | list of objects | Empty, or the message, dropped before it was sent, `{message, text, reason}` ([conventions](overview.md#conventions)). |

<!-- example: brnr session new --wait --json --prompt "reply hi" -- brnr-claude-adapter; before: Nothing -->
```json
{
  "session": "sess-1",
  "pid": 4466,
  "message": "m1",
  "reply": "hi",
  "stop_reason": "end_turn",
  "error": null,
  "tokens": null,
  "dropped": []
}
```

**Output** (`--foreground --json`): one [event](events.md) per line, as
`event watch --json` prints them.

Text output: `started sess-1 (process 4466)`; with `--wait`, the agent's
reply.

**Exit codes:** 0 once the session is open (with `--wait`: once its turn has
ended with `end_turn`); 1 if it failed to start, or (with `--wait`) the turn
failed, stopped otherwise or its message was dropped; 124 when `--timeout`
passes; 3 when it was cut off and couldn't catch up; 2 for a usage error.
With `--foreground`, the agent's exit status; a failed start is nonzero.
[Exit codes](exit-codes.md) has the rules.
**Errors:** `config_invalid`, `profile_not_found`, `agent_not_started`,
`acp_version_unsupported`, `agent_exited`, `agent_failed`, `setting_invalid`,
`capability_missing`, `input_invalid`, `start_timed_out`; with `--pid`, also
`process_not_found`, `process_unreachable`, `process_starting`,
`process_stopping`, `process_owned_by_editor`.

## `brnr session resume`

Starts a process and carries on an existing session in it: `session/resume`,
or `session/load` where the agent has only that. It takes the flags of
[`session new`](#brnr-session-new), and prints the same.

    brnr session resume [--pid <pid>] <session> [--take-over] [<new flags>] [-- <agent> [args...]]

| Flag | Meaning |
|---|---|
| `--take-over` | Ask the process that holds the session to close it first, as `session close` does. With `--pid`, it first checks that the process would take it: it isn't starting or stopping, and its agent can resume sessions. |

Which agent and cwd it uses:

- A session brnr has a transcript of resumes with the cwd, agent and profile
  of the newest record of all its files, in every project folder. `--cwd`,
  `--profile` and `-- <agent>` override them.
- A session brnr has no transcript of (one only the agent knows) is resumed
  by its id with the agent after `--`, else the profile's (`--profile`, else
  `default`); with neither, it fails (`agent_not_started`). Its cwd is
  `--cwd`, else the profile's `cwd`, else the current directory.
- With `--pid`, the agent is that process's. The cwd is `--cwd`, else the
  recorded cwd, else the current directory.

With `--pid`, a session open in that process already is refused, and one
another process holds is refused unless `--take-over`.

**Output** (`--json`, without `--wait`); with `--wait`, as
[`session new --wait --json`](#brnr-session-new):

| Field | Type | Meaning |
|---|---|---|
| `session` | string | The session's id. |
| `pid` | number | Its process. |
| `message` | string or null | The prompt's message id; null without a prompt. |

<!-- example: brnr session resume $s --json; before: Ended -->
```json
{
  "session": "sess-1",
  "pid": 4466,
  "message": null
}
```

**Exit codes:** as `session new`.
**Errors:** as `session new`, and `session_active` (open in another process,
without `--take-over`), `session_closing`.

## `brnr session status`

Reads a running session's current state.

    brnr session status <session> [--json]

| Flag | Meaning |
|---|---|
| `--json` | Print the state as JSON. |

**Output** (`--json`):

| Field | Type | Meaning |
|---|---|---|
| `session` | string | The session's id. |
| `title` | string or null | Its title, as the agent gave it; null if it gave none. |
| `pid` | number | The process that serves it. |
| `agent` | object | `{program, name, version}` ([conventions](overview.md#conventions)). |
| `agent.program` | string or null | The file name of the command brnr ran. |
| `agent.name` | string or null | The agent's name, from `initialize`'s `agentInfo`. |
| `agent.version` | string or null | The agent's version, from `initialize`'s `agentInfo`. |
| `owner` | string | `editor` or `headless`. |
| `lock_pid` | number or null | The process that holds its lock; null if none does. |
| `shared_pids` | list of numbers | The processes that serve it without its lock ([ADR 3.9](../adr/0003-processes-sessions-and-ownership.md#39-ownership-is-a-per-session-lock)). What happens there isn't in its transcript. |
| `lock_error` | string or null | Why an editor's session is served without its lock; null if it has it. |
| `cwd` | string | Its working directory. |
| `forked_from` | string or null | The session it was forked from; null if it isn't a fork. |
| `state` | string | `idle`, `busy` (a turn runs) or `waiting` (an approval or an elicitation waits for an answer). |
| `turn_elapsed` | number or null | Seconds the running turn has run; null when none runs. |
| `mode` | string or null | The current mode; null if the agent has none. |
| `model` | string or null | The current model; null if the agent has no model option. |
| `tools` | list of objects | The running turn's tool calls the agent hasn't finished, one for each id, as [`tool_call`](events.md#tool_call) has them. Once a turn ends, cancelled or failed too, none of its calls is listed. |
| `plan` | list or null, ACP's | The agent's last plan entries, as ACP has them; null before any. |
| `pending_count` | number | Approvals waiting (`permission requests` lists them). |
| `elicitation_count` | number | Elicitations waiting (`elicitation requests` lists them). |
| `held_count` | number | Messages held (`queue list` lists them). |
| `context_count` | number | Context entries held for the next prompt. |
| `usage` | object or null | The context window and cost, as [`usage`](events.md#usage) has them; null before the agent reports any. |
| `last_turn` | object or null | How the last turn ended; null before the first. |
| `last_turn.stop_reason` | string or null | As `turn_ended` has it. |
| `last_turn.error` | object or null | As `turn_ended` has it. |
| `last_turn.tokens` | object or null | As `turn_ended` has it. |
| `last_message` | string or null | The agent's last message, at most 4000 characters of it, then `…`; the events have all of it. Null before any. |
| `last_active` | string | When it last had an event. |
| `stop_when_idle` | number or null | The process's idle timeout in seconds; null if it has none. |
| `stopping` | boolean | Whether the process is stopping. |
| `uptime` | number | Seconds since the process started. |

The `_count` fields are counts. Use `permission requests`, `elicitation
requests` and `queue list` for what they count. Tool, plan, usage and token
data come from the agent.

<!-- example: brnr session status $s --json; before: Idle -->
```json
{
  "session": "sess-1",
  "title": "Fake session",
  "pid": 4466,
  "agent": {
    "program": "fake_agent.py",
    "name": "fake-agent",
    "version": "1.2.3"
  },
  "owner": "headless",
  "lock_pid": 4466,
  "shared_pids": [],
  "lock_error": null,
  "cwd": "/home/me/project",
  "forked_from": null,
  "state": "idle",
  "turn_elapsed": null,
  "mode": "default",
  "model": "small",
  "tools": [],
  "plan": null,
  "pending_count": 0,
  "elicitation_count": 0,
  "held_count": 0,
  "context_count": 0,
  "usage": null,
  "last_turn": {
    "stop_reason": "end_turn",
    "error": null,
    "tokens": null
  },
  "last_message": "hi",
  "last_active": "2026-10-23T15:00:54.541419Z",
  "stop_when_idle": null,
  "stopping": false,
  "uptime": 0
}
```

Text output: the same, line by line.

**Exit codes:** 0; 1 if it fails.
**Errors:** [those of a running session](errors.md#a-running-session).

## `brnr session fork`

Copies a session into a new one in the same headless process, if the agent
can fork sessions (`session/fork`). Not in an editor's process, nor in strict
mode.

    brnr session fork <session> [--json]

**Output** (`--json`):

| Field | Type | Meaning |
|---|---|---|
| `session` | string | The new session's id. |
| `forked_from` | string | The session it was forked from. |

<!-- example: brnr session fork $s --json; before: Idle -->
```json
{
  "session": "sess-2",
  "forked_from": "sess-1"
}
```

Text output: `forked sess-1 into sess-2`.

**Exit codes:** 0; 1 if it fails.
**Errors:** [those of a running session](errors.md#a-running-session),
`process_owned_by_editor`, `strict_mode_refused`, `capability_missing` (the
agent can't fork, or, with `stop_when_idle`, can't close sessions),
`agent_failed`.

## `brnr session close`

Cancels a running turn and closes the session (`session/close`). It is done
once the agent has closed it. The last headless session closing stops its
process.

    brnr session close <session> [--json]

**Output** (`--json`):

| Field | Type | Meaning |
|---|---|---|
| `session` | string | The session closed. |

<!-- example: brnr session close $s --json; before: Idle -->
```json
{
  "session": "sess-1"
}
```

Text output: `closed sess-1`.

**Exit codes:** 0; 1 if it fails.
**Errors:** [those of a running session](errors.md#a-running-session),
`capability_missing` (the agent can't close sessions), `experimental_disabled`
(an editor's session), `agent_failed`.

## `brnr session delete`

Deletes the agent's copy of an inactive session (`session/delete`), through
the agent recorded for it, which it starts for that, or the one `--profile`
or `-- <agent>` names. brnr's transcript stays, with a `session_deleted`
record, unless `--purge`.

    brnr session delete <session> [--purge] [--profile <p>] [--json] [-- <agent> [args...]]

| Flag | Meaning |
|---|---|
| `--purge` | Also delete brnr's transcript of the session: its events and raw ACP files in every project folder, an older brnr's too, and those set aside beside them (`<id>.older.jsonl`, `<id>.older.acp.jsonl`, …), whatever the agent answered. Not the host logs. |
| `--profile <p>` | The agent of this profile, for a session brnr has no transcript of. |
| `-- <agent> [args...]` | This agent. |
| `--json` | Print what it did as JSON. |

A session open in a process is refused: close it first. So is an agent
without `sessionCapabilities.delete`. The session's lock is held while the
agent deletes it. A transcript made private first is said on stderr,
`path_made_private: <path> (its mode was …)`.

**Output** (`--json`):

| Field | Type | Meaning |
|---|---|---|
| `session` | string | The session. |
| `deleted` | boolean | Whether the agent deleted it. |
| `error` | object or null | Why the agent didn't: `agent_failed` with the agent's as `rpc_error`, or why there is no answer; null if it did. |
| `recorded_in` | list of strings | The transcript files `session_deleted` was recorded in (without `--purge`). |
| `purged` | list of strings | The files deleted (with `--purge`). |
| `failed` | list of strings | Why a file wasn't deleted (with `--purge`) or recorded in (without). |

<!-- example: brnr session delete $s --json; before: Ended -->
```json
{
  "session": "sess-1",
  "deleted": true,
  "error": null,
  "recorded_in": [
    "/home/me/.brnr/projects/-home-me-project-abd1481c/sess-1.jsonl"
  ],
  "purged": [],
  "failed": []
}
```

Text output: `deleted sess-1; brnr's transcript of it stays (brnr event log
sess-1)`, or with `--purge` the files deleted.

**Exit codes:** 0 once the agent has deleted the session. 1 if it didn't (its
error, or no answer), with `--purge` too, but for an agent that doesn't have
it (ACP's `resource_not_found`, `-32002`): then, with `--purge`, 0, brnr's
transcript deleted. The pinned `brnr-claude-adapter`
([adapters/package.json](https://github.com/brnrhq/brnr/blob/main/adapters/package.json))
doesn't answer that: it answers `-32603`. 1 too if a file couldn't be deleted
or recorded in; the others are done all the same.
**Errors:** `session_active`, `session_closing`, `agent_not_started` (no agent
recorded or named), `capability_missing`, `agent_failed`.

## `brnr session prune`

Deletes brnr's transcripts of the sessions last active more than `<s>`
seconds ago, and the host logs of the processes that have gone and were,
as `session delete --purge` deletes them. Nothing is asked of an agent: its
own copy of a session stays. By default brnr keeps everything; the
config's [`retention`](configuration.md#retention) has every process do
this as it starts
([ADR 9.5](../adr/0009-security-and-privacy.md#95-transcripts-are-kept-until-they-are-pruned)).

    brnr session prune --older-than <s> [--dry-run] [--json]

| Flag | Meaning |
|---|---|
| `--older-than <s>` | The age, in seconds: what was last active before this many seconds ago goes. `0` is every session no process has open. Needed. |
| `--dry-run` | Say what would be deleted, and delete nothing. |
| `--json` | Print what it did as JSON. |

- A session's age is its last activity, `session list`'s `last_active`:
  the newest record a process wrote in any of its files, not a deletion.
- Never pruned: a session open in a process or whose lock is held, the
  host log of a running process, and a file brnr can't read the age of (in
  another format, or with no record).
- A session's lock is held while its files go, and its age read again: one
  used since it was listed stays. Its events file goes last.

**Output** (`--json`):

| Field | Type | Meaning |
|---|---|---|
| `older_than` | number | `--older-than`, in seconds. |
| `before` | string | The time it gives: what was last active before it goes. |
| `dry_run` | boolean | Whether nothing was deleted (`--dry-run`). |
| `sessions` | list of objects | One for each session pruned. |
| `sessions[].session` | string | The session's id. |
| `sessions[].last_active` | string | When it was last active. |
| `sessions[].files` | list of strings | Its files deleted, or that would be. |
| `host_logs` | list of objects | One for each host log pruned. |
| `host_logs[].host_id` | string | The process's run. |
| `host_logs[].last_active` | string | Its newest record's time. |
| `host_logs[].files` | list of strings | The host log, deleted or that would be. |
| `failed` | list of strings | Why a file wasn't deleted, or a session's lock taken. |

<!-- example: brnr session prune --older-than 0 --dry-run --json; before: Ended -->
```json
{
  "older_than": 0,
  "before": "2026-10-24T15:00:54.541419Z",
  "dry_run": true,
  "sessions": [
    {
      "session": "sess-1",
      "last_active": "2026-10-24T15:00:54.120335Z",
      "files": [
        "/home/me/.brnr/projects/-home-me-project-abd1481c/sess-1.acp.jsonl",
        "/home/me/.brnr/projects/-home-me-project-abd1481c/sess-1.jsonl"
      ]
    }
  ],
  "host_logs": [
    {
      "host_id": "20261024T150050-4466",
      "last_active": "2026-10-24T15:00:54.302114Z",
      "files": [
        "/home/me/.brnr/hosts/20261024T150050-4466.jsonl"
      ]
    }
  ],
  "failed": []
}
```

Text output: `pruned what was last active before 2026-10-24 15:00:54Z (more
than 0s ago):`, then each session and host log with its last activity and
its files; `would prune` with `--dry-run`; or `nothing was last active
before …`. Each file not deleted is on stderr, `brnr: not deleted: <why>`.

**Exit codes:** 0; 1 if a file couldn't be deleted, the others deleted all
the same, or if it fails.
**Errors:** `unclassified` (a runtime directory that can't be read: which
sessions are open must be known).

## `brnr session list`

brnr's sessions, open and with a transcript, in every cwd, with nothing
started. With `--profile` or `-- <agent>`, only the cwd's, joined on the
session id with the agent's own list (`session/list`, every page, from the
agent started just for that).

    brnr session list [--include active,inactive] [--profile <p>] [--cwd <dir>] [--json] [-- <agent> [args...]]

| Flag | Meaning |
|---|---|
| `--include active,inactive` | `active` keeps the sessions open in a process (`idle`, `busy`, `waiting`, `unreachable`), `inactive` the rest. Both by default. |
| `--profile <p>` | Also ask this profile's agent for its sessions. |
| `--cwd <dir>` | Only this directory's sessions. Here when an agent is asked and this isn't given. |
| `-- <agent> [args...]` | Also ask this agent for its sessions. |
| `--json` | Print the list as JSON. |

- A cwd is the same however it is spelled (a trailing slash, a symlink).
- With an agent, a session open in a process that the agent lists in the cwd
  is the cwd's, whatever its recorded cwd. An `unreachable` session is the
  cwd's if the agent lists it there or brnr has its transcript there.
- An agent that can't list sessions fails the command (`capability_missing`).

**Output** (`--json`): rows most recently active first.

| Field | Type | Meaning |
|---|---|---|
| `sessions` | list of objects | One for each session. |
| `sessions[].session` | string | The session's id. |
| `sessions[].title` | string or null | Its title, from its process or the agent's list; null where neither gives one (an inactive session only brnr knows). |
| `sessions[].state` | string | `idle`, `busy`, `waiting` (as `session status`), `unreachable` (its process doesn't answer) or `inactive` (open in no process). |
| `sessions[].pid` | number or null | Its process; null when `inactive`. |
| `sessions[].agent` | object | `{program, name, version}`. `name` and `version` only for a session open in a process that answers, else null. |
| `sessions[].agent.program` | string or null | The file name of the command brnr ran; null where brnr doesn't know it. |
| `sessions[].agent.name` | string or null | The agent's name, from `initialize`'s `agentInfo`. |
| `sessions[].agent.version` | string or null | The agent's version, from `initialize`'s `agentInfo`. |
| `sessions[].source` | string | Who knows it: `brnr`, `agent` or `both`. |
| `sessions[].last_active` | string or null | When it was last active. An agent's `updatedAt` is converted to UTC from its offset. Null where it isn't known. |
| `sessions[].forked_from` | string or null | The session a fork came from, as `session status` has it; once its process has ended, from its transcript's first record. Null for one that isn't a fork, and for a row only the agent knows. |
| `sessions[].cwd` | string or null | Its working directory; null where it isn't known. |

Rows are ordered by the time `last_active` names. An agent's `updatedAt` that
isn't an RFC 3339 time is passed on as the agent gave it, after the rows with
a time, and said on stderr. A session only the agent knows is `inactive`,
with `pid` null. `forked_from` is the session a fork came from, by `session
fork` or an editor's `session/fork` through `brnr acp`.

<!-- example: brnr session list --json; before: Idle -->
```json
{
  "sessions": [
    {
      "session": "sess-1",
      "title": "Fake session",
      "state": "idle",
      "pid": 4466,
      "agent": {
        "program": "fake_agent.py",
        "name": "fake-agent",
        "version": "1.2.3"
      },
      "source": "brnr",
      "last_active": "2026-10-23T15:00:54.541419Z",
      "forked_from": null,
      "cwd": "/home/me/project"
    }
  ]
}
```

Text output: a table of the sessions, the time to the second.

**Exit codes:** 0; 1 if it fails.
**Errors:** `config_invalid`, `profile_not_found`, `agent_not_started`,
`acp_version_unsupported`, `capability_missing`, `agent_failed`.
