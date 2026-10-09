# 50. A session that can't be locked isn't served headless

Accepted 2026-10-08. Implemented. Amends 3; resolves #57.

## Context

ADR 3 makes taking a session taking its lock, and says what happens when
another process holds it. It doesn't say what happens when the lock can't
be taken at all: `sessions/` isn't a private directory, something other than
a file (a directory, a symlink) is where the lock file goes, the process is
out of file descriptors, or the file is locked by a process that doesn't say
which. The code took that for ownership: the session was served without a
lock, and only the host log said so (`lock-failed`). Two `brnr start`s whose
agent opened the same session both succeeded, and served it together, each
with its own process: what P11 and ADR 3 exist to prevent. A headless resume
was already refused, as `start --resume` takes the lock before the agent
hears of the session; a new session and a fork were not.

A lock that can't be taken is neither held nor free: whether another process
serves the session can't be known. P4 says what brnr does when it can't be
sure: it refuses in the client role, and passes the thing through untouched
in the proxy role, and says so.

## Decision

- A headless process never serves a session it couldn't lock. A start whose
  session (new, resumed, loaded) can't be locked fails before it commits
  (ADR 7): `brnr start` exits with status 1 and the cause, naming the lock
  file and the system's error ("the agent opened sess-1, which can't be
  locked: …/sessions/sess-1.lock: Is a directory"), no prompt reaches the
  agent, and the process and its agent stop. A resume's lock is taken before
  the agent is asked (ADR 3), so the agent never hears of it. A `fork` the
  agent opens into a session that can't be locked is refused the same way as
  one another process holds, and the session forked from goes on.
- An editor's process passes an editor's session it couldn't lock through:
  `session/new`, `session/load` and `session/resume` reach the agent and are
  answered as they would be without brnr, and the process keeps the
  session's transcript. Refusing would change how the editor and the agent
  interact (P1) on a guess (P4): ADR 3's refusal is for a session another
  process is known to hold. A session/new can't be refused anyway: the agent
  has opened the session by the time brnr knows its id.
- That it isn't locked is said where it can be seen: `status` has
  `lock_error`, the cause, and its text says "not locked: …; nothing stops
  another process serving it too"; the host log has `lock-failed`.
  `start --resume` of it still refuses, since the process serving it says
  so when asked (ADR 3's check of the running processes).
- `brnr doctor` fails a `sessions/` that isn't a private directory, and
  anything but a file where a lock file goes, saying the session can't be
  locked or started headless and what to remove. It no longer takes such a
  thing for a lock left by a process that is gone; `--fix` leaves it to the
  user, since brnr didn't make it.

## Consequences

The failure the issue describes can no longer give a session two headless
owners. An editor's session that can't be locked can be served by a headless
process whose lock does succeed (a transient error, the editor's process not
answering when `start --resume` asks): a caveat of the proxy role, which
`status` and `doctor` make visible.

## Considered

- Serving an unlocked session and saying so in the host log (what the code
  did): the host log is the record, not the telling (P3), and the session
  could have two owners (P11).
- Refusing an editor's `session/load` and `session/resume` too, as the
  request hasn't reached the agent yet: brnr can't be sure another process
  holds the session, and in the proxy role it doesn't refuse on a guess (P4).
  It would also treat a load differently from a new session, which can't be
  refused.
- `doctor --fix` removing what is in a lock file's place: a directory may
  hold the user's files, and a symlink's target is the user's to judge.

## Tests

Run `cargo test --release adr_0050_`. Named claims and their assertions:

- [tests/headless.rs](../../tests/headless.rs)
  - `adr_0050_an_editors_session_that_cant_be_locked_is_passed_through`.
- [tests/doctor.rs](../../tests/doctor.rs)
  - `adr_0050_what_keeps_a_session_from_being_locked_fails`.
- [tests/cli.rs](../../tests/cli.rs)
  - `adr_0050_a_new_session_that_cant_be_locked_isnt_started`.
  - `adr_0050_a_resume_that_cant_be_locked_isnt_started`.
  - `adr_0050_a_fork_that_cant_be_owned_is_refused`.
  - `adr_0050_a_second_owner_of_a_new_session_isnt_started`.
