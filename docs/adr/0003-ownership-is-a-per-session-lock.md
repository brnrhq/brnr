# 3. Ownership is a per-session lock

Amended by 50: a session whose lock can't be taken isn't served headless (the
start or fork fails, with the cause); an editor's process passes it through,
and `status` and `doctor` say it isn't locked.

Amended by 53: the id in the lock's file name is escaped, one name per id,
so distinct sessions never share a lock.

Amended by 63: `session resume --pid <pid>` takes the lock in a running
process, and `--take-over` closes the session where it runs first, as
without `--pid`; a session process `<pid>` serves already is refused.

Accepted 2026-10-07. Implemented. `--take-over` from an editor's process is
the experimental `close` (ADR 4): `start` launches the process that is to
resume the session first, so the owner can tell the editor which one has it.
Replaces the "refused if a running host already serves the session" check of
former decision 10; resolves review item 8.

## Context

A session must be served by one process at a time (P11): two processes on
one session append to one transcript and answer one agent session twice.

ACP has no opinion on two clients resuming one session; it is process
management. The agent copes, but the conversation splits. Claude Code chains
a session's messages by `parentUuid` in one file, so two processes that
resume it each continue from their own last message: two branches under one
session id, nothing linking them, and a later resume continues one of them.
Unlike a subagent: Claude Code keeps a subagent as a sidechain under the
parent's session id (its own `agentId`, `isSidechain: true`, in
`<session>/subagents/`), and claude-agent-acp can show it over ACP as a child
session with its own `subagentSessionId`, when the client opts in.

brnr checked by asking every running process which sessions it had. A
process that doesn't answer can't say, and `start --resume` went ahead anyway
(`Err(_) => {}` in `start`, src/ctl/talk.rs): a guess (P4). Nothing checked
an editor's `session/load` or `session/resume` at all, so an editor could open
a session a headless process was running.

## Decision

- Every process holds an exclusive `flock` on `$BRNR_DIR/sessions/<id>.lock`
  (the id sanitized as for transcripts) for each session it serves, with its
  pid and the session's id written inside. Taking a session is taking its
  lock; a process that dies releases it, with nothing to clean up: the file
  it leaves, held by nobody, is taken as it is (`doctor --fix` removes it).
  One that closes a session removes the file as it lets go.
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
- By default, an editor's `session/load` or `session/resume` of a session
  another process holds is answered by the host with a JSON-RPC error naming
  the process and how to release it (`brnr close <session>`; for a session
  another editor has open, closing it there), and doesn't reach the agent. A
  caveat under P1: the editor can't pass `--take-over`, so its explicit step
  is releasing the session first.
- The feature flag `shared_sessions` in the editor's profile (ADR 42) lets
  the load through: the agent then serves the session to both processes and
  splits the conversation. The process holding the lock stays the only
  writer of the session's transcript; the editor's process records the
  session in its host log only, and says so (P3). This is process
  management, not protocol, so strict mode (ADR 41) doesn't decide it.
- `list`, `ps`, `sessions` and `doctor` read the locks to say which process
  serves a session without asking it.

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
- Always letting the editor's load through, as two editors running the agent
  directly would: the conversation splits silently, and approvals could be
  answered from either side. It is what `shared_sessions` turns on.
- Refusing in the default mode and passing through in strict mode: what
  happens here isn't a question of following the protocol.

## Tests

Run `cargo test --release adr_0003_`. Named claims and their assertions:

- [tests/security.rs](../../tests/security.rs)
  - `adr_0003_session_locks_are_private_and_never_followed`.
- [tests/headless.rs](../../tests/headless.rs)
  - `adr_0003_an_editors_load_of_a_held_session_is_refused`.
- [tests/cli.rs](../../tests/cli.rs)
  - `adr_0003_resume_of_a_held_session_is_refused`.
  - `adr_0003_take_over_moves_a_session`.
  - `adr_0003_a_silent_process_keeps_its_session`.
  - `adr_0003_a_dead_process_lets_go`.
