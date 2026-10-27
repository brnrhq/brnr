# Remote hosts

Run any execution command on another host with `--remote`. brnr uses the
installed OpenSSH client; the remote host needs a matching brnr version,
with remote support, and its own agents, adapters and provider credentials.
Establish authentication and host trust with SSH first:

```sh
ssh agent@linux
brnr session list --remote agent@linux
brnr acp --remote agent@linux --profile coding
```

## Which machine loads the profile?

**`--remote` changes where `--profile` is looked up.** The source is the
machine where you invoke brnr; the destination is the SSH host.

| Invocation | Where the profile is loaded |
|---|---|
| `brnr acp --remote agent@linux --profile coding` | `coding` is loaded from brnr config on **linux**. Source brnr config is bypassed entirely. |
| `brnr acp --profile remote-work` | `remote-work` is loaded from brnr config on the **source**. If it has a `remote` table, that table selects the destination and its execution profile. |

In the routing example below, `remote-work` exists on the source and
`coding` exists on linux. Their names do not need to match. Adding
`--remote linux` to a command that uses `--profile remote-work` would look
for `remote-work` on linux and bypass the source profile's settings.
Local and destination execution settings are never merged.

`brnr remote check --profile remote-work` always reads the **source**
routing profile to check its connection and overrides.

## SSH destinations and routing profiles

The destination is `[user@]host` or an SSH alias. Without a username,
SSH's `User` setting or its normal default applies. Keep keys, ports and
jump hosts in `~/.ssh/config`. brnr uses `BatchMode=yes` and
`StrictHostKeyChecking=yes`: it fails rather than waiting for an invisible
authentication or host-verification prompt in an editor.

An SSH alias belongs in `~/.ssh/config`, for example:

```sshconfig
Host linux
    HostName linux.example.com
    User agent
```

brnr passes `linux` directly to SSH; it has no separate remote registry.
For a named working setup, define a local routing profile:

```toml
[profiles.remote-work.remote]
target = "linux"                         # SSH alias or [user@]hostname
profile = "coding"                       # execution profile on linux
executable = "/home/agent/.local/bin/brnr" # optional; default: brnr on remote PATH
cwd = "/home/agent/work"                 # optional remote starting directory
runtime_dir = "/run/user/1000/brnr"       # optional BRNR_DIR
# home = "/home/agent/.brnr"              # optional BRNR_HOME
# config = "/home/agent/.config/brnr/config.toml" # optional BRNR_CONFIG
```

The execution profile is defined separately in brnr config **on linux**:

```toml
[profiles.coding]
agent = ["brnr-claude-adapter"]
```

For `brnr acp --profile remote-work`, brnr loads `remote-work` on the
source, connects to linux, then loads `coding` there. Omitting `profile`
from the source's `remote` table makes ACP use the destination's `default`
profile, if configured.

Check the destination with defaults, or use the profile's overrides:

```sh
brnr remote check linux
brnr remote check --profile remote-work
brnr session list --remote linux --json
brnr event watch $s --remote linux
```

`check` runs the remote doctor and supports `--json`. It leaves local
configuration unchanged. An explicit destination uses remote defaults and
does not read local brnr config; a profile supplies brnr-specific overrides.

Then the same profile selects the connection from an editor or terminal:

```sh
brnr acp --profile remote-work
brnr session list --profile remote-work
brnr prompt send $s --profile remote-work --prompt "Continue"
```

A routing profile contains only its `remote` table. Agent settings and
credentials belong on the destination; local and remote execution settings
are not merged. Explicit `--remote` wins, and with it `--profile` names a
remote execution profile. Without either flag, only `acp`, `session new`
and `session resume` consult a routing `default` profile; other commands
stay local. An explicit profile that cannot load fails, including for
`--version`; it never falls back to local execution. Routing happens once;
another remote hop is refused.

Input, output, JSON and live event streams pass through SSH. File/image
attachments, working directories, skill installation and notification
commands refer to the remote host. There is no file transfer or path
translation: the editor must send paths that match the remote workspace.
Configured paths must be absolute, without `~`. Set `runtime_dir` in the
profile's `remote` table when SSH's environment differs from the environment
that started the remote sessions. `cwd` sets the remote starting directory,
`home` selects `BRNR_HOME`, and `config` selects `BRNR_CONFIG`.

ACP starts an editor-owned remote agent. Closing the editor or a detected
SSH disconnect ends that connection; there is no automatic reconnect or
conversion to headless ownership. A headless session survives commands and
watchers disconnecting. Signals and individual pipe closures follow SSH's
semantics, which differ from a local process. Transport failures exit 255
with SSH's stderr and may have no JSON result. Help remains local, and the
`remote check --profile` reads the selected local routing profile.

Remote settings use the same private config-file checks as execution
profiles, including support for symlinked configs. See
[ADR 12](../adr/0012-remote-execution.md) for the contract.
