# 3. Ownership is a per-session lock

Accepted 2026-10-07. Not yet implemented.
Replaces the "refused if a running host already serves the session" check of
former decision 10; resolves review item 8.

## Context

A session must be served by one process at a time (P11): two processes on
one session append to one transcript and answer one agent session twice.

brnr checked by asking every running process which sessions it had. A
process that doesn't answer can't say, and `start --resume` went ahead anyway
(`Err(_) => {}` in `start`, src/ctl/talk.rs): a guess (P4). Nothing checked
an editor's `session/load` or `session/resume` at all, so an editor could open
a session a headless process was running.

## Decision

- Every process holds an exclusive `flock` on `$BRNR_DIR/sessions/<id>.lock`
  (the id sanitized as for transcripts) for each session it serves, with its
  pid written inside. Taking a session is taking its lock; a process that
  dies releases it, with nothing to clean up.
- `start --resume <session>` refuses a session whose lock is held, naming the
  process ("… is running in process 4466"), unless `--take-over`. A process
  that doesn't answer but holds the lock is alive and serving it; nothing has
  to be guessed.
- `--take-over` releases the session from its owner first: the owner cancels
  a running turn and closes that one session (`session/close`), never
  stopping a whole process. Sessions forked beside it keep running; a
  headless process left with none stops (ADR 16). When the owner is an
  editor, this is the experimental `close` action, refused unless the
  editor's profile enables it, and compensated for (ADR 4).
- An editor's `session/load` or `session/resume` of a session another process
  holds is answered by the host with a JSON-RPC error naming the process and
  how to release it (`brnr close <session>`), and doesn't reach the agent. A
  caveat under P1: the editor can't pass `--take-over`, so its explicit step
  is releasing the session first.
- `list`, `ps` and `doctor` can read the locks to say which process serves a
  session without asking it.

## Considered

- Refusing whenever any process doesn't answer (review item 8, A): one hung
  process would block every resume.
- Refusing only if the transcript was last written by the silent process
  (B): an inference, wrong for a process with `log = false`.
- Each process listing its sessions in its `<pid>.json`: works, but has to be
  rewritten on every change, and trusts a file a dead process left behind.
  The kernel releases a lock.
- The editor's load taking over automatically (stopping the headless owner):
  a click in the editor's history list would end a headless turn. A change of
  owner is always asked for in so many words (P11).
- `--take-over` stopping the owning process: it would take sessions forked
  beside it down too.
