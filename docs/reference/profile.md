# profile

The profiles in the [config file](configuration.md), read and never written
([ADR 6.5](../adr/0006-settings-and-profiles.md#65-brnr-profile-reads-the-profiles-and-never-writes)).
An agent that can't read the file, because its tool permissions or a
sandbox keep it out, reads them here. The values of MCP servers' `env` and
`headers` show as `<redacted>`, their names kept; there is no flag that
shows them. Arguments are not redacted: put secrets in `env`.

With `--remote`, each command reads the destination's config
([remote hosts](../guide/remote.md)).

## `brnr profile list`

The config file's path, then each profile with `doctor`'s line for it.

    brnr profile list [--json]

| Flag | Meaning |
|---|---|
| `--json` | Print the profiles as JSON. |

**Output** (`--json`):

| Field | Type | Meaning |
|---|---|---|
| `config` | string | The config file's path. |
| `exists` | boolean | Whether there is a file. With none, defaults apply. |
| `profiles` | list of objects | One for each profile, by name. |
| `profiles[].name` | string | Its name, as `--profile` takes it. |
| `profiles[].agent` | list or null | Its `agent`; null where it has none, and the agent is given after `--`. |
| `profiles[].bridges` | number | How many bridges it starts. |
| `profiles[].mcp_servers` | number | How many MCP servers its headless sessions get. |
| `profiles[].strict` | boolean | Its `strict`. |
| `profiles[].experimental` | list of strings | Its editor part's `experimental`. |
| `profiles[].client_capabilities` | list of strings | Its editor part's `client_capabilities`. |
| `profiles[].echo_as` | string | Its editor part's `echo_as`. |
| `profiles[].features` | list of strings | Its editor part's `features`. |

<!-- example: brnr profile list --json; before: Nothing -->
```json
{
  "config": "/home/me/.config/brnr/config.toml",
  "exists": true,
  "profiles": [
    {
      "name": "work",
      "agent": ["brnr-claude-adapter"],
      "bridges": 0,
      "mcp_servers": 0,
      "strict": false,
      "experimental": [],
      "client_capabilities": [],
      "echo_as": "tool_call",
      "features": []
    }
  ]
}
```

Text output: the file's path and how many profiles it has, or `none;
defaults apply`, then a line for each profile.

**Exit codes:** 0, also with no file; 1 if the file doesn't load.
**Errors:** `config_invalid`, with each problem and where it is.

## `brnr profile show`

A profile as `session new --profile <name>` and `acp --profile <name>`
would use it, from the file as it is now; or, with `--pid`, what a running
process was started with.

    brnr profile show [<name> | --pid <pid>] [--json]

| Argument or flag | Meaning |
|---|---|
| `<name>` | The profile. Without it, the one used without `--profile`: `default`, or the empty profile where there is none, which it says. |
| `--pid <pid>` | The process's start request, from its socket, not from the file: true of that process whatever the file says now, and there where the file can't be read. |
| `--json` | Print it as JSON. |

**Output** (`--json`), for a name:

| Field | Type | Meaning |
|---|---|---|
| `config` | string | The config file's path. |
| `name` | string | The profile's name: `default` without one. |
| `found` | boolean | Whether the file has it. `false` only without a name: the empty profile applies. |
| `profile` | object | The profile, in the file's layout. A key the file doesn't set has its default, or null where a rule applies instead; `unset` names it. [`profile keys`](#brnr-profile-keys) has the defaults and rules. |
| `profile.agent` | list or null | The agent's command, or null: the agent given after `--`. |
| `profile.log` | string | `all`, `events` or `off`. |
| `profile.strict` | boolean | Stable ACP only. |
| `profile.bridges` | list of objects | The bridges, as the file has them ([bridges](configuration.md#bridges)). |
| `profile.bridges[].command` | string | The program. |
| `profile.bridges[].args` | list of strings, optional | Its arguments. |
| `profile.bridges[].events` | list of strings, optional | The events it gets. |
| `profile.headless` | object | What `session new` and `session resume` use. |
| `profile.headless.cwd` | string or null | The sessions' working directory. |
| `profile.headless.mode` | string or null | The mode. |
| `profile.headless.model` | string or null | The model. |
| `profile.headless.thought_level` | string or null | The thought level. |
| `profile.headless.options` | object | Config options, by id. |
| `profile.headless.response_timeout` | number or null | Seconds an approval or elicitation waits. |
| `profile.headless.stop_when_idle` | number or null | Seconds until an idle session is closed. |
| `profile.headless.auth` | string or null | The login method. |
| `profile.headless.mcp_servers` | list of objects | The [MCP servers](configuration.md#mcp-servers), as the file has them, the values of `env` and `headers` `<redacted>`. |
| `profile.headless.mcp_servers[].name` | string | Its name. |
| `profile.editor` | object | What `brnr acp` uses. |
| `profile.editor.experimental` | list of strings | The experimental actions enabled. |
| `profile.editor.client_capabilities` | list of strings | The editor's capabilities that reach the agent. |
| `profile.editor.echo_as` | string | How a message sent through brnr is shown to the editor. |
| `profile.editor.features` | list of strings | The process-management features enabled. |
| `unset` | list of strings | The keys the file doesn't set, by path: `log`, `headless.cwd`. |

<!-- example: brnr profile show work --json; before: Nothing -->
```json
{
  "config": "/home/me/.config/brnr/config.toml",
  "name": "work",
  "found": true,
  "profile": {
    "agent": ["brnr-claude-adapter"],
    "log": "all",
    "strict": false,
    "bridges": [],
    "headless": {
      "cwd": null,
      "mode": null,
      "model": null,
      "thought_level": null,
      "options": {},
      "response_timeout": null,
      "stop_when_idle": null,
      "auth": null,
      "mcp_servers": []
    },
    "editor": {
      "experimental": [],
      "client_capabilities": [],
      "echo_as": "tool_call",
      "features": []
    }
  },
  "unset": ["log", "strict", "bridges", "headless.cwd", "editor.features"]
}
```

Text output: TOML in the file's layout, each part's table with who uses it,
and each key the file doesn't set marked: its default, `# not set: the
default`, or a comment with the rule, `# cwd: not set; --cwd, else where
brnr session new runs`.

**Output** (`--pid --json`):

| Field | Type | Meaning |
|---|---|---|
| `pid` | number | The process. |
| `start_request` | object | What the process was started with, as its host log's `process_started` record has it, without the prompt and `foreground`. |
| `start_request.profile` | string or null | The profile's name; null where none was given. |
| `start_request.agent` | list of strings | The agent's command as it was run. |
| `start_request.cwd` | string | The directory the process started in. |
| `start_request.strict` | boolean | Stable ACP only. |
| `start_request.log` | string | What it records. |
| `start_request.bridges` | list of objects | The bridges, each command as it was run. |
| `start_request.bridges[].command` | string | The program. |
| `start_request.bridges[].args` | list of strings | Its arguments. |
| `start_request.bridges[].events` | list of strings or null | The events it gets; null: every event but `acp`. |
| `start_request.retention` | number or null | The config's `retention`. |
| `start_request.role` | object | `headless` or `editor`, with what that role was started with. |
| `start_request.role.headless` | object, optional | A headless process's. |
| `start_request.role.headless.resume` | string or null | The session it resumed. |
| `start_request.role.headless.transcript` | boolean | Whether brnr had the resumed session's transcript. |
| `start_request.role.headless.settings` | object | What the start's flags set: `mode`, `model`, `thought_level`, `options`. |
| `start_request.role.headless.settings.mode` | string or null | `--mode`. |
| `start_request.role.headless.settings.model` | string or null | `--model`. |
| `start_request.role.headless.settings.thought_level` | string or null | `--thought-level`. |
| `start_request.role.headless.settings.options` | object | `--option`, by id. |
| `start_request.role.headless.defaults` | object | What the profile set, the same fields: the flags win over it. |
| `start_request.role.headless.defaults.mode` | string or null | The profile's `mode`. |
| `start_request.role.headless.defaults.model` | string or null | The profile's `model`. |
| `start_request.role.headless.defaults.thought_level` | string or null | The profile's `thought_level`. |
| `start_request.role.headless.defaults.options` | object | The profile's `options`. |
| `start_request.role.headless.mcp_servers` | list of objects, ACP's | The MCP servers as `session/new` sends them, the values of `env` and `headers` `<redacted>`. |
| `start_request.role.headless.auth` | string or null | The login method. |
| `start_request.role.headless.events` | list of strings | The events the start followed its first turn by (`--wait`). |
| `start_request.role.headless.start_timeout` | number | Seconds the start had. |
| `start_request.role.headless.stop_when_idle` | number or null | Seconds until an idle session is closed. |
| `start_request.role.headless.response_timeout` | number or null | Seconds an approval or elicitation waits. |
| `start_request.role.editor` | object, optional | An editor's process's. |
| `start_request.role.editor.proxy_pid` | number | The `brnr acp` process. |
| `start_request.role.editor.sigmask` | list of numbers | The editor's signal mask, which the agent got. |
| `start_request.role.editor.experimental` | list of strings | The experimental actions enabled. |
| `start_request.role.editor.client_capabilities` | list of strings | The editor's capabilities that reach the agent. |
| `start_request.role.editor.echo_as` | string | How a message sent through brnr is shown to the editor. |
| `start_request.role.editor.features` | list of strings | The process-management features enabled. |

<!-- example: brnr profile show --pid 4466 --json; before: Idle -->
```json
{
  "pid": 4466,
  "start_request": {
    "profile": null,
    "agent": ["/opt/homebrew/bin/brnr-claude-adapter"],
    "cwd": "/home/me/project",
    "strict": false,
    "log": "all",
    "bridges": [],
    "retention": null,
    "role": {
      "headless": {
        "resume": null,
        "transcript": false,
        "settings": {"mode": null, "model": null, "thought_level": null, "options": {}},
        "defaults": {"mode": null, "model": null, "thought_level": null, "options": {}},
        "mcp_servers": [],
        "auth": null,
        "events": ["agent_message", "turn_ended"],
        "start_timeout": 120,
        "stop_when_idle": null,
        "response_timeout": null
      }
    }
  }
}
```

Text output: the same, as TOML, the null fields left out.

**Exit codes:** 0; 1 if it fails.
**Errors:** `profile_not_found` (a name the file doesn't have),
`config_invalid` (a file that doesn't load, or a name with no file), as a
start fails; with `--pid`, `process_not_found`, `process_unreachable`.

## `brnr profile keys`

Every key a profile can have. It reads the same table as the check that
loads the file and as `profile show`, so the three can't disagree.

    brnr profile keys [--json]

| Flag | Meaning |
|---|---|
| `--json` | Print the keys as JSON. |

**Output** (`--json`):

| Field | Type | Meaning |
|---|---|---|
| `keys` | list of objects | One for each key, by part, in the file's layout. |
| `keys[].key` | string | Its name. |
| `keys[].part` | string | Its part: `shared` (`[profiles.<name>]`), `headless` (`[profiles.<name>.headless]`) or `editor` (`[profiles.<name>.editor]`). In another part it fails to load. |
| `keys[].type` | string | Its TOML type: `string`, `list of strings`, `array of tables: command, args, events`, … |
| `keys[].default` | any or null | Its value when the file doesn't set it; null where `rule` applies instead. |
| `keys[].rule` | string or null | What applies when the file doesn't set it and it has no default. |
| `keys[].names` | list of strings | The names it takes, as its value or its list's items; empty for a key that takes any. |
| `keys[].adr` | string | The ADR section that decides what it does. |

<!-- example: brnr profile keys --json; before: Nothing -->
```json
{
  "keys": [
    {
      "key": "agent",
      "part": "shared",
      "type": "list of strings",
      "default": null,
      "rule": "the agent given after --",
      "names": [],
      "adr": "6.1"
    },
    {
      "key": "log",
      "part": "shared",
      "type": "string",
      "default": "all",
      "rule": null,
      "names": ["all", "events", "off"],
      "adr": "5.1"
    }
  ]
}
```

Text output: each part's table, then a line for each of its keys.

**Exit codes:** 0.
