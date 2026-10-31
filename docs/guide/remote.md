# Remote hosts

Run any execution command on another host with `--remote`. brnr uses the
`ssh` on the caller's PATH; the remote host needs a matching brnr version,
with remote support, on its shell's PATH, and its own agents, adapters and
provider credentials. Establish authentication and host trust with SSH
first. Command rules do not secure this executable lookup: a planted `ssh`
can run local code without network access or SSH keys. See
[restricted agents](agents.md#brnr-from-a-restricted-agent).


```sh
ssh agent@linux
brnr session list --remote agent@linux
brnr acp --remote agent@linux --profile coding
```

## Which machine loads the profile?

**A profile is loaded on the machine that runs the command.** The source
is the machine where you invoke brnr; the destination is the SSH host.

| Invocation | Where the profile is loaded |
|---|---|
| `brnr acp --profile coding` | `coding` is loaded from brnr config on the **source**. brnr starts the agent locally, without SSH routing. |
| `brnr acp --remote agent@linux --profile coding` | `coding` is loaded from brnr config on **linux**. Source brnr config is not read. |

`--remote` on the command line selects execution on another machine;
`remote check <target>` also connects, to check its installation. A profile
holds no routing, and a profile named `default` is the source's
default, nothing more. With `--remote`, `--profile` goes to the
destination for every command that takes it: `acp`, `session new`,
`session resume`, `session list` and `session delete`. A command that
takes no `--profile` refuses it. To see the destination's profile, ask it:

```sh
brnr profile show coding --remote linux
```

## SSH destinations

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
The execution profile is defined in brnr config **on linux**:

```toml
[profiles.coding]
agent = ["brnr-claude-adapter"]

[profiles.coding.headless]
cwd = "/home/agent/work"
```

Check the destination, then use it:

```sh
brnr remote check linux
brnr session list --remote linux --json
brnr event watch $s --remote linux
```

`check` starts system SSH and runs the remote doctor, including its adapter
`--version` probes, and supports `--json`. It reads no local brnr config,
and leaves it unchanged. The probes do not constrain what the selected
executables can do.

## The destination's environment

SSH runs the command in a non-interactive shell on the destination. brnr is
the `brnr` on that shell's PATH, and its paths are that shell's `BRNR_DIR`,
`BRNR_HOME` and `BRNR_CONFIG`, or their defaults. A non-interactive shell
reads `~/.zshenv` (zsh) or `~/.bashrc` (bash), not `~/.zprofile` or
`~/.bash_profile`. Set them there, on the destination:

```zsh
# ~/.zshenv on linux
export PATH="$HOME/.local/bin:$PATH"
export BRNR_DIR="/run/user/1000/brnr"
```

If brnr isn't on that PATH, the command fails with
`remote_brnr_not_found`, and says where to set it. Another brnr version
there fails with `remote_version_mismatch`. Set `BRNR_DIR` when the SSH
shell's environment differs from the one that started the remote sessions.

## What runs where

Input, output, JSON and live event streams pass through SSH. File/image
attachments, working directories, skill installation and notification
commands refer to the remote host. There is no file transfer or path
translation: the editor must send paths that match the remote workspace.
The remote command starts in the remote home directory; a session's
working directory is `--cwd` or the destination profile's `headless.cwd`.

ACP starts an editor-owned remote agent. Closing the editor or a detected
SSH disconnect ends that connection; there is no automatic reconnect or
conversion to headless ownership. A headless session survives commands and
watchers disconnecting. Signals and individual pipe closures follow SSH's
semantics, which differ from a local process. Transport failures exit 255
with SSH's stderr and may have no JSON result. Help remains local. The
destination runs the command itself: another `--remote` there is refused.

See [ADR 12](../adr/0012-remote-execution.md) for the contract.
