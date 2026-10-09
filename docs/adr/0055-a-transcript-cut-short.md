# 55. A transcript cut short

Accepted 2026-10-08. Implemented. Amends 22: what reads a transcript reads
past a line it can't, and says so. Resolves #61.

## Context

A process killed mid-write (SIGKILL, a power cut, a full disk) leaves its
last line in a session's events file cut short: `{"ts":"partial`, with no
newline. `list --inactive` read the first and last lines of each events
file and passed over a file where either didn't parse, so the session
vanished: `list` didn't show it, `log` said `no session`, and `--resume`
couldn't find its cwd, agent and profile (ADR 14), although every record
before the cut was there. A process that took the file again glued its
first record onto the cut line, so that record was lost too. `log` passed
over a whole line that didn't parse without saying so, and never mentioned
a line cut short at the end.

The logger already ends a line that a failed write of its own cut short
before it writes again (log.rs), so a line that isn't a record can be
anywhere in a file, not only at its end.

## Decision

- What a transcript is for `list`, `--resume` and `doctor` comes from the
  first and last records brnr can read in it: a line that isn't a JSON
  object, cut short or not, is passed over. The last is still read
  backwards, so a long transcript costs no more than its last records. A
  line cut short is never taken for a record: a JSON object ends only at
  its last byte.
- For each case, then:
  - ending in a newline, as written: its last line is the last record;
  - ending partway through a line: its last whole record;
  - a whole line that isn't a record, last or anywhere: passed over;
  - empty, or no line brnr can read: no session, as there is nothing to
    say which, where, or when (`log` says `no session`).
- `log` shows every record it can read, and says on stderr each line it
  can't (`brnr: <file>:<line> isn't a record brnr can read, not shown`),
  and, once the transcript won't grow (its process has gone), a file that
  ends partway through a line (`brnr: <file> ends partway through a record
  (<n> bytes), not shown`). Of a running session, what follows the last
  newline is a line still being written, and isn't mentioned. Its exit
  status is still 0: what it was asked for, the transcript, is shown (P3).
- A process that opens a transcript to append to it, ending partway
  through a line, starts its first record on a line of its own. The line
  cut short stays as it was.
- Nothing that reads a transcript writes to it: brnr never repairs,
  truncates or rewrites one (P3).

## Consequences

- A session killed mid-write is listed, logged and resumed as of its last
  whole record; the bytes of the cut record stay in the file, and `log`
  points at them.
- A file whose lines are all cut short or broken is not a session; its
  bytes are still there to read.

## Considered

- Truncating the file to its last newline when it is read or reopened:
  repairs it, but destroys what was there, and a reader would write.
- Failing `log` on a line it can't read: the records around it are what
  the user asked for, and stay readable.
- Taking the session's id from the file name when no record can be read:
  the name is a sanitized id (ADR 22), not the id, and there would be no
  cwd, agent or profile to resume it with.
