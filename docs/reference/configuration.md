# Configuration

The config file is TOML: `$BRNR_CONFIG`, else
`$XDG_CONFIG_HOME/brnr/config.toml`, else `~/.config/brnr/config.toml`.
[Profiles](../guide/profiles.md#profiles) has a complete example. A missing
file is no configuration.

- Unknown keys, keys in the wrong section, and unknown names in
  `experimental`, `client_capabilities` and `features` fail to load. `doctor`
  checks every profile.
- The file loads only if it is the user's own and neither the group nor
  others can read or write it, and its folder is the user's own and neither
  can write it. A symlink is followed, and the file it resolves to and that
  file's folder are checked. Otherwise every command that reads it fails,
  naming the path, the owner or mode, and the `chmod` or `chown` that fixes
  it (`config_invalid`), and `doctor` fails its config check the same way.
  brnr never changes the file's mode
  ([ADR 9.4](../adr/0009-security-and-privacy.md#94-the-config-file-is-read-only-if-only-you-can-change-or-read-it)).
- Without `--profile`, the `default` profile applies, if there is one.

**Covered** from 1.0.0: the TOML below, its sections, keys, values and names;
the environment variables below, `BRNR_DIR` as where the runtime directory
is, not what is in it; the `BRNR_*` variables `event notify` gives its
command; and `BRNR_PID` in a started bridge's environment, but not
`BRNR_SOCKET`. A configuration that uses a key added in a later 1.x release
doesn't load in an earlier one.

## `[profiles.<name>]`

What every process of the profile uses.

| Key | Type | Meaning |
|---|---|---|
| `agent` | list of strings | The agent's command and its arguments, used when none is given after `--`. |
| `log` | string | What the process records ([transcripts](../guide/transcripts.md)): `"all"` (the default), `"events"` (no raw ACP) or `"off"`. A string only. |
| `strict` | boolean | Stable ACP only ([ADR 2.1](../adr/0002-acp-faithfully.md)). Default `false`. |
| `bridges` | array of tables | The [bridges](#bridges) the process starts. |

## `[profiles.<name>.headless]`

What only `session new` and `session resume` use.

| Key | Type | Meaning |
|---|---|---|
| `cwd` | string | The sessions' working directory. |
| `mode` | string | The mode, set before the first prompt. |
| `model` | string | The model. |
| `thought_level` | string | The thought level. |
| `options` | table of strings | Config options, by id, to values. |
| `auth` | string | The agent's login method, run first ([ADR 4.7](../adr/0004-turns-messages-and-requests.md)). |
| `response_timeout` | integer | Seconds an approval or elicitation waits for an answer, 0 or more. Then an approval is answered with its `reject_once` option, else its turn is cancelled; an elicitation is answered `cancel`. |
| `stop_when_idle` | integer | Close a session idle this many seconds, 0 or more. |
| `mcp_servers` | array of tables | The [MCP servers](#mcp-servers) of the sessions it opens. |

A timeout that isn't set imposes no deadline, and neither has a default: an
unattended run must set `response_timeout`, or a request nobody answers waits
as long as the process runs. `--mode`, `--model`, `--thought-level`,
`--option`, `--stop-when-idle` and `--response-timeout` win over these.

## `[profiles.<name>.editor]`

What only `brnr acp` uses.

| Key | Type | Meaning |
|---|---|---|
| `experimental` | list of strings | The [actions on the editor's session](../guide/editor.md#experimental-actions) from outside that are enabled: `send`, `context`, `cancel`, `permission`, `elicitation`, `config`, `close`. Default empty. |
| `client_capabilities` | list of strings | The editor's capabilities that reach the agent outside strict mode: `fs`, `terminal` ([ADR 2.7](../adr/0002-acp-faithfully.md#27-the-editors-file-system-and-terminal-reach-the-agent-in-strict-mode-or-when-its-profile-names-them)). Default empty. |
| `features` | list of strings | `shared_sessions`, if enabled ([ADR 3.10](../adr/0003-processes-sessions-and-ownership.md)). Default empty. |
| `echo_as` | string | How a message or context sent through brnr is shown to the editor: `"tool_call"` (the default) or `"user_message_chunk"` ([ADR 7.2](../adr/0007-experimental-actions-on-an-editors-session.md#72-how-a-message-sent-through-brnr-is-shown-to-the-editor)). |

## Bridges

`[[profiles.<name>.bridges]]`: a program the process starts, which speaks the
[bridge protocol](bridge.md).

| Key | Type | Meaning |
|---|---|---|
| `command` | string | The program, not empty. A list fails to load. |
| `args` | list of strings | Its arguments, as an MCP server's. Optional. |
| `events` | list of strings | The events it gets: event names, not notes. Optional; the default is every event but `acp`. |

## MCP servers

`[[profiles.<name>.headless.mcp_servers]]`: an MCP server for the sessions
the process opens, local or remote
([ADR 6.3](../adr/0006-settings-and-profiles.md#63-mcp-servers-for-headless-sessions)).

| Key | Type | Meaning |
|---|---|---|
| `name` | string | Its name. |
| `command` | string | A local (stdio) server's program. |
| `args` | list of strings | Its arguments. |
| `env` | table of strings | Its environment. |
| `url` | string | A remote server's URL, in place of `command`. |
| `type` | string | A remote server's transport: `"http"` (the default) or `"sse"`. |
| `headers` | table of strings | A remote server's HTTP headers. |

A server has `command` or `url`, not both. `type` and `headers` are for a
`url` server, `args` and `env` for a `command` one. The values of `env` and
`headers` are redacted from the transcripts.

## Environment variables

| Variable | Meaning |
|---|---|
| `BRNR_HOME` | The state directory: transcripts, host logs and session locks. Default `~/.brnr`. |
| `BRNR_CONFIG` | The config file. Default `$XDG_CONFIG_HOME/brnr/config.toml`, else `~/.config/brnr/config.toml`. |
| `BRNR_DIR` | The private runtime directory: sockets and metadata. Default `$XDG_RUNTIME_DIR/brnr`, else `$TMPDIR/brnr-<uid>`. brnr refuses one others can use. |
| `BRNR_START_TIMEOUT` | Seconds a start has to commit. Default `120`. |

How `BRNR_HOME` and `BRNR_DIR` are found, and the state directory's layout,
are fixed for 1.x
([ADR 3.9](../adr/0003-processes-sessions-and-ownership.md#39-ownership-is-a-per-session-lock));
what host logs and lock files contain isn't. See
[environment](../guide/profiles.md#environment).

Agents and bridges inherit the environment brnr was started in; the
variables an agent logs in with are its own. A started bridge also gets
`BRNR_PID`, its process, and `BRNR_SOCKET`, the control socket. `event
notify` gives its command `BRNR_EVENT`, `BRNR_TEXT`, `BRNR_TITLE`,
`BRNR_MESSAGE`, `BRNR_SESSION`, `BRNR_REQUEST` and `BRNR_PID`
([event notify](event.md#brnr-event-notify)).
