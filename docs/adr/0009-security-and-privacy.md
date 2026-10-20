# 9. Security and privacy

Draft 2026-10-19. Implemented. Factor: N1 (ADR 1). Brings together former
ADRs 25, 53 and 59 (docs/adr/README.md maps them).

What brnr records and where, and what it reads: secrets kept out of what
it writes, session ids made safe file names and each cwd a folder of its
own, transcripts written only
where only the user can read them, and the config file read only where
only the user can change or read it. Security comes first (N1).
docs/threat-model.md says, for each trust boundary, what enforces each
claim and what the gaps are: B6 for 9.1, B4 for 9.2, B2 for 9.3, B7 for
9.4.

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

## 9.2 One file name per session id, one folder per cwd

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
    in another cwd writes in that one's folder.
  - `session list`, `event log` and `session resume` find a transcript by
    the id its records carry, not by its file name. The original id is in
    every record and in the lock file.
  - `session list --cwd` keeps brnr's rows by the cwd their records name,
    not by their folder, so two cwds whose hashes collide (below) each list
    only their own. With an agent named, its rows are what its
    `session/list` returns for that cwd, joined on the id: a session open
    in a process that the agent lists there is kept whatever its recorded
    cwd. An `unreachable` one, whose cwd its process doesn't say, is the
    cwd's if the agent lists it there or its transcript began there.
- A cwd's folder is `<claude>-<hash>` (`paths::project_key`, #120):
  - `<claude>` is the cwd as `~/.claude/projects` names it: every
    character but ASCII letters and digits is `-`. The folder sorts and
    reads next to Claude Code's (`-Users-me-src-3db3a304` beside
    `-Users-me-src`).
  - `<hash>` is the low 32 bits of FNV-1a 64 over the cwd's bytes, as 8
    lowercase hex digits. brnr computes it itself, so it can't change with
    a crate or with Rust; it is fixed for 1.x (ADR 3.9, ADR 8.4).
  - The cwd is taken as given, its trailing `/` removed (`/` stays `/`).
    No symlink is resolved and the file system isn't asked, so a cwd's
    folder is the same however the file system changes.
  - So `/a-b` and `/a/b` get two folders, and so do `/A` and `/a`, also
    where the file system ignores case. `/` is `--860189fe`.
  - Two cwds whose hashes collide share a folder. That is all a collision
    does: their sessions stay apart by id, and each record names its cwd.
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
- A transcript in a folder of the earlier rule, `<claude>` alone, is
  listed and read as any other. One rule is kept for those files, so that
  a session doesn't split into two transcripts: when a process opens a
  session in a cwd whose `<claude>-<hash>` file doesn't exist, but
  `projects/<claude>/<name>.jsonl` does and its first record's `cwd` is
  this cwd (the same trailing `/` removed), it goes on in that file and its
  `.acp.jsonl`. Any other cwd's file there is left as it is, and the
  session's transcript is in its own folder. That file is read as
  transcripts are (ADR 9.3).

**Consequences.**

- A claude-agent-acp session's events file has the same name as Claude
  Code's own transcript, in a folder whose name starts with Claude Code's
  and a `-`: a tool joins the two by that prefix, not by the exact name.
- Names of ids with capitals or other characters are longer and less
  readable. The id itself is in the records.
- The folder name alone doesn't say the cwd: two cwds that differ only
  where Claude Code puts a `-` are told apart by the hash, and the records
  say which is which.
- A session of before the hash, resumed in the cwd it began in, keeps its
  one transcript in the old folder; resumed in another cwd, it starts one in
  that cwd's folder, as any session does.

**Why.** A session id is the agent's text (P8), and it names the session's
transcript and its lock. Replacing other bytes with `_` kept a name inside
its folder, but not the session's identity: `a/b` and `a_b` were one file,
so two sessions wrote one transcript and contended for one lock. On macOS,
whose file systems ignore case and Unicode normalization by default,
`Sess-1` and `sess-1` were one file too.

The folder, as `~/.claude/projects` names it, was one for `/a-b` and
`/a/b`, and on macOS for `/A` and `/a`: two projects' transcripts in one
folder, told apart only by reading them (#120). A hash of the exact cwd
gives each its own, and keeps Claude Code's name in front, so the folders
still sort and read alike. A hash is rejected above for session ids
because a collision there put two sessions in one file. Here a collision
puts two cwds in one folder, which is all the earlier rule did for every
such pair: the sessions in it are still apart by id. Not resolving the cwd
keeps the name a function of what the agent was told, with no file system
call that could fail or change its answer. Leaving old files where they are
keeps P9, and continuing the one file a session already has keeps it one
transcript (P3).

**Considered.**

- Replacing, with a suffix (a hash of the id) only for ids that change: the
  names of ids that don't change stay readable, but a name then depends on
  a hash that can collide, and the rule is two rules.
- A hash of the id for every name: it is then not the agent's transcript's
  name.
- Keeping capitals: one name per id on Linux, two ids in one file on macOS.
- The exact name `~/.claude/projects` has (#120's first fix): one folder
  for `/a-b` and `/a/b`, the cwds told apart only by the cwd each record
  names, which every reader then has to check.
- Escaping every byte of the cwd, as session ids are: one folder per cwd
  with no collision, but not Claude Code's name, nor next to it.
- A suffix only on a collision: the name of a folder would depend on which
  cwd came first, and on what else is in `projects`, and the rule would be
  two rules.
- Resolving symlinks, or folding case on a file system that ignores it: a
  file system call per name, whose answer changes as the file system does.
- Moving old transcripts into the new folders: brnr doesn't rewrite what
  it has written (P9).

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
- What reads a transcript reads it only where it could have been written,
  by the same rules for every part of the path: `session list`, `event
  log`, `session resume`, `session delete`, catching up (ADR 10.2) and
  `doctor` open the state directory a transcript or host log is in, each
  directory below it (`projects` and the project folder, or `hosts`) and
  the file, each from the one above it (`openat`, `O_NOFOLLOW`), and check
  each through what was opened (`fstat`). A symlink, one that isn't a
  directory (or a regular file), one owned by someone else, or a file with
  other hard links isn't looked in or read, and a FIFO isn't waited on. One
  read through a link would be whatever the link points at, taken for a
  session of the user's. Reading makes nothing private: `doctor` warns of
  what others can reach, and of a state directory, `hosts` or `projects`
  that isn't a directory, however private. It opens each path the same
  way, lists a directory through what it opened, and `--fix` makes private
  only what it opened, through it (`fchmod`): it doesn't look below a
  symlink, nor change what is put at a name after it looked.
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

## 9.4 The config file is read only if only you can change or read it

- Before brnr reads the config file (`$BRNR_CONFIG`, else
  `$XDG_CONFIG_HOME/brnr/config.toml`, else `~/.config/brnr/config.toml`),
  it checks the file and the folder it is in, and refuses:
  - a file or folder that isn't the user's own;
  - a file or folder that the group or others can write;
  - a file that the group or others can read, whatever it holds;
  - a file that isn't a regular file (a FIFO is refused, not waited on);
  - on macOS, a file or folder with an access control list (`ls -le`),
    which can let others in whatever the mode says: `chmod -N` removes it.
    Any ACL is refused, a `deny` one too, as one rule (N4). On Linux an
    ACL's grants show in the mode's group bits, its mask, which are
    checked.
- A symlink is followed, as dotfile setups have them: what is checked is the
  file it resolves to, and that file's folder. The link's own folder, and
  the folders above, aren't checked.
- Both are checked through what the file is read from: the folder is opened,
  the file is opened from it without following a symlink (`openat`,
  `O_NOFOLLOW`), and each is checked by `fstat` of what was opened. What is
  checked is what is read.
- The error names the path, says why, with the owner or the mode it has,
  and gives the command that fixes it, one line for each problem:
  `brnr: /home/u/.config/brnr/config.toml: it can be read by the group and
  others (mode 644), and may hold secrets, so brnr won't read the config:
  chmod go-rw /home/u/.config/brnr/config.toml`. For a symlink it names the
  file it links to (`…/config.toml: /home/u/dotfiles/brnr.toml, which it
  links to, can be …`); for the folder, `its folder … can be written by
  others (mode 757) …: chmod go-w …`; for another user's, `it is owned by
  uid 0, not you (uid 501) …: sudo chown 501 …`.
- brnr never changes the file, its folder or their modes itself (P4), and
  `brnr doctor --fix` doesn't either. `brnr doctor` fails its config check
  with the same problems.
- Every command reads the config through one function, `config::load_all`,
  so the check covers each: `brnr acp`, `session new`, `session resume`,
  what takes `--profile`, and `doctor`.
- No config file (and no folder) is no config, as before: defaults apply.
- There is no override.

**Consequences.**

- A config file written with a common umask (0644) now fails every command
  that reads it until it is `chmod go-rw`'ed. This breaks setups that
  worked: the CHANGELOG says so.
- A config kept in a shared dotfiles repository works through a symlink to
  a private file; a checkout others can read doesn't.

**Why.** The config file names the commands brnr runs as the user: the
agent, bridges, MCP servers' commands. One others can write lets them run
code as the user. Its MCP servers' `env` and `headers` (ADR 6.3) hold
tokens, which 9.1 keeps out of everything brnr records; one others can
read gives them away anyway. N1 says only the user can reach brnr's files,
and that includes what brnr reads as well as what it writes (P13). ssh
refuses its keys on the same terms.

**Considered.**

- Refusing a file others can read only when it holds MCP `env` or
  `headers`: a secret can also be in an MCP server's `args` or `url`, or a
  bridge's command line, and two rules are harder to know than one (N4).
- Making it private, as 9.3 does for transcripts: brnr writes transcripts,
  but the config is the user's own file, which brnr only reads. Changing
  its mode is picking for the user (P4); the error gives the one command
  that does it.
- A warning, and reading it anyway: the command would run what others
  could have written. Security comes first (N1), and an explicit check that
  fails, fails (P7).
- Not following a symlink, as 9.3 doesn't for transcripts: dotfile setups
  link the config, and what matters is the file read and who can change
  it, which are checked through what is read. A folder above that others
  can write can swap in only another folder, which is then checked.
- Checking the folders up to `/`: they aren't brnr's, and a home directory
  others can search is common.
- A setting to allow a config the group can read: P13 is "only you".

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
    - `adr_0009_distinct_ids_get_distinct_names`.
    - `adr_0009_a_cwd_has_a_folder_of_its_own`: the names of `/`, `/a-b`,
      `/a/b`, `/A`, `/a` and `/Users/me/src`, written down once; a trailing
      `/` is the same cwd; `/a-b` and `/a/b`, `/A` and `/a` differ, also
      folded to lowercase.
  - [tests/headless.rs](../../tests/headless.rs):
    `adr_0009_distinct_ids_never_share_a_transcript`,
    `adr_0009_distinct_ids_never_share_a_lock`.
  - [tests/cli.rs](../../tests/cli.rs):
    - `adr_0009_cwds_alike_are_listed_apart`: `<dir>/x-y` and
      `<dir>/x/y`, named alike by `~/.claude/projects`, have two folders,
      each with only its own sessions' files, and each lists only its own
      sessions with `--cwd`, closed, open, and open in a process that
      doesn't answer, with an agent named or not.
    - `adr_0009_cwds_alike_are_resumed_apart`: each one's session has its
      own transcript, in its own folder, in `event log`, and `session
      resume` opens it in its own cwd.
    - `adr_0009_a_transcript_under_the_old_rule_is_continued`: a session's
      two files, in the folder of the earlier rule, are appended to when it
      is resumed in its cwd, and no new folder is made.
    - `adr_0009_another_cwds_transcript_under_the_old_rule_is_not_continued`:
      `<dir>/x-y`'s session, in that folder, resumed in `<dir>/x/y`, leaves
      the old files as they were and writes in `<dir>/x/y`'s own folder.
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
    - `adr_0009_a_transcript_is_read_only_where_it_could_be_written`: a
      symlinked project folder isn't looked in; a symlink, a FIFO and a
      directory aren't read.
    - `adr_0009_every_part_of_a_transcripts_path_is_the_users_own`: a
      symlinked `projects`, `hosts` or state directory isn't looked in or
      read through, nor is a file with another hard link or a directory of
      another user's.
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
    - `adr_0009_a_symlinked_hosts_or_projects_folder_is_never_read`: with
      `hosts` a symlink, `session list` reads no host log and `doctor`
      reads, shows and makes private none; with `projects` one too, its
      session isn't listed or logged.
- 9.4:
  - [tests/security.rs](../../tests/security.rs):
    - `adr_0009_a_config_others_can_read_or_write_is_refused`: a config
      file the group or others can read or write, and one in a folder they
      can write, fail `session new` and `brnr acp` before an agent starts,
      saying the path, the mode and the `chmod` that fixes it; the modes
      stay as they were.
    - `adr_0009_a_config_of_another_users_is_refused`: root's `/etc/hosts`
      as the config is refused for its owner and its folder's, with the
      `chown` (skipped as root).
    - `adr_0009_a_symlinked_config_is_checked_where_it_points`: a link to a
      private file is read; to one others can read, refused, naming that
      file.
    - `adr_0009_a_config_with_an_acl_is_refused` (macOS): a config file,
      and its folder, with an access control list are refused with the
      `chmod -N` that removes it, and the ACL stays.
    - `adr_0009_a_missing_config_is_no_config`.
  - [tests/doctor.rs](../../tests/doctor.rs):
    `adr_0009_doctor_fails_a_config_others_can_reach`: the file and the
    folder fail the config check with their fixes, `--fix` changes
    neither, and a missing config and a symlink to a private one are ok.
