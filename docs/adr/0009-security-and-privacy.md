# 9. Security and privacy

Draft 2026-10-19. Implemented. Factor: N1 (ADR 1). Brings together former
ADRs 25, 53 and 59 (docs/adr/README.md maps them).

What brnr records and where: secrets kept out of what it writes, session
ids made safe file names, and transcripts written only where only the user
can read them. Security comes first (N1). docs/threat-model.md says, for
each trust boundary, what enforces each claim and what the gaps are: B6
for 9.1, B4 for 9.2, B2 for 9.3.

## 9.1 Secrets are redacted in what brnr records

- The values of MCP servers' `env` and `headers` are replaced by
  `"<redacted>"` in everything brnr records: the host log, the raw ACP file
  (ADR 5.1) and the `started` request (ADR 3.3), and in `acp` events. This
  holds whoever sent the request: the host (a profile's servers in
  `session/new`, `session/load`, `session/resume` and `session/fork`) or
  the editor.
- Events never carry these values.
- The agent gets them unchanged, and the editor's bytes pass unchanged
  (P1).
- This is the one exception to P5's raw ACP. The keys and the structure
  stay, so the interpretation can still be checked.

**Why.** A profile's MCP servers go to the agent with their `env` and
`headers` (`GITHUB_TOKEN = "…"`, `Authorization`) in the requests that open
a session, and an editor's `session/new` carries its servers the same way.
Recorded as sent, that is a token on disk once per session, and in every
bridge that asks for `acp`, which may forward it anywhere (N1, P13).

**Considered.**

- Keeping them, protected by file permissions only.
- Not recording these requests at all: the record of the session's opening
  would be lost.

## 9.2 One file name per session id

- A session id becomes a file name by escaping, not replacing: lowercase
  ASCII letters, digits, `-` and `_` stay as they are, and every other byte
  of the id's UTF-8 is `%` and two lowercase hex digits (`a/b` is `a%2fb`,
  `x.acp` is `x%2eacp`, `Sess-1` is `%53ess-1`). The empty id is `%`.
  `paths::file_name` does it (P8).
- The escaping is one-to-one, also where the file system folds case or
  Unicode normalization, so distinct ids never share a name.
- The name has no `/` and no `.`. It can't leave its folder, is never `.`
  or `..`, and no events file can be named like another session's raw ACP.
- One namespace for lookup, locks and logs: a session is its agent's id,
  exact (ADR 3.2, P4).
  - Its lock is `sessions/<name>.lock`, one per id whatever the agent or
    folder (ADR 3.9).
  - Its transcript is `projects/<folder>/<name>.jsonl` and
    `<name>.acp.jsonl` (ADR 5.1), one per id and folder. A session resumed
    in another folder writes in that one.
  - `session list`, `event log` and `session resume` find a transcript by
    the id its records carry, not by its file name. The original id is in
    every record and in the lock file.
- An id whose name doesn't fit in a file name (255 bytes with the
  `.acp.jsonl`; an escaped byte takes three) is not supported. Its
  transcript fails to open and the host log says so, and its lock can't be
  taken (ADR 3.9).
- Existing files are not migrated or renamed (P9). Ids of lowercase
  letters, digits, `-` and `_` only, such as the UUIDs of Claude Code's and
  Codex's adapters, keep the names they had. A transcript under a name of
  the earlier rule (other bytes replaced by `_`) is still listed and read,
  by the ids its records carry. A session with such an id, resumed, writes
  a new file under its escaped name. A file two sessions already share
  stays as it is.

**Consequences.**

- A claude-agent-acp session's events file has the same folder and name as
  Claude Code's own transcript.
- Names of ids with capitals or other characters are longer and less
  readable. The id itself is in the records.
- The project folder stays as `~/.claude/projects` has it, which can put
  two cwds in one folder (`/a-b` and `/a/b`). Their sessions are still
  apart, as ids are, and each record names its cwd.

**Why.** A session id is the agent's text (P8), and it names the session's
transcript and its lock. Replacing other bytes with `_` kept a name inside
its folder, but not the session's identity: `a/b` and `a_b` were one file,
so two sessions wrote one transcript and contended for one lock. On macOS,
whose file systems ignore case and Unicode normalization by default,
`Sess-1` and `sess-1` were one file too.

**Considered.**

- Replacing, with a suffix (a hash of the id) only for ids that change: the
  names of ids that don't change stay readable, but a name then depends on
  a hash that can collide, and the rule is two rules.
- A hash of the id for every name: it is then not the agent's transcript's
  name.
- Keeping capitals: one name per id on Linux, two ids in one file on macOS.
- Escaping the project folder too: it would not be the agent's own folder
  name, which joining with its transcripts relies on.

## 9.3 Transcripts are written only where only you can read them

- Before brnr writes a record into a session's events file, its raw ACP
  file or a host log, these are the user's own and private, with no group
  or other bits: the state directory (`$BRNR_HOME`, else `~/.brnr`), each
  directory below it on the way, and the file. That is the boundary. The
  state directory's parents aren't brnr's; the state directory's mode keeps
  others out of everything below it.
- All three files are opened in one place, `log::open_append`, so one
  policy covers them.
- Each is opened with `O_NOFOLLOW`, each directory from the one above it
  (`openat`), and created 0700 (directories) or 0600 (files) if missing. A
  symlink is never followed, the state directory included. For a symlinked
  `~/.brnr`, set `BRNR_HOME` to where it points.
- Each is checked through the descriptor it was opened as (`fstat`), so
  what is checked is what is written to, and nothing can be swapped in
  between.
  - A directory or file of the user's that others could read, write or
    search is made private through the same descriptor (`fchmod`), keeping
    the owner's bits, before anything is written. The host log has a
    `made-private` event per path with the `mode` it had (`"644"`): what
    was there before could have been read. `brnr session delete` has no
    host log, so it says `made-private`, with the mode, on stderr
    (ADR 8.1).
  - A symlink, one that isn't a directory (or a regular file), one owned by
    someone else, or a file with other hard links (another name, anywhere)
    is refused. Nothing is written there, and the error names the path and
    what is wrong with it. For the host log, the process doesn't start
    (`log: <path>: is a symlink, so brnr won't write a transcript there
    …`). For a session's file, the session is served without it and the
    host log has `session-log-failed` with the error (ADR 5.1).
  - A FIFO is opened without blocking, so it is refused, not waited on.
- What reads a transcript reads it only where it could have been written:
  `session list`, `event log`, `session resume`, `session delete`,
  catching up (ADR 10.2) and `doctor` don't look in a symlinked project
  folder, and don't read a transcript or host log that is a symlink (it
  is opened `O_NOFOLLOW`) or isn't a regular file. One read through a link
  would be whatever the link points at, taken for a session of the user's.
  `transcript.rs` holds every read.
- There is no override. `log = false` (ADR 5.1) is the way to write
  nothing, and a transcript to share is copied out.

**Consequences.**

- A transcript that was readable by others is private again as soon as a
  process opens it, and from then on. What others read before is theirs.
- `brnr doctor` warns of transcripts others can read, and `brnr doctor
  --fix` makes private the files that no process opens again.
- A symlinked state directory or transcript is refused, with no shim (P9).
- Each open is a few more system calls: per session opened, not per
  record.

**Why.** P13 has transcripts only the user can read. The modes brnr gives
what it creates apply only to what it creates. A transcript restored from a
backup, copied in, or chmod'ed, readable by others under directories they
can search, would take the session's prompts, replies and tool output for
every local user to read, and a symlink would send them wherever it
points (N1).

**Considered.**

- Refusing what others can reach, rather than repairing it: the transcript
  would stay readable by others, and the session would go unrecorded, until
  the user ran `brnr doctor --fix`. Repairing through the descriptor is as
  safe, since only a file or directory already the user's is changed, and
  it closes the exposure at once. What can't be repaired safely is refused.
- Checking by path (`lstat`, then `chmod` and `open`): a path can be
  swapped between the check and the open. A descriptor can't.
- Following a symlinked state directory when the link is the user's: it is
  the user's choice, but where it points is another check by path, and
  `BRNR_HOME` says the same thing without one.
- Checking the state directory's parents up to `/`: they aren't brnr's, a
  home directory others can search is common, and the state directory
  being private is what keeps others out.
- A setting to allow group-readable transcripts: P13 is "only you", and a
  copy shares as well.

## Tests

Run `cargo test --release adr_0009_`. Named claims, by section:

- 9.1:
  - [tests/security.rs](../../tests/security.rs):
    `adr_0009_a_resumed_sessions_secrets_are_redacted`.
  - [tests/headless.rs](../../tests/headless.rs):
    `adr_0009_a_profiles_mcp_secrets_are_redacted`,
    `adr_0009_an_editors_mcp_secrets_are_redacted`.
  - [src/log.rs](../../src/log.rs):
    `adr_0009_secrets_are_redacted_keys_and_structure_stay`.
- 9.2:
  - [src/paths.rs](../../src/paths.rs):
    `adr_0009_distinct_ids_get_distinct_names`.
  - [tests/headless.rs](../../tests/headless.rs):
    `adr_0009_distinct_ids_never_share_a_transcript`,
    `adr_0009_distinct_ids_never_share_a_lock`.
- 9.3:
  - [src/log.rs](../../src/log.rs):
    - `adr_0009_new_and_private_paths_are_opened_as_they_are`: what is
      created is 0700 and 0600, and a private file is appended to as it is.
    - `adr_0009_what_others_can_reach_is_made_private_before_a_write`: a
      0644 file beneath private parents, and beneath 0755 ones, is made
      private, parents too, before the next write.
    - `adr_0009_a_symlink_is_never_followed`: the file, its folder and the
      state directory as symlinks are refused, and their targets left as
      they were.
    - `adr_0009_only_the_users_own_directories_and_files_are_written_to`: a
      file with another hard link, a FIFO, a file where a directory goes
      and another user's directory are refused.
  - [src/transcript.rs](../../src/transcript.rs):
    `adr_0009_a_transcript_is_read_only_where_it_could_be_written`: a
    symlinked project folder isn't looked in; a symlink, a FIFO and a
    directory aren't read.
  - [tests/security.rs](../../tests/security.rs):
    - `adr_0009_a_transcript_others_can_read_is_made_private_before_it_is_written`:
      a resumed session's events and raw ACP files, made 0644 beneath
      private and beneath 0755 parents, are private when it writes to them,
      and the host log has a `made-private` for each path.
    - `adr_0009_a_symlinked_transcript_is_never_followed`: a symlinked
      events file is left alone and `session-log-failed` says why; a
      symlinked state directory fails the start before the agent is
      prompted.
    - `adr_0009_a_symlinked_transcript_is_never_read`: a session whose
      events file is a symlink, or in a symlinked project folder, isn't
      listed or logged.
