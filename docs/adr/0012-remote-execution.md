# 12. Remote execution

Draft 2026-10-27. Implemented. Factors: F3, F5, N1, N2 and N4 (ADR 1).
Resolves #401.

## 12.1 Public commands can execute through the system SSH client

`--remote <target>` before `--` executes a public command on that target.
The target is a configured remote's exact name, or an SSH destination in
`[user@]host` form (including an SSH alias). An explicit username is passed
to SSH; otherwise SSH's own configuration and defaults decide it. Ports,
keys, jump hosts and connection sharing belong in SSH configuration.

The sequence is:

1. Parse the public command using the existing argument table, with
   transport selection added. A flag's value and words after `--` are
   never transport options. Help stays local and does not read config or
   connect. Invalid command-table syntax does not connect; command-specific
   validation (such as incompatible flags) runs on the destination.
2. Resolve an explicit destination, or a local routing profile (12.2).
   Do not resolve local execution profiles, attachments or sessions.
3. Replace the local process with system `ssh`, preserving its standard
   streams: `-T -o BatchMode=yes -o StrictHostKeyChecking=yes`. Missing
   authentication or host trust fails; the user establishes these with
   `ssh <destination>` beforehand. No keys or SSH libraries are bundled.
4. The remote shell changes directory if configured, then probes the
   configured executable's `_remote --version`, with stdin `/dev/null`.
   This also detects old executables without the remote entry point. Require
   exactly the local package version, including release-candidate suffixes.
   A missing executable or mismatch fails before the actual command. The
   probe writes nothing to the command's stdout; its failure has stderr
   diagnostics and, for `--json`, an error object.
5. Execute remote brnr with the same public command and arguments, minus
   transport selection. The private `_remote` entry point prevents another
   routing hop, without leaving an environment marker in agents. Profiles
   and state are resolved on the destination. Never fall back locally.

Every remote shell word is quoted (P8); a destination must be nonempty,
not start with `-`, and contain only ASCII letters, digits and SSH
destination punctuation (`-._@:%[]+`). It is one SSH argument. The remote
shell must support POSIX shell syntax, and shell startup files must leave
stdout clean. No local shell interprets arguments.

All execution commands support this, including `doctor`, `skill` and
`--version`. Inspection commands under `remote` remain local; they refuse
`--remote`. For `skill install`, installation is remote. Attachments,
`--cwd`, `--dir`, agent executables and `event notify` callbacks are remote
paths/programs. There is no file transfer or path translation. An editor
must send workspace paths that make sense remotely.

The remote command's stdout, stderr and ordinary exit status pass through
SSH. SSH's transport/authentication failure is status 255 with its own
stderr, and need not have brnr JSON. A remote command that itself exits
255 cannot be distinguished from a transport failure by exit status alone.
Local routing failures use brnr's normal error reporting and exit 1.

ACP still starts an editor-owned agent, through the destination's ordinary
proxy and host. Closing input is EOF to that proxy. Losing the SSH channel,
once detected, closes its pipes; normal editor-owned cleanup then applies.
There is no reconnect, retry or ownership transfer. A network black hole
can take SSH's configured keepalive/timeout interval to detect. Headless
sessions survive their submitting command and an observer disconnecting.
SSH does not promise local ACP's exact signal, wait-status or pipe-closure
semantics: a remote signal exit is represented by SSH, and individual
stdout EOF need not reach the local editor before the SSH channel ends.

**Why.** F3/F5 put the same CLI in local terminals, scripts and editors.
System SSH preserves existing authentication and trust policy with no
new crypto dependencies (N1/N4). It adds no network listener; the remote
Unix sockets remain private (P13). The network access is explicitly
requested (P15). F5's preference for external composition gives way to a
thin shared routing layer so every command and profile uses one rule.

**Considered.** Embedded SSH libraries add dependencies and a second
implementation of users' SSH configuration. Forwarding control sockets
couples clients to a private protocol. Automatic reconnect would risk
replaying prompts or approvals, and changes ownership (P11/P14).

## 12.2 Remotes are reusable connections, and profiles can delegate

`config.toml` can contain:

```toml
[remotes.linux]
host = "agent@linux"
executable = "/home/agent/.local/bin/brnr" # default: brnr on remote PATH
cwd = "/home/agent/work"                 # optional, before all commands
runtime_dir = "/run/user/1000/brnr"       # optional BRNR_DIR
home = "/home/agent/.brnr"                # optional BRNR_HOME
config = "/home/agent/.config/brnr/config.toml" # optional BRNR_CONFIG

[profiles.work.remote]
target = "linux"
profile = "coding"                       # optional remote profile
```

Each path must be absolute on the destination; there is no `~` expansion.
An executable can instead be a bare program on the remote PATH. Missing
runtime/home/config settings leave the destination's normal discovery
rules in place. SSH credentials and agent-provider credentials are not
copied or stored in remote settings.

A routing profile contains only its `remote` table: local execution
settings alongside it fail validation, rather than being silently ignored.
The target is a saved remote or SSH destination. Its optional `profile`
selects a destination execution profile; otherwise the remote default
applies. Execution settings are not merged between machines.

- Explicit `--remote` wins. With it, `--profile` is a destination profile,
  and no local profile of that name is required.
- Otherwise an explicit local `--profile` can select routing for any
  execution command. The destination profile is forwarded only where the
  command supports profiles (launches, session listing/deletion).
- Without either flag, only `acp`, `session new` and `session resume`
  consult `profiles.default.remote`, matching their existing default
  execution-profile behaviour. Observation and management stay local
  without explicit selection.
- Routing is resolved once. A destination profile that delegates again
  fails before another SSH process starts.

Remote settings are maintained manually in the config file. The remote
CLI has no command that creates, edits or deletes config. Its inspection
commands are:

- `remote list`: configured names and connection settings, read locally.
- `remote check <target>`: version preflight followed by the remote
  `doctor`, using the configured environment and executable. The normal
  doctor result and exit status are returned.

Both support `--json`. List is `{remotes: [{name, settings}]}`; check
returns doctor's interface. Neither writes local config or creates its
file or directory when missing. Config access retains the existing
private-file and symlink rules (ADR 9.4). `doctor` validates local routing
profiles without checking remote executables locally or connecting.

**Why.** Connections are reusable across profiles (N4), a profile names a
working setup (F5), and execution settings have one owner (P4/P12).
Keeping the remote CLI read-only for configuration avoids giving agents
that can invoke brnr an additional configuration-editing interface (N1).
Direct file access still depends on the agent's filesystem permissions;
this does not establish a boundary against unrestricted same-user code
(P13). Commands never install software or change host trust (P7/P15).

**Considered.** `remote init` and `remote remove`: both write configuration,
which can select programs, remote hosts and execution settings. Those
edits remain the user's responsibility, outside brnr's command interface.

## Tests

`tests/remote.rs` exercises the actual remote brnr and POSIX shell through
a fake SSH client, without requiring credentials:

- `adr_0012_remote_sessions_stdin_and_json_use_the_destination`
- `adr_0012_profiles_route_once_and_explicit_remote_wins`
- `adr_0012_shell_quoting_preserves_prompts_and_delimiters`
- `adr_0012_auth_missing_client_version_and_bad_targets_fail`
- `adr_0012_direct_user_destinations_use_remote_path_without_initialisation`
- `adr_0012_remote_inspection_never_writes_configuration`
- `adr_0012_acp_has_clean_streams_and_remote_profile_resolution`
- `adr_0012_help_and_invalid_commands_never_connect`
- `adr_0012_event_streams_deliver_live_events_and_disconnect_without_stopping_headless`
- `adr_0012_remote_environment_paths_and_default_launch_profiles_are_resolved_once`

On Linux, `adr_0012_system_ssh_authenticates_and_runs_cli_and_acp_on_linux`
uses a real loopback OpenSSH server with temporary host/client keys,
known_hosts and client configuration. CI installs OpenSSH for this test.

Not covered by a Rust test: real editor rendering and path agreement,
third-party SSH authentication agents, and detection time after a network
black hole. These require testing in the user's editor/SSH environment.
