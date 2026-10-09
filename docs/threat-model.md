# Threat model

brnr runs a coding agent in a process of its own and lets other commands
reach it over a Unix socket, including answering the agent's approvals from
outside the editor. That makes it a security boundary: whoever can reach the
process can act as the agent's user. This document says who can reach what,
how brnr keeps it so, and which test checks each claim. Where a claim has no
enforcement or no test, it is listed as a gap. [SECURITY.md](../SECURITY.md)
says what is in scope for a report; the principles cited are
[ADR 1](adr/0001-principles.md)'s.

## Assets

- **The agent's actions on the user's machine.** The agent runs as the user:
  it edits files and runs commands. Whoever can prompt it, or answer its
  approvals, directs those.
- **Approvals.** A permission request is the agent asking before it acts. An
  answer given by the wrong party, or given when nobody answered, is an
  action the user didn't take.
- **Prompts and code in transcripts.** `~/.brnr` keeps every session's events
  and raw ACP (ADR 22): prompts, the agent's replies, tool calls with their
  input and output, diffs of the user's code.
- **Secrets.** The values of a profile's or an editor's MCP servers' `env`
  and `headers` (tokens, `Authorization`), which go to the agent in the
  requests that open a session (ADR 25).

## Actors

| Actor | Trusted with | Where it stands |
|---|---|---|
| Other local users | nothing | outside: kept out of the runtime directory, sockets and transcripts by file modes (P13) |
| Root | everything | outside brnr's reach; can do anything as any user |
| Processes running as the same user | everything the user can do | **inside** the boundary (P13): see below |
| The editor | its session | inside; it runs `brnr acp`, and its bytes pass unchanged (P1) |
| The agent (the adapter and what it runs) | the user's machine, as far as its own permission prompts go | inside as a process; its *text* is untrusted (P8) |
| Bridges and `notify` commands | everything a socket client can do | inside: run as the user from the user's config; what they forward leaves the machine (P13) |
| A malicious adapter or npm package | as the agent | inside once run: brnr can't contain it |

Processes running as the same user are inside the boundary, and that is a
deliberate choice, not a gap: P13 is "only you", and any process of the
user's is the user to the kernel. It can open the control socket, approve
whatever is waiting, read the transcripts, edit the config to add a bridge,
or `ptrace` the agent. brnr has nothing to tell such a process from the user
at a terminal, and doesn't pretend to; SECURITY.md lists "anyone who can
already run code as the user" as out of scope. What brnr does keep apart
within the user is protocol, not access: actions on an editor's session are
experimental and refused unless enabled by name (ADR 4), and strict mode has
none of them (ADR 41). Those protect the editor's view from surprises, not
the user from their own processes.

## Trust boundaries

```text
 other local users, root (outside: file modes, P13)
- - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - -
 the user                              (B1)
                          $BRNR_DIR, 0700 ── <pid>.sock, 0600
                                    │      └─ brnr send / approve / watch, any
                                    │         process of the user's
 editor ──stdio── brnr acp ──socketpairs── brnr process ──pipes── agent
          (B3: bytes unchanged,             │   │              (B4: its text is
           but the named changes)           │   │               untrusted, P8)
                                            │   └── bridges, notify ──► Slack, push, …
                                            │       (B5: beyond the machine is theirs)
                          ~/.brnr, 0700 ── transcripts and host logs, 0600
                                   (B2; secrets redacted, B6)
```

- **B1**, the runtime directory and the control sockets: between other local
  users and the process.
- **B2**, the transcripts and host logs: between other local users and what
  the sessions said.
- **B3**, `brnr acp`: between the editor and the agent, where brnr must be
  invisible (P1).
- **B4**, the agent: its text (session ids, titles, commands, messages) and
  its permission requests reach brnr's files, terminal and approvals.
- **B5**, bridges and `notify` commands: the edge of the machine.
- **B6**, secrets: what brnr records and sends of the MCP servers' values.

## Claims

Each claim with what enforces it (file and function) and the test that checks
it. Tests are in `tests/` unless a module is named; `security.rs` holds those
written for this document.

### B1: the runtime directory and sockets refuse others

| Claim | Enforced by | Tested by |
|---|---|---|
| The runtime directory is created 0700 and the process refuses one that isn't a directory owned by the user with no group or other bits, or is a symlink; it then writes nothing there and starts no agent. | `paths::ensure_private`, `paths::check_private` (src/paths.rs), from `Host::start` (src/host/mod.rs) | `security.rs`: `a_runtime_dir_others_can_use_is_refused`, `a_runtime_dir_of_another_users_is_refused`; `headless.rs`: `runtime_dir_symlink_is_refused` |
| Every command that reads the runtime directory refuses one others can use, or a symlink, before it trusts anything there: no request, approval included, is sent through it. | `discover` (src/ctl.rs) | `security.rs`: `a_runtime_dir_others_can_use_is_refused`, `a_symlinked_runtime_dir_is_refused_by_every_command`, `no_approval_goes_through_a_dir_others_can_use`; `headless.rs`: `shared_runtime_dir_is_refused` |
| The control socket is 0600, the directory 0700, the session locks' directory 0700 and each lock 0600, whatever the umask. | `Host::start` (bind, then `set_permissions` 0600); `lock::take` (src/lock.rs) | `security.rs`: `what_brnr_makes_is_private_whatever_the_umask` |
| Another user can't connect to the socket: the directory and the socket's mode both refuse them. | the kernel, given the modes above | `security.rs`: `another_user_cant_connect` (runs as root only, connecting as `nobody`) |
| A metadata file counts only with the socket next to it, so a planted file can't point a command at another socket. | `read_meta` (src/ctl.rs) | `headless.rs`: `metadata_names_its_own_socket` |
| Session locks are held to the runtime directory's terms and never followed through a symlink. A headless start, resume or fork that can't take its lock is refused (ADR 50). | `lock::take` (`ensure_private`, `O_NOFOLLOW`); `own`, `take_lock` (src/host/acp.rs); `host_request_done`, `peer_result` (src/host/requests.rs) | `security.rs`: `adr_0003_session_locks_are_private_and_never_followed`; `cli.rs`: `adr_0050_a_new_session_that_cant_be_locked_isnt_started`, `adr_0050_a_resume_that_cant_be_locked_isnt_started`, `adr_0050_a_fork_that_cant_be_owned_is_refused` |
| brnr listens on no network. | the process binds only a `UnixListener` (src/host/mod.rs); ADR 44 | `security.rs`: `the_process_listens_on_no_network` (with `lsof`, where installed) |
| `doctor` fails a runtime directory others can use, and `--fix` tightens it but never through a symlink. | `runtime_dir`, `private_problem`, `fixable` (src/ctl/doctor.rs) | `doctor.rs`: `open_runtime_dir_fails_until_fixed`, `symlinked_runtime_dir_is_not_fixed` |

### B2: transcripts are private

| Claim | Enforced by | Tested by |
|---|---|---|
| What brnr creates under `~/.brnr` is 0700 (directories) and 0600 (files), whatever the umask. | `open_private` (src/log.rs) | `headless.rs`: `transcripts_are_private`; `security.rs`: `what_brnr_makes_is_private_whatever_the_umask`; src/log.rs: `adr_0059_new_and_private_paths_are_opened_as_they_are` |
| Before a record goes into a session's events, its raw ACP or a host log, the state directory, each directory below it and the file are the user's and have no group or other bits, however they came to exist: one others can reach is made private through the descriptor that is then written to (`fchmod`), beneath private parents or traversable ones, and the host log records `made-private` with the mode it had. There is no override (ADR 59). | `open_private`, `keep_private` (src/log.rs), the one open of all three (`open_append`); `Writer::made_private` | `security.rs`: `adr_0059_a_transcript_others_can_read_is_made_private_before_it_is_written`; src/log.rs: `adr_0059_what_others_can_reach_is_made_private_before_a_write` |
| A transcript, a directory on the way or the state directory that is a symlink is never followed; one that isn't a directory or regular file, is another user's, or has another hard link, is refused. Each is opened from the one above it (`openat`, `O_NOFOLLOW`) and checked by `fstat` of what was opened, so nothing can be swapped in between. A refused host log fails the start; a refused session file is `session-log-failed` in the host log. | `open_private`, `keep_private`, `at` (src/log.rs); `sys::openat`, `sys::mkdirat` (src/sys.rs) | `security.rs`: `adr_0059_a_symlinked_transcript_is_never_followed`; src/log.rs: `adr_0059_a_symlink_is_never_followed`, `adr_0059_only_the_users_own_directories_and_files_are_written_to` |
| `doctor` warns of transcripts others can read, and `--fix` makes them private. | `transcripts` (src/ctl/doctor.rs) | `doctor.rs`: `readable_transcripts_are_made_private` |

### B3: `brnr acp` passes bytes unchanged but for the named changes

| Claim | Enforced by | Tested by |
|---|---|---|
| The editor's lines reach the agent byte for byte, whatever their spacing, key order and escapes, MCP secrets included, lines that aren't JSON too; the agent's reach the editor the same way. | `editor_message`, `agent_message` (src/host/acp.rs): a line is re-encoded only where the host changes it | `security.rs`: `adr_0002_acp_passes_bytes_unchanged` |
| The agent's stderr comes out of `brnr acp`'s, and `brnr acp` exits as the agent did. | src/proxy.rs | `security.rs`: `adr_0002_acp_passes_stderr_and_the_exit_status` |
| The named changes: the editor's `fs` and `terminal` capabilities are dropped, but in strict mode (ADR 2, ADR 41). | `drop_capabilities` (src/host/acp.rs) | `headless.rs`: `adr_0041_fs_and_terminal_pass_through_only_in_strict_mode` |
| The editor's load of a session another process owns is refused unless `shared_sessions` allows it (ADR 3, ADR 42). | `attach` (src/host/acp.rs) | `headless.rs`: `adr_0003_an_editors_load_of_a_held_session_is_refused`, `adr_0042_shared_sessions_let_an_editor_load_a_held_session` |
| Actions on an editor's session from outside are refused unless the profile enables each by name, and in strict mode always (ADR 4). | `check_experimental`, `check_editor_send` (src/host/experimental.rs); `check_strict` (src/host/strict.rs) | `headless.rs`: `adr_0004_experimental_actions_are_refused_without_opt_in`, `adr_0041_strict_mode_has_no_experimental_actions`, `adr_0004_approve_answers_in_the_editors_place`, `adr_0026_a_late_answer_to_a_cancelled_request_is_dropped` |

### B4: the agent's text, and its approvals

| Claim | Enforced by | Tested by |
|---|---|---|
| A session id becomes a file name only escaped: it can't leave the project folder or the locks' directory, and distinct ids never share a transcript or a lock (P8, ADR 53). | `file_name`, `session_log`, `session_lock` (src/paths.rs) | `security.rs`: `a_session_id_cant_climb_out_of_its_folder`; src/paths.rs: `a_sessions_two_files`, `adr_0053_distinct_ids_get_distinct_names`; `headless.rs`: `adr_0053_distinct_ids_never_share_a_transcript`, `adr_0053_distinct_ids_never_share_a_lock` |
| The agent's text is shown with control characters and bidi overrides escaped, and `show` warns of a command dressed up as another. | `render::clean` (src/render.rs) | `cli.rs`: `adr_0027_show_escapes_a_spoofed_command`; `security.rs`: `a_session_id_cant_climb_out_of_its_folder`; src/render.rs: `control_characters_are_escaped` |
| The agent's text never reaches a command line: `notify` passes it in the environment and on stdin, and the host's start request goes on a pipe. | src/ctl/notify.rs (`BRNR_TEXT`, …); `Request::send` (src/request.rs) | `cli.rs`: `adr_0036_notify_runs_a_command_per_event`; `headless.rs`: `adr_0008_prompt_is_not_on_the_command_line` |
| Headless, a permission request waits until someone answers it: brnr never answers for the user. | `answer_as_client` (src/host/acp.rs) | `security.rs`: `adr_0027_an_unanswered_request_waits` |
| `permission_timeout` only denies: the reject option, else `cancelled`, never an allow. | `fire_permission_timers`, `resolve_permission` (src/host/acp.rs) | `cli.rs`: `adr_0027_unanswered_permission_times_out_as_deny`; `security.rs`: `adr_0027_a_timeout_never_allows` |
| A request is answered only in its own session, with an option of the kind asked for (`deny` can't pick an allow option). | `answer` (src/host/control.rs), `resolve_permission` | `security.rs`: `adr_0027_a_request_is_answered_only_in_its_session`; `cli.rs`: `adr_0027_an_option_of_the_other_kind_is_refused` |

### B5: bridges and `notify` commands

| Claim | Enforced by | Tested by |
|---|---|---|
| A started bridge runs the profile's command as the user, with `BRNR_PID` and `BRNR_SOCKET`; it can do anything a socket client can (ADR 35). | `start_bridge` (src/host/control.rs) | `cli.rs`: `adr_0036_notify_works_as_a_bridge`, `adr_0035_a_bridge_that_closes_its_stdout_gets_events` |
| A bridge gets every event but `acp` unless it asks for it by name, so the raw ACP isn't forwarded by default. | `Peer::wants` (src/host/control.rs) | `security.rs`: `adr_0035_a_bridge_gets_no_raw_acp_unless_it_asks` |
| A reader that falls behind is cut off and told, and never holds the session up (ADR 6). | `send_to`, `drop_peer` (src/host/control.rs) | `headless.rs`: `adr_0006_slow_watcher_is_disconnected`; `cli.rs`: `adr_0036_notify_cut_off_as_a_bridge_stops_its_command` |

What a bridge sends beyond the machine, and to whom, is the bridge's: brnr
can't see past its stdin. That is the reach P13 says a bridge adds.

### B6: secrets

| Claim | Enforced by | Tested by |
|---|---|---|
| The values of MCP servers' `env` and `headers` reach the agent unchanged, and are `<redacted>` in the host log, the raw ACP, the `started` record and `acp` events, for a profile's `session/new`, `session/fork`, `session/resume` and `session/load`, and for an editor's own `session/new`, `load`, `resume` and `fork`. | `log::redacted`, `redact_mcp_servers` (src/log.rs); `Request::recorded` (src/request.rs); `host_request` (src/host/requests.rs); `editor_message` (src/host/acp.rs) | `headless.rs`: `adr_0025_a_profiles_mcp_secrets_are_redacted`, `adr_0025_an_editors_mcp_secrets_are_redacted`; `security.rs`: `adr_0025_a_resumed_sessions_secrets_are_redacted`, `adr_0002_acp_passes_bytes_unchanged`, `adr_0035_a_bridge_gets_no_raw_acp_unless_it_asks`; src/log.rs: `adr_0025_secrets_are_redacted_keys_and_structure_stay` |
| `doctor --report`, made to be pasted into an issue, redacts what ADR 25 redacts again in the host-log lines it shows (for a log an older brnr wrote), and shows the home directory as `~`; the rest of those lines is as recorded, prompts included, so it says to read it first (ADR 45). | `report_line` (src/ctl/doctor.rs) | `doctor.rs`: `adr_0045_the_report_is_what_to_paste` |
| `status`, `ps` and `list` carry no secret. | the process's metadata and status hold no request (`Host::start`, `status_report`) | `security.rs`: `adr_0025_a_resumed_sessions_secrets_are_redacted` |

### The adapters and the release

| Claim | Enforced by | Tested by |
|---|---|---|
| The adapters are built from pinned npm versions with a frozen lockfile, without optional dependencies (the agents themselves aren't bundled). | `adapters/package.json`, `adapters/bun.lock`, `bun install --frozen-lockfile --omit=optional` in `adapters/build.sh` | CI builds them and runs `adapters/check.py` |
| The workflows pin actions by commit and are checked by zizmor; crates by `cargo deny`. | `.github/workflows/`, `deny.toml` | CI |
| Release binaries rebuild identically within the same toolchain and OS, in separate directories at different times. | `.github/scripts/reproducible.sh`, a prerequisite of publication | `reproducible.yml`, on Linux and macOS |
| The release inventories brnr's locked Rust dependencies; the public Homebrew install is checked after the tap update. This does not establish that dependencies are safe. | `release.yml`: CycloneDX SBOM and `homebrew-smoke` job (ADR 52) | SBOM generation must succeed without changing `Cargo.lock`; a fresh macOS runner checks the installed version and runs doctor |

## Gaps

What brnr doesn't enforce, or doesn't test, today.

- **SIGKILL of the host bypasses child cleanup.** The host is the sole
  supervisor. Its death closes the agent and bridge pipes, but an agent or
  bridge that ignores EOF, is blocked, or leaves descendants can survive it.
  `doctor --fix` repairs stale metadata and locks; it does not kill those
  survivors. `chaos.rs` checks a flooding agent exits on its broken pipe,
  and that killing the proxy during a hung turn lets the surviving host
  stop its agent. Neither establishes unconditional cleanup after host
  SIGKILL; that would require a separate lifetime supervisor on both macOS
  and Linux.
- **Same-user processes.** Inside the boundary by design (above): any of
  them can approve, prompt, read transcripts and add bridges.
- **No peer credentials on the socket.** The process doesn't check who
  connected (`SO_PEERCRED`, `getpeereid`); the directory's and the socket's
  modes are the only control. Root is never kept out.
- **The other-user test runs only as root.** `another_user_cant_connect` is
  skipped otherwise, which includes CI, so the kernel's side of B1 is
  checked only where someone runs the tests as root. The modes it relies on
  are tested everywhere.
- **The process checks the runtime directory once, at start.** A directory
  opened to others while a process runs is refused by the commands
  (`no_approval_goes_through_a_dir_others_can_use`), but the running process
  goes on serving its socket, which its own mode still guards.
- **What others read before a repair.** A transcript others could reach is
  made private when a process opens it again (ADR 59), not before: what was
  in it until then may have been read, which the host log's `made-private`
  says. One no process opens again stays as it is until `doctor --fix`.
- **Another user's file in the state directory is tested only as `/`.**
  Making one takes root, so the ownership check is tested with `/` as the
  state directory; a file of another user's beneath it is refused by the
  same check, untested.
- **A session whose transcript is refused is served unrecorded.** Logging
  never stops forwarding (P1, P6), so the session goes on; only the host
  log's `session-log-failed` and `doctor`'s transcripts check say so.
- **Metadata files follow the umask.** `<pid>.json` (pids, cwd, the agent's
  command line, the socket's path) is created with the default mode, so a
  permissive umask leaves it readable by mode; the 0700 directory is what
  keeps it private. Not tested.
- **An editor's session that can't be locked runs unlocked.** The proxy
  passes it through; `status` reports `lock_error`, and the host log records
  `lock-failed`. A headless resume is refused while the editor's process
  reports serving it, but a second process could acquire its lock if that
  process doesn't answer and the lock failure was transient (ADR 50).
- **Redaction is by field, not by value.** brnr redacts the MCP servers'
  `env` and `headers` in the requests that open a session (ADR 25). A secret
  elsewhere is recorded as it is: in an MCP server's `args` or `url`, in the
  agent's command line, or repeated by the agent itself in its messages, tool
  calls, errors or stderr (ADR 10 puts the agent's stderr in the host log),
  and it then reaches bridges with those events.
- **Approvals are the agent's own asks.** brnr answers the permission
  requests the agent makes; it doesn't sandbox the agent. A malicious
  adapter or npm package, or an agent in a permissive mode, acts without
  asking, and nothing in brnr can stop it.
- **`permission_timeout` is headless only.** An editor's session waits for
  the editor; with `approve` enabled, any of the user's processes may answer
  it (ADR 4).
- **A refused runtime directory is a denial of service.** With neither
  `BRNR_DIR` nor `XDG_RUNTIME_DIR` set, the directory is `$TMPDIR/brnr-<uid>`;
  where `$TMPDIR` is shared (`/tmp` on Linux), another user who creates that
  path first makes brnr refuse to start until it is removed. They gain no
  access.
- **Agents' lookup next to brnr.** A bare agent or bridge name is looked up
  next to the `brnr` executable first (ADR 38); that directory is trusted as
  `PATH` is.
- **The adapters' dependencies.** Pinned and locked, but brnr doesn't review
  them: a compromised version of an adapter's package, pinned in a release,
  runs as the agent.

## Out of scope

As [SECURITY.md](../SECURITY.md) has it: the agents and the upstream adapters
themselves, what an agent does with an approval its user gave, and anyone who
can already run code as the user.
