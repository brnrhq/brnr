# 12. Remote execution

Draft 2026-10-27. Implemented. Factors: F3, F5, N1, N2 and N4 (ADR 1).
Resolves #401 and, for 12.2, #409.

## 12.1 Public commands can execute through the system SSH client

`--remote <target>` before `--` executes a public command on that target.
The target is always an SSH destination in
`[user@]host` form (including an SSH alias). An explicit username is passed
to SSH; otherwise SSH's own configuration and defaults decide it. Ports,
keys, jump hosts and connection sharing belong in SSH configuration.

System `ssh` means the executable named `ssh` on the caller's PATH, not a
pinned path. This applies to every `--remote` command and `remote check`,
including read verbs. Command rules do not secure that lookup; an agent
that can plant `ssh` on its PATH can run local code without network access
or SSH keys (see the [threat model](../threat-model.md#gaps)).

The sequence is:

1. Parse the public command using the existing argument table, with
   transport selection added. A flag's value and words after `--` are
   never transport options. Help stays local and does not read config or
   connect. Invalid command-table syntax does not connect; command-specific
   validation (such as incompatible flags) runs on the destination.
2. Pass the destination straight to SSH without reading local brnr
   config. Do not resolve profiles, attachments or sessions. Without
   `--remote`, execution stays local, except that `remote check <target>`
   explicitly starts SSH to run the destination's doctor (12.2).
3. Replace the local process with system `ssh`, preserving its standard
   streams: `-T -o BatchMode=yes -o StrictHostKeyChecking=yes`. Missing
   authentication or host trust fails; the user establishes these with
   `ssh <destination>` beforehand. No keys or SSH libraries are bundled.
4. The remote shell finds `brnr` on its PATH, then probes its `_remote
   --version`, with stdin `/dev/null`. This also detects old executables
   without the remote entry point. Require exactly the local package
   version, including release-candidate suffixes. A missing `brnr`
   (`remote_brnr_not_found`) or a mismatch (`remote_version_mismatch`)
   fails before the actual command. The probe writes nothing to the
   command's stdout; its failure has stderr diagnostics and, for `--json`,
   an error object, with its code (ADR 8.7).
5. Execute remote brnr with the same public command and arguments, minus
   `--remote`. The private `_remote` entry point refuses another `--remote`
   hop, without leaving an environment marker in agents. Profiles and state
   are resolved on the destination. Never fall back locally.

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

## 12.2 `--remote` is the only transport, and a profile is read where the command runs

`--remote <target>` on the command line is the one way a command reaches
another machine for ordinary execution. `remote check <target>` explicitly
connects without that flag to check the destination. Profiles hold no routing:

- A profile is always read on the machine that runs the command. Without
  `--remote`, `--profile` is a local profile. With `--remote`, `--profile`
  goes to the destination, which loads it from its own config, for every
  verb that takes `--profile` (`acp`, `session new`, `session resume`,
  `session list`, `session delete`). A verb that takes no `--profile`
  refuses it, before anything connects (ADR 8.1).
- Without `--remote`, no command but `remote check` connects to another
  machine through brnr's transport. There is no default routing:
  `profiles.default` is a local execution profile, as every profile is.
- A profile has no `remote` table. A `remote` key is an unknown key, which
  fails to load (ADR 6.1); `brnr doctor` names it.
- The destination's brnr is `brnr` on the remote shell's PATH, and its
  `BRNR_DIR`, `BRNR_HOME` and `BRNR_CONFIG` are that shell's. They are set
  in the destination's shell environment, for example `~/.zshenv` or
  `~/.bashrc`, which a non-interactive SSH shell reads. If `brnr` is not on
  that PATH, the error says so, and where to set it
  (`remote_brnr_not_found`).
- The remote command runs where the remote shell starts, the user's home
  there; relative paths (`--file`) are from it. A start's working directory
  is `--cwd` or the destination profile's `headless.cwd`; `acp`'s sessions
  have the cwd the editor sends.
- The `_remote` entry point refuses `--remote`: one hop only.

There is no `[remotes]` registry and no `remote list` command. Hostnames,
users, keys, ports and jump hosts belong in SSH configuration.

`remote check <target> [--json]` runs the version preflight, then the
destination's doctor. It reads no local brnr configuration. The result and
exit status are doctor's interface. Checks neither write local config nor
create a missing config file or directory. Local `doctor` checks local
profiles only, and does not use SSH. Even without `--fix`, doctor runs
the three `brnr-*-adapter` programs and non-npm `muse-acp` with `--version`,
resolved beside brnr or on the caller's PATH; it reads npm adapters'
versions from `package.json`. These are program starts, not a guarantee
that the selected adapters have no side effects; `remote check` starts SSH
and runs those probes remotely.

The CLI never writes config (N1). Profiles are maintained by hand, on each
machine, and keep the private-file and symlink rules (ADR 9.4). An agent
reads them with `brnr profile` (ADR 6.5), locally or, with `--remote`, on
the destination.

**Why.** Remote execution has `--remote` on its command line, so it is
visible to an agent's tool-permission rules (N1), but prefix rules cannot
exclude it at every argument position; see the
[threat model's Gap](../threat-model.md#gaps). `remote check`
connects without that flag and runs the destination's diagnostic programs.
`--profile` has one meaning, a profile read where the command runs
(ADR 8.1); before, it had three: a local execution profile, a local routing
profile, or a destination profile. `profiles.default.remote` sent a plain
`brnr acp` or `brnr session new` to another machine, and the command line
didn't show it (P4). The overrides that a routing table held
(`executable`, `cwd`, `runtime_dir`, `home`, `config`) are the remote
shell's environment, which SSH users already keep, so brnr doesn't need
a second place for them (P12, N4). SSH already owns the connection's
identity and authentication; a remote-name registry would duplicate its
configuration (F5, P4). Commands
never install software or change host trust (P7, P15).

**Consequences.**

- A setup that used a routing profile puts `--remote <alias>` in the
  editor's or script's command line, and the destination profile's name in
  `--profile`.
- brnr must be on the remote PATH of a non-interactive shell. A brnr
  installed where only a login shell finds it fails with
  `remote_brnr_not_found`, which says how to fix it.
- Direct file access still depends on filesystem permissions; this is no
  boundary against unrestricted same-user code (P13).

**Considered.**

- Routing profiles, a `[profiles.<p>.remote]` table with the target, the
  destination profile and path overrides (the first draft of this record):
  three meanings for `--profile`, routing the command line doesn't show, and
  a second place for the remote shell's environment.
- A `[remotes]` registry and `remote list`: duplicate SSH's aliases.
- `remote init` and `remote remove`: they edit configuration, which stays
  the user's to do (N1).
- An `--executable` flag for a brnr off the remote PATH: one more transport
  flag for what the remote shell's environment already says.

## Tests

`tests/remote.rs` exercises the actual remote brnr and POSIX shell through
a fake SSH client, without requiring credentials:

- 12.1: `adr_0012_remote_sessions_stdin_and_json_use_the_destination`,
  `adr_0012_shell_quoting_preserves_prompts_and_delimiters`,
  `adr_0012_auth_missing_client_version_and_bad_targets_fail`,
  `adr_0012_direct_user_destinations_use_remote_path_without_initialisation`,
  `adr_0012_acp_has_clean_streams_and_remote_profile_resolution`,
  `adr_0012_help_and_invalid_commands_never_connect`,
  `adr_0012_event_streams_deliver_live_events_and_disconnect_without_stopping_headless`
- 12.2: `adr_0012_profile_without_remote_never_connects`,
  `adr_0012_with_remote_profile_reaches_the_destination_for_every_verb_that_takes_it`,
  `adr_0012_nested_remote_routing_is_refused`,
  `adr_0012_a_remote_table_fails_to_load_and_doctor_names_it`,
  `adr_0012_a_missing_remote_brnr_says_how_to_fix_the_path`,
  `adr_0012_remote_checks_never_write_or_require_local_configuration`,
  `adr_0012_ssh_destinations_bypass_local_config_and_legacy_registry_is_rejected`

On Linux, `adr_0012_system_ssh_authenticates_and_runs_cli_and_acp_on_linux`
uses a real loopback OpenSSH server with temporary host/client keys,
known_hosts and client configuration. The server's `SetEnv` gives the
remote shell its PATH and `BRNR_*` paths, as a destination's `~/.zshenv`
or `~/.bashrc` would. CI installs OpenSSH for this test.
The test checks that `ssh-keygen`, `/usr/sbin/sshd` and `/usr/bin/ssh`
can execute before setup, and uses the shared `skip` helper if any cannot;
this is a failure in CI or with `BRNR_TESTS_STRICT` set.

Not covered by a Rust test: real editor rendering and path agreement,
third-party SSH authentication agents, and detection time after a network
black hole. These require testing in the user's editor/SSH environment.
