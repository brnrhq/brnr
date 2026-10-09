# 59. Transcripts are written only where only you can read them

Proposed 2026-10-09. Implemented. Amends 1 (P13) and 22.

## Context

P13 has transcripts only the user can read. brnr created what it wrote under
`~/.brnr` 0700 (directories) and 0600 (files), but those modes apply only to
what it creates: a transcript that already existed was appended to as it
was. One restored from a backup, copied in, or chmod'ed, readable by others
under directories they could search, went on taking the session's prompts,
replies and tool output, readable by every local user. It was followed
through a symlink too, wherever that pointed. `brnr doctor` warned of
transcripts others could read, but only when it was run, and nothing made
the process that writes them check.

Every file a record goes into is opened in one place, `log::open_append`:
a session's events file, its raw ACP file, and the host log, so one policy
there covers all three.

## Decision

- Before brnr writes a record into a transcript or host log, the state
  directory (`$BRNR_HOME`, else `~/.brnr`), each directory below it on the
  way, and the file are the user's own, and private: no group or other
  bits. That is the boundary; the state directory's own parents aren't
  brnr's, and the state directory's mode is what keeps others out of
  everything below it.
- Each is opened with `O_NOFOLLOW`, each directory from the one above it
  (`openat`), and created 0700 or 0600 if missing. A symlink is never
  followed, the state directory included: a symlinked `~/.brnr` is set with
  `BRNR_HOME` to where it points instead.
- Each is checked through the descriptor it was opened as (`fstat`), so
  what is checked is what is written to, and nothing can be swapped in
  between:
  - a directory or a file of the user's that others could read, write or
    search is made private, through the same descriptor (`fchmod`), keeping
    the owner's bits, before anything is written. The host log says so, a
    `made-private` event per path with the `mode` it had (`"644"`): what
    was there before could have been read.
  - a symlink, one that isn't a directory (or a regular file), one owned by
    someone else, or a file with other hard links (another name, anywhere)
    is refused: nothing is written there, and the error names the path and
    what is wrong with it. For the host log, the process doesn't start
    (`log: <path>: is a symlink, so brnr won't write a transcript there
    …`); for a session's file, the session is served without it and the
    host log has `session-log-failed` with that error (ADR 22). A FIFO is
    opened without blocking, so it is refused, not waited on.
- There is no override: `log = false` (ADR 22) is the way to write nothing,
  and a transcript to share is copied out.

## Consequences

- A transcript that was readable by others is private again as soon as a
  process opens it, and from then on; what others read before is theirs.
  `brnr doctor --fix` is still how files that no process opens
  again are made private.
- A symlinked state directory, or a symlinked transcript, which brnr
  followed before, is now refused. There is no shim (pre-1.0).
- Each open is a few more system calls: per session opened, not per record.

## Considered

- Refusing what others can reach, rather than repairing it: the transcript
  would stay readable by others, and the session would go unrecorded, until
  the user ran `doctor --fix`. Repairing through the descriptor is as safe,
  since only a file or directory already the user's is changed, and it
  closes the exposure at once; what can't be repaired safely is refused.
- Checking by path (`lstat`, then `chmod` and `open`): a path can be swapped
  between the check and the open. Checking the descriptor can't be.
- Following a symlinked state directory when the link is the user's: it is
  the user's choice, but where it points is another check by path, and
  `BRNR_HOME` says the same thing without one.
- Checking the state directory's parents up to `/`: they aren't brnr's, a
  home directory others can search is common, and the state directory being
  private is what keeps others out.
- A setting to allow group-readable transcripts: P13 is "only you", and a
  copy shares as well.

## Tests

Run `cargo test --release adr_0059_`. Named claims and their assertions:

- [src/log.rs](../../src/log.rs)
  - `adr_0059_new_and_private_paths_are_opened_as_they_are`: what is
    created is 0700 and 0600, and a private file is appended to as it is.
  - `adr_0059_what_others_can_reach_is_made_private_before_a_write`: a 0644
    file beneath private parents, and beneath 0755 ones, is made private,
    parents too, before the next write, each in what it made private.
  - `adr_0059_a_symlink_is_never_followed`: the file, its folder and the
    state directory as symlinks are refused, and their targets left as
    they were.
  - `adr_0059_only_the_users_own_directories_and_files_are_written_to`: a
    file with another hard link, a FIFO, a file where a directory goes and
    another user's directory are refused.
- [tests/security.rs](../../tests/security.rs)
  - `adr_0059_a_transcript_others_can_read_is_made_private_before_it_is_written`:
    a resumed session's events and raw ACP files, made 0644 beneath private
    and beneath 0755 parents, are private when it writes to them, and the
    host log has a `made-private` for each path.
  - `adr_0059_a_symlinked_transcript_is_never_followed`: a symlinked events
    file is left alone and `session-log-failed` says why; a symlinked state
    directory fails the start before the agent is prompted.
