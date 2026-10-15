# 65. Session locks live with the transcripts, and where things live is fixed for 1.x

Accepted 2026-10-14. Implemented. Amends 3 and 64; resolves #155, and the
part of #70 that is where sessions are found.

## Context

brnr finds two directories, each on its own:

- the runtime directory, `$BRNR_DIR`, else `$XDG_RUNTIME_DIR/brnr`, else
  `$TMPDIR/brnr-<uid>`: each process's control socket and metadata, and,
  by ADR 3, the session locks;
- the state directory, `$BRNR_HOME`, else `~/.brnr`: the transcripts.

Two environments that find different runtime directories and one state
directory had different locks on one transcript. On Linux an interactive
shell has `XDG_RUNTIME_DIR` and a cron job, or an `ssh` command, doesn't;
anywhere, two values of `BRNR_DIR` do it. A session resumed from each ran in
two processes at once, both appending to one transcript, which is what P11
and ADR 3 exist to prevent; each environment's commands listed only its own
process, and the other's session as `inactive`. Reproduced in
`adr_0065_a_session_is_owned_once_however_the_runtime_dir_is_found`, which
failed before this change.

The lock guards the transcript. A lock kept somewhere else guards it only
for the processes that agree on where else.

Issue #70 asks that a session's identity be defined the same way across
projects, agents, locks, logs and the CLI's lookup before 1.0. Part of that
is where these files are and how a session id and a cwd become their names,
which the records so far describe but don't promise.

## Decision

- A session's lock is `sessions/<id>.lock` in the state directory (the id
  escaped as `paths::file_name` does, ADR 53): beside `projects/` and
  `hosts/`, one flat directory, found from the session id alone, which a
  resume has before it knows the session's cwd. Processes that write one
  transcript contend for one lock, however each found its runtime
  directory. Sockets and metadata stay in the runtime directory: they are a
  process's, and go with the machine's run; a transcript's lock doesn't.
- `sessions/` is held to the runtime directory's terms (P13, ADR 50):
  created 0700, refused with the cause when it isn't the user's own private
  directory or is a symlink; each lock is 0600 and opened `O_NOFOLLOW`. It
  isn't repaired as transcripts are (ADR 59): nothing of the user's is
  recorded in it, and in a directory others can write to another user can
  put a file where a lock goes.
- The lock records its holder's runtime directory, beside its pid and the
  session's id. A command whose runtime directory isn't the holder's can't
  reach the holder (its socket and metadata aren't here), and says so rather
  than guessing (P4) or saying nothing (P3): `session resume` is refused
  with `sess-1 is running in process 4466 with runtime directory
  /run/user/1000/brnr (BRNR_DIR=/run/user/1000/brnr reaches it)`, with
  `--take-over` as without (it can't ask a process it can't reach to
  close); `session list` shows the session `unreachable`, with that pid,
  its agent and cwd unknown here, and not `inactive`; commands on the
  session (`close`, `delete`, `prompt send`, …) say the same; and an
  editor's load of it is answered with the process and `BRNR_DIR=… brnr
  session close <id>` to release it. The directory after `BRNR_DIR=` is
  one shell word, in single quotes when it has a space, a quote or another
  character a shell reads, so that what is shown runs as it is shown.
- Fixed for 1.x, kept within the major version from 1.0.0 (P9), as ADR 64
  has it: where things are is covered, and what host logs and lock files
  contain, and what is in the runtime directory, aren't:
  - how the runtime directory is found: `$BRNR_DIR`, else
    `$XDG_RUNTIME_DIR/brnr`, else `$TMPDIR/brnr-<uid>`;
  - how the state directory is found, `$BRNR_HOME`, else `~/.brnr`, and
    its layout: `projects/<project_key>/<file_name>.jsonl` and
    `<file_name>.acp.jsonl`, `hosts/<host id>.jsonl`,
    `sessions/<file_name>.lock`;
  - `file_name` (ADR 53): lowercase ASCII letters, digits, `-` and `_`
    stay; every other byte of the id's UTF-8 is `%` and two lowercase hex
    digits; the empty id is `%`;
  - `project_key`: the session's cwd with every character but ASCII
    letters and digits replaced by `-`, as `~/.claude/projects` has it.

  What a transcript's records contain, and how a reader tells formats
  apart, is what remains of #70; not this record's. A lock's fields (its
  pid, session id and runtime directory) are for brnr's own commands, and
  can change in any release.

## Consequences

- A lock left in `$BRNR_DIR/sessions/` by an earlier version is nobody's and
  read by nothing: remove the directory. No shim (P9).
- A lock file that outlives a reboot, which a tmpfs runtime directory used
  to clear: a file nobody holds, as a dead process's already was (ADR 3),
  taken as it is. `doctor` lists it under `session locks`, where `--fix`
  removes it; the runtime directory's check is about metadata and sockets.
- A state directory on a file system where `flock` doesn't work (some
  network file systems): the lock can't be taken, and the start is refused
  with the cause (ADR 50), not served as if free.
- The `dir` in a lock is for saying where the holder is, written absolute;
  whether a lock is held is still the kernel's answer, not the file's.
- `log = false` writes no transcripts, but its sessions' locks are in the
  state directory all the same: the lock is ownership, not logging.
- A state directory two machines share (a network home directory): they
  contend for one lock only where the file system carries `flock` between
  them, and a holder's pid means nothing on the other machine. A gap in the
  threat model, as it was with the lock in each machine's runtime directory.

## Considered

- Keeping the lock in the runtime directory and refusing when the
  transcript's last writer isn't a process listed here: an inference from
  the transcript, as ADR 3 already rejected (its option B), wrong for
  `log = false`.
- A lock in both directories: two locks are the problem.
- The lock next to the transcript, in the project folder: the lock is taken
  before the cwd is known (a resume of a session only the agent knows has
  none), and one id resumed in several folders has a transcript in each
  (ADR 53) and is one session.
- One directory for everything: the runtime directory exists for the
  socket's path length (`sun_path`) and because what is in it is a run's.
- Repairing an open `sessions/` as ADR 59 repairs transcripts: see the
  decision.

## Tests

Run `cargo test --release adr_0065_`. Named claims and their assertions:

- [tests/cli.rs](../../tests/cli.rs)
  - `adr_0065_a_session_is_owned_once_however_the_runtime_dir_is_found`: a
    resume with another `BRNR_DIR` and the same `BRNR_HOME` is refused,
    naming the process and its runtime directory, with and without
    `--take-over`; the agent isn't asked; nothing starts in the other
    runtime directory; `session list` there shows the session
    `unreachable` with that pid; `session close` and `session delete` there
    are refused the same way, and the agent isn't asked.
  - `adr_0065_the_brnr_dir_said_is_one_shell_word`: with a holder whose
    runtime directory has a space, and one whose has a quote, the refusal
    names `BRNR_DIR='<its runtime directory>'`, and that put before
    `brnr session close sess-1` in a shell closes the session.
- [tests/headless.rs](../../tests/headless.rs)
  - `adr_0065_an_editors_load_held_from_another_runtime_dir_is_refused`: an
    editor's process with another `BRNR_DIR` answers the load with the
    holder and `BRNR_DIR=<its runtime directory> brnr session close sess-1`;
    the agent isn't asked.
  - `adr_0065_the_release_command_runs_as_shown`: with the holder's runtime
    directory having a space, and then a quote, the command in the answer
    has it quoted, and run by a shell it releases the session, which then
    loads.
- [src/paths.rs](../../src/paths.rs)
  - `adr_0065_brnr_dir_is_one_shell_word`: a plain directory is written as
    it is; one with a space, a quote, `$`, `` ` ``, `;`, `*` or a leading
    `~`, or an empty one, in single quotes, each quote in it as `'\''`.

ADR 3's, 50's and 53's tests run against the lock in its new place, as do
`what_brnr_makes_is_private_whatever_the_umask` (tests/security.rs) and
`stale_session_locks_are_removed` (tests/doctor.rs).
