# process

A brnr process runs one agent for one or more sessions: a headless one, which
`session new` starts, or an editor's, which `brnr acp` starts.

## `brnr process list`

Lists the brnr processes that are running, as found in the runtime
directory.

    brnr process list [--json]

| Flag | Meaning |
|---|---|
| `--json` | Print the list as JSON. |

**Output** (`--json`):

| Field | Type | Meaning |
|---|---|---|
| `processes` | list of objects | One for each process. |
| `processes[].pid` | number | The process's pid, as `--pid` and `process stop` take it. |
| `processes[].state` | string | `starting` (it hasn't committed its start), `running`, `stopping`, or `unreachable`: it doesn't answer. |
| `processes[].owner` | string or null | `editor` or `headless`; null when it is `unreachable`. |
| `processes[].agent` | object | The agent it runs, `{program, name, version}` ([conventions](overview.md#conventions)). `name` and `version` are null for a process that is `unreachable`. |
| `processes[].sessions` | list of objects | Its sessions, as `session list` rows with `session` and `state` only. |
| `processes[].sessions[].session` | string | The session's id. |
| `processes[].sessions[].state` | string | As in `session list`. An unreachable process's sessions are those it holds the locks of, each `unreachable`. |
| `processes[].uptime` | number or null | Seconds since it started; null when it is `unreachable`. |
| `processes[].cwd` | string | The directory it started in. Each session has a cwd of its own (`session list`). |

<!-- example: brnr process list --json; before: Idle -->
```json
{
  "processes": [
    {
      "pid": 4466,
      "state": "running",
      "owner": "headless",
      "agent": {
        "program": "fake_agent.py",
        "name": "fake-agent",
        "version": "1.2.3"
      },
      "sessions": [
        {
          "session": "sess-1",
          "state": "idle"
        }
      ],
      "uptime": 0,
      "cwd": "/home/me/project"
    }
  ]
}
```

Text output: a table of the processes, `no brnr processes` when there are
none.

**Exit codes:** 0; 1 if the runtime directory isn't private to the user.

## `brnr process stop`

Stops a process and its agent, and so all its sessions. It closes the
agent's stdin, then sends SIGTERM, then SIGKILL, to the agent's process
group. It doesn't wait for the process to exit.

    brnr process stop <pid> [--json]

| Flag | Meaning |
|---|---|
| `--json` | Print what it did as JSON. |

**Output** (`--json`):

| Field | Type | Meaning |
|---|---|---|
| `pid` | number | The process, as `process list` has it. |
| `status` | string | `stopping`. |

<!-- example: brnr process stop 4466 --json; before: Idle -->
```json
{
  "pid": 4466,
  "status": "stopping"
}
```

Text output: `stopping`.

**Exit codes:** 0 once the process is stopping; 1 if it isn't.
**Errors:** `process_not_found`, `process_unreachable`, `agent_exited` (the
agent has exited already).
