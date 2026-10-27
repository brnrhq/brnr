# 9. Security and privacy

Draft 2026-10-19. Implemented. Factor: N1 (ADR 1). Brings together former
ADRs 25, 53 and 59 (docs/adr/README.md maps them).

What brnr records and where, and what it reads: secrets kept out of what
it writes, session ids made safe file names and each cwd a folder of its
own, transcripts written only
where only the user can read them, the config file read only where
only the user can change or read it, and how long transcripts are kept.
Security comes first (N1).
docs/threat-model.md says, for each trust boundary, what enforces each
claim and what the gaps are: B6 for 9.1, B4 for 9.2, B2 for 9.3 and 9.5,
B7 for 9.4.

## 9.1 Secrets are redacted in what brnr records

- The values of MCP servers' `env` and `headers` are replaced by
  `"<redacted>"` in everything brnr records: the host log, the raw ACP file
  (ADR 5.1) and the `process_started` record's `start_request` (ADR 3.3),
  and in `acp` events. This
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
  the earlier rule (other bytes replaced by `_`), or in a folder of the
  earlier rule (`<claude>` alone), is an older brnr's: it stays as it is,
  and isn't read, as no record an older brnr wrote is (ADR 5.10). A
  session resumed writes its escaped name in its cwd's `<claude>-<hash>`
  folder, as any session does. A file two sessions already share stays
  as it is.

**Consequences.**

- A claude-agent-acp session's events file has the same name as Claude
  Code's own transcript, in a folder whose name starts with Claude Code's
  and a `-`: a tool joins the two by that prefix, not by the exact name.
- Names of ids with capitals or other characters are longer and less
  readable. The id itself is in the records.
- The folder name alone doesn't say the cwd: two cwds that differ only
  where Claude Code puts a `-` are told apart by the hash, and the records
  say which is which.
- A session of before the hash, resumed, starts a transcript in its cwd's
  folder, as any session does. The one it had is an older brnr's, which
  `doctor` lists (ADR 5.10). Going on in the old file (#120 kept that
  rule until #314) would add records of this brnr's after records it
  doesn't read.

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
keeps P9.

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
    `path_made_private` event per path with the `mode` it had (`"644"`): what
    was there before could have been read. `brnr session delete` has no
    host log, so it says `path_made_private`, with the mode, on stderr
    (ADR 8.1).
  - A symlink, one that isn't a directory (or a regular file), one owned by
    someone else, or a file with other hard links (another name, anywhere)
    is refused. Nothing is written there, and the error names the path and
    what is wrong with it. For the host log, the process doesn't start
    (`log: <path>: is a symlink, so brnr won't write a transcript there
    …`). For a session's file, the session is served without it and the
    host log has `session_log_failed` with the error (ADR 5.1).
  - On macOS, one with an access control list (`ls -le`), which can let
    others in whatever the mode says, is refused the same way, and the
    error says the `chmod -N` that removes it (`<path>: has an access
    control list (ls -le), which can let others read or write it whatever
    its mode, so brnr won't write a transcript there (chmod -N <path>
    removes it)`). brnr doesn't remove it (P4). Any ACL is refused, a
    `deny` one too, as for the config (ADR 9.4). On Linux an ACL's grants
    show in the mode's group bits, its mask, which are made private with
    the rest.
  - A FIFO is opened without blocking, so it is refused, not waited on.
- What reads a transcript reads it only where it could have been written,
  by the same rules for every part of the path: `session list`, `event
  log`, `session resume`, `session delete`, catching up (ADR 10.2) and
  `doctor` open the state directory a transcript or host log is in, each
  directory below it (`projects` and the project folder, or `hosts`) and
  the file, each from the one above it (`openat`, `O_NOFOLLOW`), and check
  each through what was opened (`fstat`). A symlink, one that isn't a
  directory (or a regular file), one owned by someone else, one with an
  access control list (macOS), or a file with other hard links isn't
  looked in or read, and a FIFO isn't waited on. One read through a link
  would be whatever the link points at, taken for a session of the user's;
  one with an ACL, what others may have written. Reading makes nothing
  private: `doctor` warns of what others can reach, of an ACL, with the
  `chmod -N` that removes it, of what it can't open (which readers pass
  over), and of a state directory, `hosts` or `projects` that isn't a
  directory, however private. It opens each path
  the same way, lists a directory through what it opened, and `--fix`
  makes private only what it opened, through it (`fchmod`): it doesn't
  look below a symlink, nor change what is put at a name after it looked,
  nor remove an ACL.
  `transcript.rs` holds every read.
- There is no override. `log = "off"` (ADR 5.1) is the way to write
  nothing, and a transcript to share is copied out.

**Consequences.**

- A transcript that was readable by others is private again as soon as a
  process opens it, and from then on. What others read before is theirs.
- `brnr doctor` warns of transcripts others can read, and `brnr doctor
  --fix` makes private the files that no process opens again.
- A symlinked state directory or transcript is refused, with no shim (P9).
- On macOS, a transcript or a directory on its way with an ACL leaves the
  session unrecorded, or the process not started for the state directory
  or `hosts`, until the user runs the `chmod -N` the error gives.
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
- Removing an ACL, as the mode is made private: the mode brnr gave what
  it creates is brnr's to restore, but an ACL is added on purpose, by the
  user, an administrator or a tool, and removing it is picking for them
  (P4). The error gives the one command that does it.
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

Remote routing profiles use the same reader and checks
(ADR 12.2). The remote command group only inspects configuration and remote
installations; it has no configuration-editing commands. Symlinked config
files retain the normal read-only support below.

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
- Every config read goes through `profiles::load_all` and
  `profiles::read_private`, including local routing profiles and
  `remote check --profile`: the same check covers launches, commands
  selecting a profile and `doctor`. Explicit SSH destinations do not read
  local config (ADR 12).
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

## 9.5 Transcripts are kept until they are pruned

Resolves #373.

By default brnr keeps every transcript and host log until the user deletes
it: nothing has an age limit, and nothing is deleted without being asked
for. Two controls prune by age, by one rule:

- `brnr session prune --older-than <s> [--dry-run] [--json]` prunes when
  it is run (ADR 8.1). `--dry-run` says what it would delete, and deletes
  nothing.
- `retention = <s>`, at the top of the config file (ADR 6.1), outside the
  profiles: every brnr process prunes as it starts, whatever its profile.
  Unset, nothing is pruned. The command that starts the process reads it
  and puts it in the start request, as it does the profile (P10).

What is pruned:

- A session's transcript, every file `session delete --purge` deletes
  (ADR 8.1): its events and raw ACP files in every project folder, and
  those set aside beside them (ADR 5.10).
- The host log of a process that has gone.

Age:

- A session's age is its last activity: the time of the newest record a
  process recorded in any of its files that isn't a deletion, `session
  list`'s `last_active` (ADR 5.1). Not when it was created: a session in
  use for months is kept as long as it is used.
- A host log's age is its newest record's time.
- `<s>` is a whole number of seconds, as every duration is (ADR 8.1): what
  was last active more than `<s>` seconds ago is pruned. `--older-than 0`
  prunes every session no process has open. `retention = 0` fails to load
  (`config_invalid`): a process would delete every transcript as soon as
  its session closed, and other tools read 0 as "no limit" (P4). A value
  that isn't a whole number of seconds (`"30d"`, `-1`) fails to load too.

Never pruned:

- A session open in a process, or whose lock a process holds (ADR 3.9),
  however old its last activity. A session's lock is taken before its
  files are deleted, and its age read again under it: one opened, and
  closed, since it was listed is kept. A session whose lock is held is
  passed over, and one whose lock can't be taken is said, in `failed`.
- The session a headless start resumes, by the process that resumes it:
  it is in use from then on, though its lock is taken a moment later.
- The host log of a running process: one `process list` lists, or one
  whose pid is alive and that doesn't end with `process_exited` (a process
  with another runtime directory, ADR 3.9). A long-lived process keeps its
  host log whole until it exits; the next prune after that, once the log is
  old enough, deletes it.
- What brnr can't read the age of: a session with an events file whose
  newest record is in another format (ADR 5.10), a newer brnr's, or whose
  files hold no record brnr can read, as nothing in them says when it was
  last active (P4), and a host log likewise. `doctor`
  lists the files in another format; they are deleted by hand. A session
  brnr does read the age of takes the files an older brnr's set aside
  beside its own with it, as `--purge` does: they are the session's, and
  older than the file that took their place (ADR 5.10). Its age is read
  again, from every file of it, under its lock, for `--dry-run` too.
- The agent's own copy of a session: that is `session delete`'s.

How:

- Files are deleted as `--purge` deletes them (ADR 9.3, B2): the state
  directory, `projects` or `hosts`, and each project folder opened without
  following a symlink, each from the one before (`openat`, `O_NOFOLLOW`),
  and only the session's own names, escaped (ADR 9.2), removed from what
  was opened (`unlinkat`). A symlinked folder isn't looked in, and a
  symlink with a transcript's name is removed, not what it points at. A
  host log that is a symlink isn't listed.
- A session's events file goes last, so a prune cut short leaves the file
  the session is found by, and the next prune deletes the rest.
- A process prunes on a thread of its own, beside its start, which doesn't
  wait for it (N3). The thread finds what is open as the commands do: from
  the processes and the locks. A thread of the process that takes a lock
  waits while the prune holds one (`lock::holding`), so it never finds a
  session's lock held by its own process.
- A file that can't be deleted is said, and the rest are deleted all the
  same. A project folder that can't be listed is said, and nothing in it is
  deleted: what it holds of the session isn't known.

What it says:

- `--json` is `{older_than, before, dry_run, sessions, host_logs, failed}`:
  `before` the time the age gives, each session `{session, last_active,
  files}`, each host log `{host_id, last_active, files}`, `files` the
  paths deleted (or that would be, with `--dry-run`), and `failed` why a
  file wasn't deleted or a lock taken, as `session delete`'s `failed`. Text
  says the same: what it pruned, each session and host log with its last
  activity and its files, or `nothing was last active before …`.
- The command exits 1 if a file couldn't be deleted, 0 otherwise. Which
  sessions are open must be known: a runtime directory that can't be read
  fails it, with its error, and nothing is deleted.
- A process records what it pruned in its host log, `transcripts_pruned`,
  with the fields of `--json` but `dry_run`, when it pruned something or
  something failed. It can say nothing else: an editor's process has no
  terminal (P1), and a headless start isn't waiting for it. With `log =
  "off"` there is no host log, and nothing records it.
- `doctor` says what is kept: everything, or what each process prunes, with
  the age in days (`more than 30 days ago (retention = 2592000)`), so a
  value meant in days shows as seconds.
- No error code is added (ADR 8.7): a value that isn't one is a usage
  error (exit 2) or `config_invalid`, and a file that can't be deleted is
  an I/O error on brnr's own files, `unclassified`, in `failed`.

**Why.** A transcript holds prompts, replies, tool calls with their input
and output, diffs of the user's code and, where the profile or strict mode
passes `fs` and `terminal`, unsaved buffers and command output (ADR 2.7).
An enterprise's policy, or a user's, limits how long such data is kept, and
`--purge` one session at a time can't apply one; the host logs, shared,
could not be removed at all (N1). Keeping everything by default is what
brnr did, and deleting what a user didn't ask to delete would lose it
silently (P3): the default is said in the README and the docs instead. Age
by last activity is `session list`'s, one notion of when a session was used
(N4), and never deletes a session in use. Seconds are every duration's unit
in brnr (ADR 8.1); `doctor` turns them into days where a mistake would show.
The same deletion as `--purge`, and the session's lock held while it runs,
keep the boundary B2 already has: nothing outside the state directory, and
nothing a process uses.

**Consequences.**

- `retention` deletes, at every start, what is older: setting it is
  choosing that. `session prune --older-than <s> --dry-run` shows what the
  same age would delete.
- A session resumed after it was pruned starts a new transcript; what came
  before is gone from brnr, and the agent's copy, if it kept one, is the
  agent's.
- An editor that loads a session in the moment a prune deletes it shares
  it (ADR 3.9) and may write into a file that is then deleted, as with
  `session delete --purge`. The prune holds the lock for the time its
  unlinks take.
- Files in an older format stay until they are deleted by hand, but those
  set aside beside a session that is pruned.
- A process that exits while its prune runs waits for it before its last
  records are written, as it waits for them (ADR 5.5): a few seconds at
  most, the status of every process. A signal gives up on it, and the
  exit stops it there; what was deleted by then isn't recorded.

**Considered.**

- Deleting after an age by default (30 or 90 days): data the user didn't
  ask to delete would go, silently (P3).
- A duration with a unit (`"30d"`, `--older-than 30d`), as the issue had
  it: easier to read, but a second way to write a duration, in config and
  on the command line, beside every other in seconds (ADR 8.1, N4).
- A floor (a day) on `retention`, against a value meant in days: one more
  rule; `doctor` shows the age in days instead.
- Age by creation, or by the file's modification time: a session in use
  for months would go; a file's time changes with a backup or a copy, and
  `session list` has the record's.
- Rotating a long-lived process's host log: the host log is one file that
  `doctor` and a catch-up read whole (ADR 10.2), and two would be one more
  thing to join. A process that runs for longer than the retention keeps
  its log until it exits.
- Pruning from `doctor --fix`: `doctor` never deletes a log (P4), and the
  command already does it when asked.
- Pruning by mtime what brnr doesn't read: a guess at an age (P4), in files
  `doctor` already lists.
- A per-profile `retention`: the state directory is one for every profile,
  and a session's profile can change as it is resumed; one value for all
  is one rule.
- `brnr prune` at the top level, or under `event`: the transcripts are
  sessions', as `session delete --purge` deletes them, and the host logs
  go by the same age.
- Pruning synchronously before the start: the start would wait for a walk
  of every transcript and a status from every process (N3).

## Tests

Run `cargo test --release adr_0009_`. Named claims, by section:

- 9.1:
  - [tests/security.rs](https://github.com/brnrhq/brnr/blob/main/tests/security.rs):
    `adr_0009_a_resumed_sessions_secrets_are_redacted`.
  - [tests/headless.rs](https://github.com/brnrhq/brnr/blob/main/tests/headless.rs):
    `adr_0009_a_profiles_mcp_secrets_are_redacted`,
    `adr_0009_an_editors_mcp_secrets_are_redacted`.
  - [src/log.rs](https://github.com/brnrhq/brnr/blob/main/src/log.rs):
    `adr_0009_secrets_are_redacted_keys_and_structure_stay`.
- 9.2:
  - [src/paths.rs](https://github.com/brnrhq/brnr/blob/main/src/paths.rs):
    - `adr_0009_distinct_ids_get_distinct_names`.
    - `adr_0009_a_cwd_has_a_folder_of_its_own`: the names of `/`, `/a-b`,
      `/a/b`, `/A`, `/a` and `/Users/me/src`, written down once; a trailing
      `/` is the same cwd; `/a-b` and `/a/b`, `/A` and `/a` differ, also
      folded to lowercase.
  - [tests/headless.rs](https://github.com/brnrhq/brnr/blob/main/tests/headless.rs):
    `adr_0009_distinct_ids_never_share_a_transcript`,
    `adr_0009_distinct_ids_never_share_a_lock`.
  - [tests/cli.rs](https://github.com/brnrhq/brnr/blob/main/tests/cli.rs):
    - `adr_0009_cwds_alike_are_listed_apart`: `<dir>/x-y` and
      `<dir>/x/y`, named alike by `~/.claude/projects`, have two folders,
      each with only its own sessions' files, and each lists only its own
      sessions with `--cwd`, closed, open, and open in a process that
      doesn't answer, with an agent named or not.
    - `adr_0009_cwds_alike_are_resumed_apart`: each one's session has its
      own transcript, in its own folder, in `event log`, and `session
      resume` opens it in its own cwd.
    - `adr_0009_a_transcript_under_the_old_rule_is_left_as_it_was`: a
      session's two files, in the folder of the earlier rule, stay as they
      were when it is resumed in its cwd, which writes in its own folder.
- 9.3:
  - [src/log.rs](https://github.com/brnrhq/brnr/blob/main/src/log.rs):
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
  - [src/transcript.rs](https://github.com/brnrhq/brnr/blob/main/src/transcript.rs):
    - `adr_0009_a_transcript_is_read_only_where_it_could_be_written`: a
      symlinked project folder isn't looked in; a symlink, a FIFO and a
      directory aren't read.
    - `adr_0009_every_part_of_a_transcripts_path_is_the_users_own`: a
      symlinked `projects`, `hosts` or state directory isn't looked in or
      read through, nor is a file with another hard link or a directory of
      another user's.
  - [tests/security.rs](https://github.com/brnrhq/brnr/blob/main/tests/security.rs):
    - `adr_0009_a_transcript_others_can_read_is_made_private_before_it_is_written`:
      a resumed session's events and raw ACP files, made 0644 beneath
      private and beneath 0755 parents, are private when it writes to them,
      and the host log has a `path_made_private` for each path.
    - `adr_0009_a_symlinked_transcript_is_never_followed`: a symlinked
      events file is left alone and `session_log_failed` says why; a
      symlinked state directory fails the start before the agent is
      prompted.
    - `adr_0009_a_symlinked_transcript_is_never_read`: a session whose
      events file is a symlink, or in a symlinked project folder, isn't
      listed or logged.
    - `adr_0009_a_symlinked_hosts_or_projects_folder_is_never_read`: with
      `hosts` a symlink, `session list` reads no host log and `doctor`
      reads, shows and makes private none; with `projects` one too, its
      session isn't listed or logged.
    - `adr_0009_a_transcript_with_an_acl_is_never_written` (macOS): an
      events file with an ACL is `session_log_failed`, with the `chmod -N`,
      and isn't written; a state directory with one fails the start before
      the agent is prompted; the ACL stays.
    - `adr_0009_a_transcript_with_an_acl_is_never_read` (macOS): a session
      whose events file, or project folder, has an ACL isn't listed or
      logged; `doctor` warns of it with the `chmod -N`, and `--fix` leaves
      it.
  - [tests/doctor.rs](https://github.com/brnrhq/brnr/blob/main/tests/doctor.rs):
    `adr_0009_doctor_says_what_it_cant_open_and_what_fix_left`: a
    transcript of mode 000 is a warning, `--fix` leaves it, and the example
    it then gives is what is left, not what it made private.
  - [src/ctl/doctor.rs](https://github.com/brnrhq/brnr/blob/main/src/ctl/doctor.rs):
    `adr_0009_fix_changes_what_was_opened_not_its_name`: a file `--fix`
    looked at, then swapped for a symlink, is made private as it was
    opened, and what the link points at is left as it was; looked at
    again, the symlink is a problem `--fix` doesn't fix.
- 9.4:
  - [tests/security.rs](https://github.com/brnrhq/brnr/blob/main/tests/security.rs):
    - `adr_0009_a_config_others_can_read_or_write_is_refused`: a config
      file the group or others can read or write, and one in a folder they
      can write, fail `session new` and `brnr acp` before an agent starts,
      saying the path, the mode and the `chmod` that fixes it; the modes
      stay as they were.
    - `adr_0009_a_config_of_another_users_is_refused`: root's `/etc/hosts`
      as the config is refused for its owner and its folder's, with the
      `chown` (can't run as root: `skip` fails it in CI).
    - `adr_0009_a_symlinked_config_is_checked_where_it_points`: a link to a
      private file is read; to one others can read, refused, naming that
      file.
    - `adr_0009_a_config_with_an_acl_is_refused` (macOS): a config file,
      and its folder, with an access control list are refused with the
      `chmod -N` that removes it, and the ACL stays.
    - `adr_0009_a_missing_config_is_no_config`.
  - [tests/doctor.rs](https://github.com/brnrhq/brnr/blob/main/tests/doctor.rs):
    `adr_0009_doctor_fails_a_config_others_can_reach`: the file and the
    folder fail the config check with their fixes, `--fix` changes
    neither, and a missing config and a symlink to a private one are ok.
- 9.5:
  - [tests/cli.rs](https://github.com/brnrhq/brnr/blob/main/tests/cli.rs):
    - `adr_0009_prune_deletes_what_was_last_active_before_the_age`: a
      session last active long ago is pruned, its raw ACP before its
      events, and one opened long ago but active since is kept; a deletion
      isn't activity; the host log of a process gone is pruned, and one
      whose pid is alive kept unless it ends with `process_exited`;
      `--dry-run` says the same and deletes nothing; a symlinked folder or
      host log isn't followed; `--older-than 0` prunes every session no
      process has open.
    - `adr_0009_prune_never_touches_an_open_or_locked_session`: an open
      session, a session whose lock another process holds, and a running
      process's host log stay, with `--older-than 0`.
    - `adr_0009_a_process_prunes_as_it_starts_when_retention_is_set`:
      without `retention` nothing is pruned; with it, a start prunes an old
      session but the one it resumes, and the host log has
      `transcripts_pruned`.
    - `adr_0009_prune_keeps_a_session_it_cant_read_the_age_of`: a session
      with an old file and a file whose newest record is a newer brnr's is
      kept, both files, with `--dry-run` and without; one whose older
      brnr's file is set aside goes with that file.
    - `adr_0009_a_process_records_its_prune_before_it_exits`: a process
      that stops as soon as its turn is done has `transcripts_pruned` in
      its host log.
    - `adr_0009_retention_is_a_number_of_seconds_not_0`: `0`, `"30d"` and
      `-1` fail to load, `config_invalid`, before an agent starts; `doctor`
      says the age in days.
