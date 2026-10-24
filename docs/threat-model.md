# Threat model

brnr runs a coding agent in a process of its own and lets other commands
reach it over a Unix socket, including answering the agent's approvals from
outside the editor. That makes it a security boundary: whoever can reach the
process can act as the agent's user. This document says who can reach what,
how brnr keeps it so, and which test checks each claim. Where a claim has no
enforcement or no test, it is listed as a gap. [SECURITY.md](https://github.com/brnrhq/brnr/blob/main/SECURITY.md)
says what is in scope for a report; the principles cited are
[ADR 1](adr/0001-principles.md)'s.

## Assets

- **The agent's actions on the user's machine.** The agent runs as the user:
  it edits files and runs commands. Whoever can prompt it, or answer its
  approvals, directs those.
- **Approvals.** A permission request is the agent asking before it acts. An
  answer given by the wrong party, or given when nobody answered, is an
  action the user didn't take.
- **Elicitations.** The agent asking the user for input (ADR 4.5): a form's
  values, or a URL to open, such as an MCP server's login. An accept given by
  the wrong party, or for nobody, is input the user didn't give; a URL the
  agent sends is its text, and opening it is the user's to do.
- **Prompts and code in transcripts.** `~/.brnr` keeps every session's events
  and raw ACP (ADR 5.1), until the user deletes them, or, with `retention`
  set, until they are older than it (ADR 9.5): prompts, the agent's replies, tool calls with their
  input and output, diffs of the user's code, and, in an editor's session,
  what the editor answered the agent's file and terminal requests with,
  where the profile or strict mode passes them: files as its buffers have
  them, unsaved changes included, and commands' output (ADR 2.7).
- **Secrets.** The values of a profile's or an editor's MCP servers' `env`
  and `headers` (tokens, `Authorization`), which go to the agent in the
  requests that open a session (ADR 9.1).
- **The config file.** `config.toml` names the commands brnr runs as the
  user (the agent, bridges, MCP servers) and holds the profiles' MCP
  secrets (ADR 6.3). Whoever can write it runs code as the user; whoever
  can read it has the secrets.

## Actors

| Actor | Trusted with | Where it stands |
|---|---|---|
| Other local users | nothing | outside: kept out of the runtime directory, sockets, transcripts and session locks by file modes, and a config file they can read or write is refused (P13) |
| Root | everything | outside brnr's reach; can do anything as any user |
| Processes running as the same user | everything the user can do | **inside** the boundary (P13): see below |
| The editor | its session, and serving the agent's file and terminal requests (`fs/*`, `terminal/*`) | inside; it runs `brnr acp`, and its bytes pass unchanged (P1); what it reads, writes and runs for the agent is its own to allow (ADR 2.7) |
| The agent (the adapter and what it runs) | the user's machine, as far as its own permission prompts go | inside as a process; its *text* is untrusted (P8) |
| Bridges and `event notify` commands | everything a socket client can do | inside: run as the user from the user's config; what they forward leaves the machine (P13) |
| A malicious adapter or npm package | as the agent | inside once run: brnr can't contain it |

Processes running as the same user are inside the boundary, and that is a
deliberate choice, not a gap: P13 is "only you", and any process of the
user's is the user to the kernel. It can open the control socket, approve
whatever is waiting, read the transcripts, edit the config to add a bridge,
or `ptrace` the agent. brnr has nothing to tell such a process from the user
at a terminal, and doesn't pretend to; SECURITY.md lists "anyone who can
already run code as the user" as out of scope. What brnr does keep apart
within the user is protocol, not access: actions on an editor's session are
experimental and refused unless enabled by name (ADR 7.1), and strict mode has
none of them (ADR 2.1). Those protect the editor's view from surprises, not
the user from their own processes.

## Trust boundaries

```text
 other local users, root (outside: file modes, P13)
- - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - -
 the user                              (B1)
                          $BRNR_DIR, 0700 ── <pid>.sock, 0600
                                    │      └─ brnr prompt send / permission / event
                                    │         watch, any process of the user's
 editor ──stdio── brnr acp ──socketpairs── brnr process ──pipes── agent
          (B3: bytes unchanged,             │   │              (B4: its text is
           but the named changes)           │   │               untrusted, P8)
                                            │   └── bridges, notify ──► Slack, push, …
                                            │       (B5: beyond the machine is theirs)
                          ~/.brnr, 0700 ── transcripts and host logs, 0600;
                                           session locks, sessions/ 0700
                                   (B2; secrets redacted, B6)
                ~/.config/brnr ── config.toml: read only if only the user
                                  can change or read it (B7)
```

- **B1**, the runtime directory and the control sockets: between other local
  users and the process.
- **B2**, the transcripts and host logs: between other local users and what
  the sessions said.
- **B3**, `brnr acp`: between the editor and the agent, where brnr must be
  invisible (P1).
- **B4**, the agent: its text (session ids, titles, commands, messages) and
  its permission requests reach brnr's files, terminal and approvals.
- **B5**, bridges and `event notify` commands: the edge of the machine.
- **B6**, secrets: what brnr records and sends of the MCP servers' values.
- **B7**, the config file: between other local users and what brnr runs
  and the secrets it holds.

## Claims

Each claim with what enforces it (file and function) and the test that checks
it. Tests are in `tests/` unless a module is named; `security.rs` holds those
written for this document. A test that can't run where it is (as root,
without `lsof`) fails in CI or with `BRNR_TESTS_STRICT` set (`skip`,
tests/common/mod.rs), so in CI it never passes unrun. Elsewhere it passes
without checking its claim, and says `skipped:` on stderr.

### B1: the runtime directory and sockets refuse others

| Claim | Enforced by | Tested by |
|---|---|---|
| The runtime directory is created 0700 and the process refuses one that isn't a directory owned by the user with no group or other bits, is a symlink, or, on macOS, has an access control list, which can grant what the mode doesn't show (the error gives the `chmod -N` that removes it; brnr doesn't); it then writes nothing there and starts no agent. It is opened without following a symlink and checked through what was opened (`O_NOFOLLOW`, `fstat`). The directory is the boundary: the sockets and metadata in it aren't checked one by one (ADR 3.9). | `paths::ensure_private`, `paths::check_private` (src/paths.rs), from `Host::start` (src/host/mod.rs); `sys::has_acl` (src/sys.rs) | `security.rs`: `a_runtime_dir_others_can_use_is_refused`, `a_runtime_dir_of_another_users_is_refused`, `adr_0003_a_runtime_dir_with_an_acl_is_refused` (macOS); `headless.rs`: `runtime_dir_symlink_is_refused`; src/paths.rs: `adr_0003_a_private_directory_is_checked_through_what_was_opened` |
| Every command that reads the runtime directory refuses one others can use, a symlink, or, on macOS, one with an access control list, before it trusts anything there: no request, approval included, is sent through it. | `discover` (src/ctl/client.rs), through `paths::check_private` | `security.rs`: `a_runtime_dir_others_can_use_is_refused`, `a_symlinked_runtime_dir_is_refused_by_every_command`, `no_approval_goes_through_a_dir_others_can_use`, `adr_0003_a_runtime_dir_with_an_acl_is_refused` (macOS); `headless.rs`: `shared_runtime_dir_is_refused` |
| The control socket is 0600, the directory 0700, the session locks' directory (`sessions/` in the state directory, ADR 3.9) 0700 and each lock 0600, whatever the umask. | `Host::start` (bind, then `set_permissions` 0600); `lock::take` (src/lock.rs) | `security.rs`: `what_brnr_makes_is_private_whatever_the_umask` |
| Another user can't connect to the socket: the directory and the socket's mode both refuse them. | the kernel, given the modes above | `security.rs`: `another_user_cant_connect` (needs root, so `cargo test` ignores it; CI runs it as root on Linux, connecting as `nobody`) |
| A metadata file counts only with the socket next to it, so a planted file can't point a command at another socket. | `read_meta` (src/ctl/client.rs) | `headless.rs`: `metadata_names_its_own_socket` |
| Session locks, in the state directory beside the transcripts they guard (ADR 3.9), are held to the runtime directory's terms (private, with no access control list on macOS, or refused, with the `chmod -N` that removes one) and never followed through a symlink. A headless start, resume or fork that can't take its lock is refused, as is a session opened in a running process (`session new --pid`, `resume --pid`), which the agent then closes again (ADR 3.9, ADR 8.1). | `lock::take` (`ensure_private`, `O_NOFOLLOW`); `sys::has_acl` (src/sys.rs); `own`, `take_lock` (src/host/acp.rs); `host_request_done`, `peer_opened`, `peer_result` (src/host/requests.rs) | `security.rs`: `adr_0003_session_locks_are_private_and_never_followed`, `adr_0003_session_locks_with_an_acl_are_refused` (macOS); `cli.rs`: `adr_0003_a_new_session_that_cant_be_locked_isnt_started`, `adr_0003_a_resume_that_cant_be_locked_isnt_started`, `adr_0003_a_fork_that_cant_be_owned_is_refused`, `adr_0008_pid_session_that_cant_be_locked_is_closed` |
| brnr listens on no network. | the process binds only a `UnixListener` (src/host/mod.rs); P15 | `security.rs`: `the_process_listens_on_no_network` (with `lsof`, which CI has) |
| `doctor` fails a runtime directory others can use, or, on macOS, with an access control list, and `--fix` tightens its mode but never through a symlink, and never removes an ACL. | `runtime_dir`, `Found`, `private_problem`, `fixable` (src/ctl/doctor.rs); `sys::has_acl` (src/sys.rs) | `doctor.rs`: `open_runtime_dir_fails_until_fixed`, `symlinked_runtime_dir_is_not_fixed`; `security.rs`: `adr_0003_a_runtime_dir_with_an_acl_is_refused` (macOS) |

### B2: transcripts are private

| Claim | Enforced by | Tested by |
|---|---|---|
| What brnr creates under `~/.brnr` is 0700 (directories) and 0600 (files), whatever the umask. | `open_private` (src/log.rs) | `headless.rs`: `transcripts_are_private`; `security.rs`: `what_brnr_makes_is_private_whatever_the_umask`; src/log.rs: `adr_0009_new_and_private_paths_are_opened_as_they_are` |
| Before a record goes into a session's events, its raw ACP or a host log, the state directory, each directory below it and the file are the user's and have no group or other bits (and on macOS no access control list, which is refused, below), however they came to exist: one others can reach is made private through the descriptor that is then written to (`fchmod`), beneath private parents or traversable ones, and the host log records `path_made_private` with the mode it had (`brnr session delete`, which records `session_deleted` with no host log, says so on stderr, ADR 8.1). There is no override (ADR 9.3). | `open_private`, `keep_private` (src/log.rs), the one open of all three (`open_append`); `Writer::made_private`; `record_in_transcripts` (src/log.rs), `delete` (src/ctl/delete.rs) | `security.rs`: `adr_0009_a_transcript_others_can_read_is_made_private_before_it_is_written`; `cli.rs`: `adr_0008_delete_records_in_each_transcript_it_can`; src/log.rs: `adr_0009_what_others_can_reach_is_made_private_before_a_write` |
| A transcript, a directory on the way or the state directory that is a symlink is never followed; one that isn't a directory or regular file, is another user's, has another hard link, or, on macOS, has an access control list, which can grant what the mode doesn't show, is refused (an ACL with the `chmod -N` that removes it; brnr doesn't). Each is opened from the one above it (`openat`, `O_NOFOLLOW`) and checked by `fstat` of what was opened, so nothing can be swapped in between. A refused host log fails the start; a refused session file is `session_log_failed` in the host log, or, for `brnr session delete`, said on stderr, and the command fails. What reads transcripts (`session list`, `event log`, `session resume`, `session delete`, catching up, `doctor`) holds every part of the path to the same rules, opened the same way from the state directory it is in, and checked by `fstat`: nothing is looked in or read through a symlink (the state directory, `projects`, `hosts`, a project folder, the file), nor is what isn't the user's own directory or regular file, or has another hard link, which could be any file of the user's taken for a session, or an access control list (ADR 8.1, ADR 9.3). | `open_private`, `keep_private`, `at` (src/log.rs); `sys::openat`, `sys::mkdirat`, `sys::has_acl` (src/sys.rs); `open`, `walk`, `check`, `entries` (src/transcript.rs), the one place transcripts are read | `security.rs`: `adr_0009_a_symlinked_transcript_is_never_followed`, `adr_0009_a_symlinked_transcript_is_never_read`, `adr_0009_a_symlinked_hosts_or_projects_folder_is_never_read`, `adr_0009_a_transcript_with_an_acl_is_never_written` (macOS), `adr_0009_a_transcript_with_an_acl_is_never_read` (macOS); src/log.rs: `adr_0009_a_symlink_is_never_followed`, `adr_0009_only_the_users_own_directories_and_files_are_written_to`; src/transcript.rs: `adr_0009_a_transcript_is_read_only_where_it_could_be_written`, `adr_0009_every_part_of_a_transcripts_path_is_the_users_own`; `cli.rs`: `adr_0008_delete_records_in_each_transcript_it_can` |
| `session delete --purge` deletes only the session's own files: in each project folder under the state directory, its events file and raw ACP, named for its id escaped (ADR 9.2), so an id can't name another session's file or climb out. The state directory, `projects` and each folder are opened without following a symlink, each from the one before, and the names removed from what was opened (`unlinkat`): a symlinked folder isn't looked in, and a symlink named like the transcript is removed, not what it points at. The host logs stay (ADR 8.1). | `purge` (src/log.rs); `sys::openat`, `sys::unlinkat` (src/sys.rs); `paths::file_name` (src/paths.rs) | `security.rs`: `adr_0008_purge_deletes_only_that_sessions_files`; `cli.rs`: `adr_0008_delete_purge` |
| `session prune --older-than <s>`, and each process as it starts with the config's `retention`, delete only what was last active more than `<s>` seconds ago, as `--purge` deletes it: a session's own files, named for its id escaped, removed from the folders opened without following a symlink (`openat`, `O_NOFOLLOW`, `unlinkat`), its events file last; and the host log of a process that has gone, from `hosts` opened the same way. Never a session open in a process or whose lock is held: its lock is taken first, and its age read again under it. Never the host log of a running process (listed, or its pid alive without `process_exited`), nor a file brnr can't read the age of. Nothing has an age limit unless the user sets one (ADR 9.5). | `prune`, `run`, `still_old` (src/ctl/prune.rs); `purge`, `purge_host_log` (src/log.rs); `lock::holding` (src/lock.rs); `check_retention` (src/profiles.rs) | `cli.rs`: `adr_0009_prune_deletes_what_was_last_active_before_the_age`, `adr_0009_prune_never_touches_an_open_or_locked_session`, `adr_0009_a_process_prunes_as_it_starts_when_retention_is_set`, `adr_0009_retention_is_a_number_of_seconds_not_0`, `adr_0009_prune_keeps_a_session_it_cant_read_the_age_of`, `adr_0009_a_process_records_its_prune_before_it_exits` |
| `doctor` warns of transcripts others can read, of one with an access control list (macOS), with the `chmod -N` that removes it, which `--fix` doesn't do, of one it can't open, which `--fix` leaves too, and of a state directory, `hosts` or `projects` that isn't a directory, and `--fix` makes them private, never through a symlink: each path is opened from the one above it (`openat`, `O_NOFOLLOW`), checked by `fstat`, a directory listed through what was opened, and a mode changed through it (`fchmod`), so it doesn't look below a symlinked state directory, `hosts` or `projects`, nor change what is put at a name after it looked (ADR 9.3). | `transcripts`, `Found` (src/ctl/doctor.rs); `sys::openat`, `sys::names`, `sys::has_acl` (src/sys.rs) | `doctor.rs`: `readable_transcripts_are_made_private`, `a_state_part_that_is_not_a_directory_is_a_problem`, `adr_0009_doctor_says_what_it_cant_open_and_what_fix_left`; src/ctl/doctor.rs: `adr_0009_fix_changes_what_was_opened_not_its_name`; `security.rs`: `adr_0009_a_symlinked_hosts_or_projects_folder_is_never_read`, `adr_0009_a_transcript_with_an_acl_is_never_read` (macOS) |

### B3: `brnr acp` passes bytes unchanged but for the named changes

| Claim | Enforced by | Tested by |
|---|---|---|
| The editor's lines reach the agent byte for byte, whatever their spacing, key order and escapes, MCP secrets included, lines that aren't JSON too, but for a request's id (below); the agent's reach the editor the same way. | `editor_message`, `agent_message` (src/host/acp.rs): a line is re-encoded only where the host changes it; `json::members`, `json::replace` (src/json.rs) change an id in place | `security.rs`: `adr_0003_acp_passes_bytes_unchanged` |
| Every request to the agent has an id of the host's, the editor's too, so no editor id can be taken for another requester's, and an answer goes only to whoever sent the request: the editor's with its id as it wrote it. The editor's `$/cancel_request` reaches only its own requests (ADR 2.4). | `Outbound` (src/host/outbound.rs); `editor_message`, `agent_response`, `editor_cancel` (src/host/acp.rs); `host_request` (src/host/requests.rs) | `headless.rs`: `adr_0002_an_editors_prompt_ends_only_with_its_own_answer`, `adr_0002_an_editors_request_with_a_host_requests_id_is_the_editors`, `adr_0002_ids_come_back_as_the_editor_wrote_them`, `adr_0002_the_editors_cancel_request_is_for_its_own_request`; src/host/outbound.rs: `adr_0002_ids_are_never_given_twice`, `adr_0002_an_answer_is_its_requests_once`; src/json.rs: `adr_0002_an_id_is_found_where_it_is_and_nowhere_else` |
| The agent's stderr comes out of `brnr acp`'s, and `brnr acp` exits as the agent did. | src/proxy.rs | `security.rs`: `adr_0003_acp_passes_stderr_and_the_exit_status` |
| The agent's stdout ends on the editor's when it ends, while the agent runs on, and brnr writes nothing there after it; its stderr comes out as it is written, newline or not, every byte once (ADR 2.6). | `agent_stdout_ended`, `read_agent_stderr` (src/host/mod.rs); `write_editor` (src/host/acp.rs); `run` (src/proxy.rs); `end_stdout` (src/sys.rs) | `security.rs`: `adr_0002_the_editors_stdout_ends_with_the_agents`, `adr_0002_stderr_comes_as_it_is_written`, `adr_0002_stdout_and_stderr_end_on_their_own`; src/host/fuzz.rs: `adr_0002_brnrs_own_lines_end_with_the_agents_stdout` |
| The editor's stdin ends when the agent's does, a stop's, the agent's own or its exit's, so the editor's next write fails, as on a pipe; what the editor wrote that reached the host before then isn't sent, recorded in the raw ACP or answered, and the host log counts it, `editor_bytes_dropped` (ADR 2.6). An agent that closes its stdin itself is found out by the write that fails to reach it: that line is recorded as sent, and its request waits; the writes after it are the ones dropped. | `close_agent_stdin`, `editor_bytes_dropped` (src/host/mod.rs); `editor_bytes` (src/host/acp.rs); `relay_stdin` (src/proxy.rs); `end_stdin` (src/sys.rs) | `headless.rs`: `adr_0002_the_editors_write_fails_once_the_agents_stdin_closes`; src/host/fuzz.rs: `adr_0002_the_editors_stdin_ends_with_the_agents` |
| The editor's `fs` and `terminal` capabilities don't reach the agent by default, a named change, and the host log says so; those the profile's `client_capabilities` names do, and in strict mode both, as the editor sent them. Where they do, the agent's `fs/*` and `terminal/*` requests reach the editor as the agent wrote them, and the editor's answers, results and errors, reach the agent: the editor serves them, as it would directly, and brnr neither approves nor answers them in an editor's session (ADR 2.7). | `drop_capabilities`, `editor_request`, `agent_request`, `editor_message` (src/host/acp.rs) | `headless.rs`: `adr_0002_fs_and_terminal_reach_the_agent_in_strict_mode_or_when_named`, `adr_0002_the_agents_file_and_terminal_requests_reach_the_editor` |
| The editor's load, resume or prompt of a session another process owns, not open in the editor's process, is refused unless `shared_sessions` allows it, and never reaches the agent (ADR 3.9, ADR 3.10). | `attach`, `editor_message` (src/host/acp.rs) | `headless.rs`: `adr_0003_an_editors_load_of_a_held_session_is_refused`, `adr_0003_an_editors_prompt_of_a_held_session_is_refused`, `adr_0003_shared_sessions_let_an_editor_load_a_held_session` |
| A load or resume the agent answers with success after its session closed in the editor's process, which another process took meanwhile, isn't served there unless `shared_sessions` allows it: it doesn't open, its lock isn't held, the agent is asked to close it, and the editor is told which process holds it (ADR 3.9, ADR 3.10). | `open_answered`, `not_served` (src/host/acp.rs); `peer_done`, `peer_result` (src/host/requests.rs) | `headless.rs`: `adr_0003_a_load_answered_after_another_process_took_its_session_isnt_served`, `adr_0003_a_resume_answered_after_another_process_took_its_session_isnt_served`, `adr_0003_shared_sessions_serve_a_load_answered_after_another_process_took_it` |
| Actions on an editor's session from outside are refused unless the profile enables each by name, and in strict mode always (ADR 7.1). | `check_experimental`, `check_editor_send` (src/host/experimental.rs); `check_strict` (src/host/strict.rs) | `headless.rs`: `adr_0007_experimental_actions_are_refused_without_opt_in`, `adr_0002_strict_mode_has_no_experimental_actions`, `adr_0007_approve_answers_in_the_editors_place`, `adr_0002_a_late_answer_to_a_cancelled_request_is_dropped`, `adr_0004_an_editors_elicitation_is_seen_and_reaches_the_editor` |

### B4: the agent's text, and its approvals

| Claim | Enforced by | Tested by |
|---|---|---|
| A session id becomes a file name only escaped: it can't leave the project folder or the locks' directory, and distinct ids never share a transcript or a lock (P8, ADR 9.2). | `file_name`, `session_log`, `session_lock` (src/paths.rs) | `security.rs`: `a_session_id_cant_climb_out_of_its_folder`; src/paths.rs: `a_sessions_two_files`, `adr_0009_distinct_ids_get_distinct_names`; `headless.rs`: `adr_0009_distinct_ids_never_share_a_transcript`, `adr_0009_distinct_ids_never_share_a_lock` |
| The agent's text is shown with control characters and bidi overrides escaped, and `permission show` warns of a command dressed up as another. | `render::clean` (src/render.rs) | `cli.rs`: `adr_0004_show_escapes_a_spoofed_command`; `security.rs`: `a_session_id_cant_climb_out_of_its_folder`; src/render.rs: `control_characters_are_escaped` |
| The agent's text never reaches a command line: `event notify` passes it in the environment and on stdin, and the host's start request goes on a pipe. | src/ctl/notify.rs (`BRNR_TEXT`, …); `Request::send` (src/request.rs) | `cli.rs`: `adr_0005_notify_runs_a_command_per_event`; `headless.rs`: `adr_0003_prompt_is_not_on_the_command_line` |
| Headless, a permission request waits until someone answers it: brnr never answers for the user. | `agent_request` (src/host/acp.rs); `Approvals` (src/host/approvals.rs) | `security.rs`: `adr_0004_an_unanswered_request_waits` |
| Headless, a request the agent cancels (`$/cancel_request`) is answered only `cancelled` or `cancel`, as ACP has it, never an allow or an accept; in an editor's session brnr answers nothing, and the editor's answer goes on unchanged (P1). A turn's end ends none, and an answer from outside after the cancel sends nothing (ADR 4.5). | `agent_cancel`, `editor_message` (src/host/acp.rs); `Approvals::give_up` (src/host/approvals.rs) | `cli.rs`: `adr_0004_a_request_the_agent_cancels_ends_by_the_agent`, `adr_0004_a_turn_that_ends_leaves_its_request_waiting`; `headless.rs`: `adr_0004_an_editor_gets_the_agents_cancel_and_answers_it`; src/host/approvals.rs: `adr_0004_the_agents_cancel_ends_only_what_waits` |
| A headless process offers the agent no `fs` or `terminal` (one started headless, forked in it, or taken over from an editor), and answers a request for them "method not found": brnr reads, writes and runs nothing for the agent (ADR 2.7). | `begin_headless_start` (src/host/requests.rs); `agent_request` (src/host/acp.rs) | `headless.rs`: `adr_0002_headless_sessions_offer_neither` |
| `response_timeout` (or `--response-timeout`) only rejects once: the `reject_once` option, else the turn is cancelled (`session/cancel`, the request answered `cancelled`); never an allow, nor `reject_always` (ADR 8.1). | `fire_permission_timers` (src/host/acp.rs); `Approvals::time_out`, `choose` (src/host/approvals.rs) | `security.rs`: `adr_0008_timeout_rejects_once_else_cancels_the_turn`, `adr_0004_a_timeout_never_allows`; src/host/approvals.rs: `adr_0004_a_timeout_rejects_once_or_cancels_the_turn` |
| Headless, an elicitation waits until someone answers it, and `response_timeout` answers it only `cancel`: never `accept`, nor `decline`, the user's no (ADR 4.5). | `agent_request`, `fire_permission_timers` (src/host/acp.rs); `Approvals::time_out` (src/host/approvals.rs) | `security.rs`: `adr_0004_a_timeout_cancels_never_accepts`; `cli.rs`: `adr_0004_requests_and_show_list_an_elicitation` |
| An accept sends only what fits the form: fields it names, the required ones, values of their type, within their bounds and choices, checked in the host for the CLI and bridges alike; one that doesn't fit sends nothing. An elicitation is answered only in its own session, or outside any by its process (ADR 4.5). | `response`, `check` (src/host/elicitation.rs); `elicitation` (src/host/control.rs); `Approvals::answer` (src/host/approvals.rs) | `cli.rs`: `adr_0004_accept_checks_fields_against_the_schema`, `adr_0004_a_request_scoped_elicitation_is_answered_by_pid`; src/host/elicitation.rs: `what_doesnt_fit_names_the_field` |
| An elicitation's URL is shown escaped, never opened or run: brnr has no code that opens one (P8, ADR 4.5). | `render::clean`; `show` (src/ctl/elicitation.rs) | `cli.rs`: `adr_0004_requests_and_show_list_an_elicitation`; src/render.rs: `adr_0004_elicitation_events_have_lines` |
| A request is answered only in its own session, with the option of the kind asked for, and no other kind in its place: `reject` can't pick an allow option, nor `allow` a reject one, nor `--always` a once one; a missing or doubled kind answers nothing; a socket `allow` or `reject` with `option`, `option_id`'s old name (a field it doesn't take, ADR 8.8), or with an `option_id` that isn't a string (null, a number), is refused, not answered by kind in its place (ADR 8.1). | `check_request`, `answer` (src/host/control.rs); `Approvals::answer`, `choose` (src/host/approvals.rs) | `security.rs`: `adr_0004_a_request_is_answered_only_in_its_session`; `cli.rs`: `adr_0008_allow_and_reject_pick_by_kind`, `adr_0008_a_missing_or_doubled_kind_fails`, `adr_0008_option_must_be_on_the_verbs_side`, `adr_0008_a_permissions_option_is_option_id`, `adr_0008_a_permissions_option_id_is_a_string` |

### B5: bridges and `event notify` commands

| Claim | Enforced by | Tested by |
|---|---|---|
| A started bridge runs the profile's command as the user, with `BRNR_PID` and `BRNR_SOCKET`; it can do anything a socket client can (ADR 5.7). | `start_bridge` (src/host/control.rs) | `cli.rs`: `adr_0005_notify_works_as_a_bridge`, `adr_0005_a_bridge_that_closes_its_stdout_gets_events` |
| A bridge gets every event but `acp` unless it asks for it by name, so the raw ACP isn't forwarded by default. | `Peer::wants` (src/host/control.rs) | `security.rs`: `adr_0005_a_bridge_gets_no_raw_acp_unless_it_asks` |
| A reader that falls behind is cut off and told, and never holds the session up (ADR 10.1). | `send_to`, `drop_peer` (src/host/control.rs) | `headless.rs`: `adr_0010_slow_watcher_is_disconnected`; `cli.rs`: `adr_0005_notify_cut_off_as_a_bridge_stops_its_command` |
| Asking whether a peer was cut off (`was_cut_off`) gives a position, never an event, for that peer or another, and takes nothing away from it: the record stays for its owner. What a command cut off catches up on, it reads from the transcript itself, as the user (B2), and only the events it subscribed to, as live: a peer that didn't ask for `acp` gets no raw ACP. A peer learns nothing it couldn't by subscribing (ADR 10.2). | `was_cut_off` (src/host/control.rs); `Follow::read`, `Found::wanted` (src/ctl/catch_up.rs) | `security.rs`: `adr_0010_was_cut_off_says_where_never_what` |

What a bridge sends beyond the machine, and to whom, is the bridge's: brnr
can't see past its stdin. That is the reach P13 says a bridge adds.

### B6: secrets

| Claim | Enforced by | Tested by |
|---|---|---|
| The values of MCP servers' `env` and `headers` reach the agent unchanged, and are `<redacted>` in the host log, the raw ACP, the `started` record and `acp` events, for a profile's `session/new`, `session/fork`, `session/resume` and `session/load`, and for an editor's own `session/new`, `load`, `resume` and `fork`. | `log::redacted`, `redact_mcp_servers` (src/log.rs); `Request::recorded` (src/request.rs); `host_request` (src/host/requests.rs); `editor_message` (src/host/acp.rs) | `headless.rs`: `adr_0009_a_profiles_mcp_secrets_are_redacted`, `adr_0009_an_editors_mcp_secrets_are_redacted`; `security.rs`: `adr_0009_a_resumed_sessions_secrets_are_redacted`, `adr_0003_acp_passes_bytes_unchanged`, `adr_0005_a_bridge_gets_no_raw_acp_unless_it_asks`; src/log.rs: `adr_0009_secrets_are_redacted_keys_and_structure_stay` |
| `doctor --report`, made to be pasted into an issue, redacts what ADR 9.1 redacts again in the host-log lines it shows, a line nested past 128 too, read as transcripts are (ADR 2.5: to 256; a deeper one isn't shown), shows no record in a format this brnr doesn't read (ADR 5.10), and shows the home directory as `~`; the rest of those lines is as recorded, prompts included, so it says to read it first (ADR 11.5). | `report_line` (src/ctl/doctor.rs), `record` (src/transcript.rs), `redact_record` (src/log.rs) | `doctor.rs`: `adr_0011_the_report_is_what_to_paste`, `adr_0011_the_report_redacts_a_deep_line` |
| `session status`, `process list` and `session list` carry no secret. | the process's metadata and status hold no request (`Host::start`, `status_report`) | `security.rs`: `adr_0009_a_resumed_sessions_secrets_are_redacted` |

### B7: the config file is only the user's

| Claim | Enforced by | Tested by |
|---|---|---|
| Before any command reads the config file, it and its folder are the user's own and nobody else can write them, and nobody else can read the file, and on macOS neither has an access control list, which can grant what the mode doesn't show; a symlink is followed, and the file it resolves to and that file's folder are checked, through the descriptors the file is read from (`openat`, `O_NOFOLLOW`, `fstat`). Otherwise the command fails before it starts or reaches anything, naming the path, the owner or mode, and the `chmod` or `chown` that fixes it. brnr changes neither (P4). No file is no config (ADR 9.4). | `read_private`, `check_private` (src/profiles.rs), from `load_all`, the one place the config is read; `sys::openat`, `sys::has_acl` (src/sys.rs) | `security.rs`: `adr_0009_a_config_others_can_read_or_write_is_refused`, `adr_0009_a_config_of_another_users_is_refused`, `adr_0009_a_symlinked_config_is_checked_where_it_points`, `adr_0009_a_config_with_an_acl_is_refused` (macOS), `adr_0009_a_missing_config_is_no_config` |
| `doctor` fails a config file others can reach, with the same problems and fixes, and `--fix` doesn't change it. | `config_file` (src/ctl/doctor.rs), through `load_all` | `doctor.rs`: `adr_0009_doctor_fails_a_config_others_can_reach` |

### The adapters and the release

| Claim | Enforced by | Tested by |
|---|---|---|
| The adapters are built from pinned npm versions with a frozen lockfile, without optional dependencies (the agents themselves aren't bundled). | `adapters/package.json`, `adapters/bun.lock`, `bun install --frozen-lockfile --omit=optional` in `adapters/build.sh` | CI builds them and runs `adapters/check.py` |
| The workflows pin actions by commit and are checked by zizmor; crates by `cargo deny`. | `.github/workflows/`, `deny.toml` | CI |
| Release binaries rebuild identically within the same toolchain and OS, in separate directories at different times. | `.github/scripts/reproducible.sh`, a prerequisite of publication | `reproducible.yml`, on Linux and macOS |
| The release inventories brnr's locked Rust dependencies; the public Homebrew install is checked after the tap update, which a release candidate doesn't make (ADR 11.4). This does not establish that dependencies are safe. | `release.yml`: CycloneDX SBOM and `homebrew-smoke` job (ADR 11.7) | SBOM generation must succeed without changing `Cargo.lock`; a fresh macOS runner checks the installed version and runs doctor |

## Gaps

What brnr doesn't enforce, or doesn't test, today.

- **SIGKILL of the host bypasses child cleanup.** The host is the sole
  supervisor. Its death closes the agent and bridge pipes, but an agent or
  bridge that ignores EOF, is blocked, or leaves descendants can survive it.
  `doctor --fix` repairs stale metadata and locks; it does not kill those
  survivors. `chaos.rs` checks a flooding agent exits on its broken pipe,
  and that killing the proxy during a hung setup or turn lets the surviving
  host stop its agent. Neither establishes unconditional cleanup after host
  SIGKILL; that would require a separate lifetime supervisor on both macOS
  and Linux (#391). Its test, `killed_host_leaves_no_agent_or_child_behind`,
  fails today and is ignored until then.
- **Same-user processes.** Inside the boundary by design (above): any of
  them can approve, prompt, read transcripts and add bridges.
- **No peer credentials on the socket.** The process doesn't check who
  connected (`SO_PEERCRED`, `getpeereid`); the directory's and the socket's
  modes are the only control. Root is never kept out.
- **A same-user process can fill the socket's connections.** A process
  serves 128 connections at once, and refuses the next with an error
  (ADR 10.4); a process of the user's that holds 128 open keeps brnr's
  commands out of it until it lets go. It is inside the boundary (P13), so
  this is reliability, not access; the limit keeps the process's threads and
  memory bounded (N2). A request is read up to 32 MiB, and a longer one cuts
  its peer off (ADR 10.3). `adr_0010_a_connection_past_the_cap_is_told_and_closed`
  and `adr_0010_a_request_past_the_limit_is_answered_and_cut_off` check them.
- **The other-user test runs as root only on Linux in CI.**
  `another_user_cant_connect` needs root, to connect as `nobody`, so
  `cargo test` ignores it; CI runs it as root (`--ignored`) on Linux, so
  the kernel's side of B1 is checked there. On macOS it is checked only
  where someone runs it as root: CI doesn't. The modes it relies on are
  tested everywhere.
- **The process checks the runtime directory once, at start.** A directory
  opened to others while a process runs is refused by the commands
  (`no_approval_goes_through_a_dir_others_can_use`), but the running process
  goes on serving its socket, which its own mode still guards.
- **What others read before a repair.** A transcript others could reach is
  made private when a process opens it again (ADR 9.3), not before: what was
  in it until then may have been read, which the host log's `path_made_private`
  says. One no process opens again stays as it is until `doctor --fix`.
- **Another user's file in the state directory is tested only as `/`.**
  Making one takes root, so the ownership check is tested with `/` as the
  state directory; a file of another user's beneath it is refused by the
  same check, untested. The readers' check is tested with `/usr/bin`.
- **What reads transcripts lists names by path.** The project folders and
  their files, and the host logs, are listed by path (`read_dir`) once the
  directories above them pass the checks; each file is then opened again
  from the state directory, part by part. A directory swapped for a
  symlink in between can show a name from elsewhere, never what is in it.
  `doctor`, which changes modes, lists through what it opened instead.
- **Another user's config is tested only as root's `/etc/hosts`.** Making
  a file or folder of another user's takes root, so the ownership check is
  tested with a file and folder that are both root's, not as root;
  a private file of the user's in a folder of another's is refused by the
  same check, untested.
- **A session whose transcript is refused is served unrecorded.** Logging
  never stops forwarding (P1, P6), so the session goes on; only the host
  log's `session_log_failed` and `doctor`'s transcripts check say so.
- **Metadata files follow the umask.** `<pid>.json` (pids, cwd, the agent's
  command line, the socket's path) is created with the default mode, so a
  permissive umask leaves it readable by mode; the 0700 directory, with no
  access control list, is what keeps it private, and an ACL on the file
  itself isn't looked for. Not tested.
- **An editor's session that can't be locked runs unlocked.** The proxy
  passes it through; `session status` reports `lock_error`, and the host log
  records `lock_failed`. A headless resume is refused while the editor's process
  reports serving it, but a second process could acquire its lock if that
  process doesn't answer and the lock failure was transient (ADR 3.9).
- **Redaction is by field, not by value.** brnr redacts the MCP servers'
  `env` and `headers` in the requests that open a session (ADR 9.1). A secret
  elsewhere is recorded as it is: in an MCP server's `args` or `url`, in the
  agent's command line, or repeated by the agent itself in its messages, tool
  calls, errors or stderr (ADR 3.6 puts the agent's stderr in the host log),
  and it then reaches bridges with those events.
- **The editor's file and terminal answers are recorded as they are.** In
  an editor's session the raw ACP has what the editor read for the agent
  (unsaved buffers too, a `.env` among them if the agent asks for it) and
  the output of the commands it ran: redaction is by field (above), and
  none of these is redacted. This is accepted, not open: brnr doesn't
  redact file or terminal data, as it records the rest of the session as it
  passed (ADR 5.1), and the transcripts are only the user's to read (P13,
  ADR 9.3). The editor decides what it serves; brnr doesn't filter or gate
  those requests (ADR 2.7).
- **Approvals are the agent's own asks.** brnr answers the permission
  requests the agent makes; it doesn't sandbox the agent. A malicious
  adapter or npm package, or an agent in a permissive mode, acts without
  asking, and nothing in brnr can stop it.
- **`response_timeout` is headless only.** An editor's session waits for
  the editor; with `permission` (or `elicitation`) enabled, any of the user's
  processes may answer it (ADR 7.1, ADR 4.5).
- **An elicitation's `pattern` and `format` aren't checked.** brnr has no
  regex engine; a value that doesn't match them is sent, and the agent (or
  the MCP server behind it) checks them itself (ADR 4.5).
- **A headless turn waits on an elicitation.** The agent asked and brnr
  doesn't answer for the user, so an unattended run with no
  `response_timeout` waits for as long as nobody answers (ADR 4.5).
- **A state directory shared by two machines.** Session locks are in the
  state directory (ADR 3.9), so two machines sharing it (a network home
  directory) contend for one lock only where the file system carries `flock`
  between them, and a holder's pid is looked for on this machine: a lock
  another machine's process holds looks like no process's, and a resume
  here is refused only when it tries to take the lock and can't, naming
  that machine's pid. Not tested.
- **A refused runtime directory is a denial of service.** With neither
  `BRNR_DIR` nor `XDG_RUNTIME_DIR` set, the directory is `$TMPDIR/brnr-<uid>`;
  where `$TMPDIR` is shared (`/tmp` on Linux), another user who creates that
  path first makes brnr refuse to start until it is removed. They gain no
  access.
- **`brnr acp`'s stdout and stderr share one connection.** They come
  through the host in order, under one 16 MiB cap, rather than as two
  pipes: an editor that stops reading one eventually holds up the agent's
  writes to the other, and an editor closing its stderr gives the agent no
  EPIPE. The editor's stderr ends when `brnr acp` exits, not when the
  agent's does (ADR 2.6), which `adr_0002_stdout_and_stderr_end_on_their_own`
  checks; the rest isn't tested.
- **Requests in lines the host can't read keep the editor's id.** A
  request the host doesn't read (not JSON, past 32 MiB, nested past 128,
  or what the editor wrote after its last newline) reaches the agent untracked, with the
  editor's id. If the editor gave it an id the host has given a request
  still waiting, which it was never told, the answer is taken for that
  request's (ADR 2.4). Not tested.
- **The agent's ids are passed to the editor as they are.** An agent that
  reuses an id after the host answered a request with it (on
  `brnr prompt cancel`, `brnr permission allow` or `brnr elicitation
  accept`) can have the editor's
  late answer to the first taken as its answer to the second (ADR 2.4). Not
  tested; no agent known reuses ids.
- **Agents' lookup next to brnr.** A bare agent or bridge name is looked up
  next to the `brnr` executable first (ADR 11.2); that directory is trusted as
  `PATH` is.
- **A process the library's facade starts is the calling program.**
  `brnr::session::new` and `resume` start it as `<program> host` (ADR 8.5),
  and look up bare names next to that program: the program, which runs as
  the user, is trusted as the `brnr` executable is. A program that doesn't
  hand `host` to `brnr::run` runs its own code there, with the start
  request on its stdin, and the start fails: no process says it is ready.
  Tested only for a program that does (`tests/facade.rs`).
- **The adapters' dependencies.** Pinned and locked, but brnr doesn't review
  them: a compromised version of an adapter's package, pinned in a release,
  runs as the agent.

## Out of scope

As [SECURITY.md](https://github.com/brnrhq/brnr/blob/main/SECURITY.md) has it: the agents and the upstream adapters
themselves, what an agent does with an approval its user gave, and anyone who
can already run code as the user.
