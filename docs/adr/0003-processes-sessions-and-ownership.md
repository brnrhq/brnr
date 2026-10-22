# 3. Processes, sessions and ownership

Draft 2026-10-20. Implemented, but the cause of the deaths 3.7 records is
still to be found. Factor: F3; N2 for how a
process starts and dies (ADR 1). Brings together former ADRs 2, 13, 8, 7, 9,
10, 11, 12, 3, 50, 65, 42, 14 and 16 (docs/adr/README.md maps them).

How brnr runs agents: the process that owns each agent, how it is started,
how it ends, and which process may serve a session. This is what ACP leaves
out (F3); a start that commits once, and no process left without an owner,
are what N2 asks of it.

## 3.1 The editor's process: `brnr acp` relays, the host owns the agent

- `brnr acp [--profile <p>] [--strict] [-- <agent> [args...]]` is what an
  editor runs as its ACP agent (`brnr acp -- brnr-claude-adapter`). Inside
  brnr it is the proxy (`proxy.rs`, `proxy_pid` in metadata and
  transcripts).
- It starts a brnr process, the host, detached (a new session, double
  fork, reparented to init or launchd), with one end of each of two
  socketpairs, and from then on only relays:
  - On the link: stdin and stdout as frames, the agent's stderr to its
    stderr, stdin EOF after what came before it.
  - On the signal link, which the host never stops reading, so that nothing
    on it waits behind the editor's input (ADR 10.1): the signals the proxy
    catches (HUP, INT, QUIT, TERM, USR1, USR2, but those the editor
    ignores, which the agent inherits ignored), and its stdout failing.
  - At the end it exits with the agent's exact wait status, re-raising the
    signal that killed it.
- The host starts the agent in a process group of its own, with the
  editor's signal mask. The editor's requests reach the agent with ids of
  the host's (ADR 2.4); the editor's stdout ends when the agent's does
  (ADR 2.6).
- The editor's agent goes with the editor, as if the editor had run it.
  When the proxy's stdin ends, the agent's stdin is closed. When writing to
  the proxy's stdout fails, the agent's stdout is closed (its next write
  fails, EPIPE). When the proxy goes, the agent's stdin is closed and its
  process group SIGKILLed (`link_gone`). A turn still running when the
  editor closes is lost; the agent keeps its sessions, and
  `brnr session resume <session>` carries one on headless. A process's
  owner is set when it starts (P11, P14).
- By default the editor's `initialize` loses the `fs` and `terminal` client
  capabilities, one of the few deliberate changes to the stream (P1). A
  profile passes those it names, and in strict mode they pass through
  (ADR 2.1, ADR 2.7).

**Consequences.** What the editor sees differently, stated in the README:

- A SIGKILL sent to the proxy can't be passed on as such. The host sees its
  links go without the editor's stdin ending first, closes the agent's
  stdin and SIGKILLs its process group itself (`link_gone`): the same end,
  a moment later.
- The editor's child is the proxy, not the agent, so its pid and process
  tree differ. That separation is what lets the side channel exist.
- Side-channel actions, where a profile enables them: ADR 7.1.

**Why.** Other commands must reach the editor's sessions, so the agent runs
in a process the editor doesn't own, while the editor notices nothing
different (P1). `fs` and `terminal` are dropped because ACP v2 drops them
(its `ClientCapabilities` has neither), and without them no agent comes to
rely on them.

**Considered.**

- `--on-disconnect direct|headless`, and a profile's `on_disconnect`: the
  agent carrying on headless when the editor goes, the host taking over the
  editor's pending agent requests. One owner per session (P11), and a
  directly run agent goes with its editor (P1).
- Passing `fs` and `terminal` through always: by P1's test the agent is
  affected, as it can't use the editor's buffers or terminal. Kept for
  strict mode and for a profile that names them (ADR 2.7).

## 3.2 Sessions are the agent's ids, processes are pids

- `<session>` is a session's exact id, as the agent gave it, running or
  not, for everything about a session. `--pid <pid>`, or a `<pid>`
  argument, is a brnr process as `brnr process list` shows it, for what is
  about the process. No prefixes, no `--session`, no picking (P4).
  `event watch` and `event notify` take one or the other and have no
  default; `event notify --stdin`, as a bridge, takes the events its
  process gives it instead (ADR 5.8).
- Sessions have no names. `session new --json` prints the session for a
  script to keep (`s=$(brnr session new --json … | jq -r .session)`);
  shorter handles are the shell's. The agent's own title for a session
  (`session_info_update`) is shown in `session list` and `session status`
  to tell sessions apart, and is never something to type.
- `brnr process list` lists brnr's processes: pid, agent, owner (`editor`,
  `headless`, or `unreachable` for one that doesn't answer, whose sessions
  are those it holds the locks of, ADR 3.9), uptime and sessions.
- `brnr process stop <pid>` stops a process and its agent: the agent's
  stdin is closed, then SIGTERM goes to its process group after 5 s, then
  SIGKILL after 5 s more. Ending one session is `brnr session close`
  (ADR 3.12). `session` commands are about sessions, `process` commands
  about processes.
- "Host" isn't in the CLI or its help. It is the name of the process
  inside brnr.

**Why.** An argument means one thing (P4). Settings and messages belong to
a session, and picking one for the user hid that.

**Considered.**

- A `<target>` that is a pid, a process name, a session id or a unique
  prefix of one, with `--session` or brnr picking a session inside: too
  many meanings.
- Session names (`--name`, one running session per name, a name meaning
  the latest session that had it): brnr's own layer over ACP's sessions,
  and a policy to learn rather than a description of the agent's sessions
  (P2).

## 3.3 A process is started with one resolved request

- Whoever starts a process (`session new`, `session resume`, `brnr acp`)
  resolves everything first: profile, agent, cwd, role (editor or
  headless), strict mode (ADR 2.1), experimental actions and feature flags
  (ADR 7.1, ADR 3.10), mode, model, config options, MCP servers, auth
  method, prompt and attachments, timeouts, `stop_when_idle`, what to log
  (ADR 5.1), bridges, the signal mask, and the agent's and the bridges'
  commands as they are run (a bare name installed next to brnr is that
  path, ADR 11.2).
- It writes one JSON request on the process's stdin. File descriptors
  carry only the editor link and its signal link (fds 3 and 4, ADR 3.1) or
  the start channel (fd 3, ADR 3.4).
- The process reads no config and takes no flags. `brnr host` is internal,
  not run by hand; `session new --foreground` runs a session in a terminal
  (ADR 3.5).
- The host log's `process_started` record holds the request
  (`start_request`), secrets redacted (ADR 9.1): every process records
  what it was asked to do.
- A config error under `brnr acp` is reported on the editor's stderr as
  `brnr acp: …`, as the process's own failures are, and exits 1 (ADR 8.3).

**Consequences.** There is one way to get a prompt to a process. The
agent's command isn't in the process's argv, so killing processes by the
adapter's name (`pkill -f brnr-claude-adapter`) doesn't take the host with
it (ADR 3.7).

**Why.** Config is resolved in one place, and the process gets everything
at launch (P10).

**Considered.**

- One request that names the profile, the process loading it: config would
  be resolved in two places.
- `brnr host` run by hand, with flags of its own: a second interface, whose
  flags lagged the start's.

## 3.4 A start is atomic and commits at ready

- `session new` and `session resume` read all of their input before they
  launch the process: `--prompt` (from their own stdin with `-`), `--file`,
  `--image`; and they resolve the rest of the start (ADR 3.3).
- The process is launched with a pipe as its stdin and the start channel,
  a socketpair, as fd 3. The command writes one start request on the pipe
  and closes it. The process reads its stdin to EOF before it does anything
  else; cut short (the command died mid-write), it refuses to start.
- The host reads the start channel on a thread, as every other source. EOF
  on it before the commit means the command has gone: the process stops at
  once, whatever it was doing (`start_abandoned`).
- The commit is the ready report, `{pid, session, message}`, written on
  the start channel once the session is open and its mode and config
  options are set, and before the agent gets the prompt (`message` is the
  prompt's `m<n>`, ADR 4.1). Until then anything failing (a setup step, the
  timeout, the command going away) stops the process, and the prompt is
  never sent; nor is the next step when the agent answers one after that.
  After it the process sends the prompt, and the session runs on its own
  whatever the command does (P14).
- The commit is the report's write succeeding. Nothing else is written on
  the channel before it, so the event loop writes the report itself, in one
  write that doesn't wait. A write that fails (the command has gone, its
  EOF perhaps not read yet) abandons the start as EOF does. A write that
  succeeds is all the process can know: the command may still go before it
  reads the report, and the session then runs on, in `session list` and its
  logs.
- Committed, the start channel becomes a peer, subscribed (with `--wait`)
  to the turn's events before the prompt goes, so it misses none.
- Without `--wait`, the command prints the session and its process and
  exits after the report (`--json`: `{session, pid, message}`, `message`
  null without a prompt). With `--wait`, it reads the turn's events on the
  same channel until the `turn_ended` that carries its message (ADR 8.3).
  There is no reconnect and no race.
- The start timeout (`BRNR_START_TIMEOUT`, 120 s) goes in the request and
  is the process's: it fails the start when the time passes. The command
  keeps only a fallback timer, the timeout plus 10 s, for a process stuck
  too badly to report.
- A session opened in a running process (`--pid`, ADR 3.12) commits the
  same way: the commit is the process's answer on the control socket,
  queued while the command is still connected. The command going before
  it, a setting failing, or the start timeout passing closes the session
  again (for an agent that can't close sessions, the error names it), and
  the prompt is never sent. The process stopping meanwhile fails the
  opening too, and the session ends with the process. The timeout goes in
  the socket request and is the process's; the command's fallback is 10 s
  later.

```text
 session new                        brnr process                     agent
  read prompt, files, images
  launch: stdin = request,
  fd 3 = start channel ────────────▶ read request to EOF
                                     bind socket, spawn agent ─────────▶
                                     initialize, session/new,
                                     set_mode, set_config_option ───────▶
  read ready ◀────────────────────── ready {pid, session, message} ← commit
  (no --wait: exit)                  session/prompt ────────────────────▶
  --wait: read events ◀───────────── … turn_ended
```

**Consequences.** The README's promise holds for the whole start: if the
command gives up, or is interrupted, before the commit, the process stops
too and the prompt is never sent. No idle clock waits for a prompt
(ADR 3.8).

**Why.** A start made of two conversations, a ready pipe and then the
control socket for the prompt, had a gap: a command killed there left a
session with no prompt, which ran forever. One conversation that commits at
ready closes it (P10, P14).

**Considered.**

- The prompt on the control socket, with the start deadline running until
  it arrives: a session nobody used for up to 120 s. Keeping the ready
  channel open until the prompt arrives: still two conversations.
- Idle time counting from the session opening: it helps only with
  `--stop-when-idle`, and closes at once a session whose prompt is slow.
- The prompt on the process's command line: Linux caps one argument at
  128 KiB and macOS all of them at 1 MiB, and any user can read arguments
  with `ps` (P13).
- Committing at launch: a killed command would leave a running session
  whose id nobody was told, and Ctrl-C couldn't cancel a start.

## 3.5 A headless session in the foreground

- `session new --foreground [--quiet] [--json]`, and `session resume`'s,
  runs the same process a start does, as its child, in a process group of
  its own, and passes it the signals it gets: Ctrl-C once stops the
  session, twice kills the agent.
- It shows the session's events on stdout as they come (`--json`: as JSON
  lines; `--quiet`: not at all).
- It exits as the agent did (128 + the signal that killed it), but never 0
  if the start failed: 1 where the agent then exited 0 (setting `--mode`
  failed, say), 127 or 126 where the agent couldn't be run, as a shell has
  it.
- `--wait` and `--foreground` don't go together: the foreground shows the
  whole session.
- stdout closed (`brnr session new --foreground … | head -1`): the display
  stops, with a note in the host log, and the session carries on. A
  supervisor that means "stop" stops the process.
- A slow reader: the display runs on a thread of its own, with a bounded
  buffer, then skips events, saying how many (ADR 10.1). It never blocks
  the host.
- stderr carries brnr's own messages and the agent's stderr (ADR 3.6).

**Why.** A foreground session is for a supervisor (systemd, a container) or
a terminal, with the same flags as a detached start. The transcript has
everything; a failed display shouldn't cost the session.

**Considered.**

- stdout closing stopping the session, as SIGPIPE ends the writer of a
  pipeline.
- An unbounded buffer, or printing on the event loop: a broken pipe there
  panicked the host and left no `process_exited` record, and stale `.sock` and
  `.json` files.

## 3.6 The agent's stderr

- Always: in the host log, a record for each line, or for each 64 KiB of a
  longer one (ADR 10.3). The line still coming at the end (the agent's
  stderr ends, or the host exits) gets one too.
- With an editor: through the proxy to the editor's stderr, unchanged
  (P1).
- In the foreground: to the foreground's stderr, unchanged, as running the
  agent directly would; stdout stays the events.
- To the editor and the foreground it goes as it is read, newline or not:
  each read, up to 64 KiB, every byte once and in order (ADR 2.6). What
  keeps it as lines does so alongside, without holding anything back.
- A failed headless start, detached or in the foreground, ends its error
  with the agent's last stderr lines, from a ring buffer of 20, ending with
  the line the agent is in the middle of (P3). A long line keeps its first
  2000 bytes (ADR 10.3).

**Why.** "claude CLI not found" has to reach the user who started the
session, not only the host log. A supervisor's log collects the agent's
diagnostics; at a terminal `2>/dev/null` hides them. The adapters are
chatty (claude-agent-acp writes `[session/create] … phase=…` lines for
every session).

**Considered.**

- The foreground showing it only when the start fails: a supervisor's log
  would lose the agent's diagnostics.
- Behind a flag.

## 3.7 A process's own death is recorded

- On any agent exit, including a crash during setup or a turn, the host
  SIGKILLs what remains in the agent's process group before it reaps the
  agent (P14). Until then its pid and group id can't be reused. Cleanup
  doesn't depend on a stop request having come first.
- A detached process's stderr goes to its host log (`process_stderr`), a line
  or a piece of 64 KiB at a time (ADR 10.3). With `log = "off"` there is no
  host log, and it goes to `/dev/null`.
- A panic, on any thread, is recorded. The hook notes it and wakes the
  event loop, which records the panic in the host log (`process_panicked`),
  writes `process_exited` (with the `error`) there and in every session's file, sends it to the
  peers it can still reach, removes the `.sock` and `.json` files (P3), and
  SIGKILLs the agent's process group (P14). Bridges get the time to act on
  `process_exited` they get on any exit (ADR 5.7).
- A panic once the agent is spawned and before the event loop runs is
  recorded, and ends the agent the same way, as far as what the start has
  made allows (its log, bridges, the display; no sessions yet). Whoever
  started the process is told on fd 3, as of any start that fails:
  `session new` fails with the reason; an editor's `brnr acp` says it on
  stderr and exits 101.
- A panic prints a link to report it (ADR 11.5): in the foreground from the
  hook, and from `session new` or `brnr acp` when it fails a start.
- A start that fails as it opens the start channel or its bridges records
  `process_exited` too. The logger never skips `process_exited` (ADR 10.1), so a host log
  without it is a death nothing recorded. What can't be recorded: a panic
  on the logger's own thread, and an abort (a stack overflow), which runs
  no hook.
- `brnr doctor` lists host logs without `process_exited` whose process isn't
  running (how many, and the latest three), with when each last wrote and
  the sessions it had open.
- Whether metadata in the runtime directory is a gone process's is told by
  its socket, not its pid, which may be another process's by then (a
  reboot, pids wrapping around). It is gone if its pid isn't a live process
  of the user's, or its socket refuses a connection twice (nobody listens,
  or it isn't there). macOS also refuses a stopped process's socket once
  its backlog is full, so there a refusal counts only if the process with
  that pid started after the file was written.
- `doctor --fix` removes what gone processes left behind. `session list`,
  `process list`, `session status`, `session resume` and every other
  command that looks processes up drop them, removing their metadata and
  socket.
- The agent's command isn't in the process's argv (ADR 3.3).

**Consequences.**

- A killed host can't write `process_exited`: `doctor` reports its incomplete log
  and repairs stale runtime files, and the session's lock can be taken
  again (ADR 3.9). A killed proxy leaves the host alive to record its death
  and stop even a blocked agent. A killed bridge leaves the session and its
  ownership intact (ADR 5.7). Setup faults never commit the prompt
  (ADR 3.4). `tests/chaos.rs` injects these.
- The host is the only supervisor. SIGKILL closes its pipes but can't make
  it signal its children: a blocked agent, a bridge that ignores EOF, or
  their descendants can outlive it. The host-SIGKILL test checks a flooding
  agent that exits on its broken pipe, not cleanup in every case. That
  stronger promise needs a separate supervisor, and a decision about its
  own lifetime. `doctor` doesn't kill processes inferred from stale pids.

**Why.** In October 2026 an editor's host, agent and proxy died together
three times, with no `exited`, no `editor-disconnected` and no `signal`
record (the names of then): SIGKILLed, or a panic, which a detached host's `/dev/null` stderr
hid. One unconfirmed explanation fits all three: something killing
processes by command line (`pkill -9 -f brnr-claude-adapter`), which
matched the host's argv. Still to find out: reproduce with IntelliJ
restarting its agents (the first of ADR 7.1's editor trials), watching with
`ps` who sends SIGKILL; whether the proxy was killed by a signal or lost its
process (exit 1); and whether it happens without another run alongside.

## 3.8 Stopping when idle

- `stop_when_idle = <seconds>` in a profile's headless part, or
  `--stop-when-idle <s>` on `session new` and `session resume` (with
  `--foreground` too): a session idle that long closes (`session/close`),
  and the process stops with its last session. The last one isn't closed
  first; the process just stops.
- A session is idle while no turn is running, nothing is held and no
  approval is waiting. A message or a prompt starts the count again. It
  counts from the start too, so a start without a prompt doesn't run
  forever; a start with a prompt sends it at the commit (ADR 3.4), so the
  session is busy at once. `0` is as soon as it is idle.
- Never while an editor is attached.
- Closing emits `session_closed` with `by: idle` (ADR 5.2). The last
  session, stopping the process, gets `process_exited` instead, which isn't also
  `session_closed`.
- With `stop_when_idle` set and an agent that can't close sessions,
  `session fork`, `session new --pid` and `session resume --pid` are
  refused (P7): a second session could never close when idle, and the
  process would never stop.
- A session being opened in the process counts as one of its sessions: the
  process doesn't stop as its last other session closes or goes idle
  meanwhile; that one is closed instead.

**Why.** A headless session started for one task should end when the task
is done, without anyone having to stop it.

**Considered.**

- A switch (`stop_when_idle = true`): seconds replace it, and `0` is what
  it did.
- Not counting before the first turn: a start whose prompt never came ran
  forever.
- Keeping, silently, a session that ran out while the process has others,
  when the agent can't close sessions: lost without a word (P3).

## 3.9 Ownership is a per-session lock

The lock:

- Every process holds an exclusive `flock` on `sessions/<file_name>.lock`
  in the state directory for each session it serves. `file_name` is the id
  escaped as `paths::file_name` does (ADR 9.2), so distinct sessions never
  share a lock. Inside are the holder's pid, the session's id and the
  holder's runtime directory, absolute. Taking a session is taking its
  lock; whether a lock is held is the kernel's answer, not the file's.
- A process that dies releases its locks, with nothing to clean up: the
  file it leaves, held by nobody, is taken as it is. `doctor` lists such
  files under `session locks`, and `--fix` removes them. A process that
  closes a session removes the file as it lets go.
- `log = "off"` writes no transcripts, but its sessions are locked all the
  same: the lock is ownership, not logging.
- `sessions/` is held to the runtime directory's terms (P13): created
  0700, refused with the cause when it isn't the user's own private
  directory or is a symlink. Each lock is 0600 and opened `O_NOFOLLOW`. It
  isn't repaired as transcripts are (ADR 9.3).
- `session list`, `process list` and `doctor` read the locks to say which
  process serves a session without asking it.

A session another process holds:

- `session resume <session>` refuses it, saying where it is in the words
  every command uses for it: `sess-1 is running in process 4466`, with `,
  which is not answering` for a holder that doesn't answer and `, which brnr
  doesn't list` for one not listed here, or `sess-1 is opening in process
  4466` for one that hasn't opened it yet. A process that doesn't answer but
  holds the lock is alive and serving the session; nothing is guessed. The
  lock is taken before the agent is asked.
- `--take-over` is offered only where it can work: for a session open in
  the process holding its lock, whose refusal adds `(--take-over closes it
  there and resumes it here)`, or `… resumes it in process 4470` with
  `--pid`. A holder that doesn't answer, is opening the session or isn't
  listed here can't be asked to close it, and a session a process serves
  without its lock isn't taken over: their refusals don't offer it, and
  with `--take-over` they are refused in the same words.
- With `--pid`, a session that process serves or holds already is refused
  (`sess-1 is already open in process 4466`), `--take-over` or not; one
  closing there is closing (ADR 5.5).
- `--take-over` releases the session from its owner first: the owner
  cancels a running turn and closes that one session (`session/close`),
  never stopping a whole process. Sessions forked beside it keep running; a
  headless process left with none stops (ADR 3.12). With `--pid`, it is
  refused before anything is closed if that process would refuse the
  resume (ADR 8.1).
- When the owner is an editor, `--take-over` is the experimental `close`
  action, refused unless the editor's profile enables it, and compensated
  for (ADR 7.1). `session resume` first launches the process that is to
  resume the session, so the owner can tell the editor which one has it.
- An editor's `session/load`, `session/resume` or `session/prompt` of a
  session another process holds, and not open in the editor's process, is
  answered by the host, by default, with a JSON-RPC error naming the
  process and how to release it (`brnr session close <session>`; for a
  session another editor has open, closing it there). It doesn't reach the
  agent. A prompt opens a session as a load does (ADR 5.6), so it is held
  to the same lock (#331). A caveat under P1: the editor can't pass
  `--take-over`, so its explicit step is releasing the session first.
- A load or resume the agent answers with success after the session closed
  in the editor's process (the editor's `session/close`, or brnr's close or
  take-over under it, while the request was unanswered) is checked again as
  the answer comes: another process can take the lock between the close and
  the answer (#357). One it took isn't served there: the session doesn't
  open, has no transcript there and no replay kept, and its lock isn't
  held; the agent is asked to close it (`session/close`); the editor gets
  the agent's answer, and is told in the session, as of a take-over
  ("Session held by brnr process 4466", with how to release it, as above),
  its later requests for it answered as ADR 7.1 has them. The host log has `open_failed`,
  saying too if the agent can't close sessions, and another if the agent
  fails the close: the session is left open in the agent (P3).
- The feature flag `shared_sessions` in the editor's profile (ADR 3.10)
  lets the load, or the prompt, through, and one answered after a close as
  above opens shared. The agent then serves the session to both processes
  and splits the conversation. The process holding the lock stays the only
  writer of the session's transcript; the editor's process records the
  session in its host log only, and says so (P3).
  Strict mode (ADR 2.1) doesn't decide this.

A session that can't be locked (the lock is neither held nor free:
`sessions/` isn't a private directory, something other than a file is where
the lock goes, the process is out of file descriptors, the file is locked
by a process that doesn't say which, the file system has no working
`flock`):

- A headless process never serves it. A start whose session (new, resumed,
  loaded) can't be locked fails before it commits (ADR 3.4): the command
  exits 1 with the cause, naming the lock file and the system's error ("the
  agent opened sess-1, which can't be locked: …/sessions/sess-1.lock: Is a
  directory"), no prompt reaches the agent, and the process and its agent
  stop. A resume's lock is taken before the agent is asked, so the agent
  never hears of it. A fork the agent opens into a session that can't be
  locked is refused as one another process holds, and its source goes on.
  In a running process (`--pid`), a new session that can't be locked is
  closed again, and a resume of one isn't sent.
- An editor's process passes an editor's session it couldn't lock through:
  `session/new`, `session/load` and `session/resume` reach the agent and
  are answered as without brnr, and the process keeps the session's
  transcript.
- It is said where it can be seen: `session status` has `lock_error`, the
  cause, and its text says "not locked: …; nothing stops another process
  serving it too"; the host log has `lock_failed`. `session resume` of it
  still refuses, as the process serving it says so when asked.
- `brnr doctor` fails a `sessions/` that isn't a private directory, and
  anything but a file where a lock goes, saying that the session can't be
  locked or started headless and what to remove. It doesn't take such a
  thing for a lock a gone process left; `--fix` leaves it to the user.

A holder in another runtime directory:

- A command whose runtime directory isn't the lock holder's can't reach
  the holder, and says so rather than guess (P4) or say nothing (P3).
  `session resume` is refused with `sess-1 is running in process 4466 with
  runtime directory /run/user/1000/brnr (BRNR_DIR=/run/user/1000/brnr
  reaches it)`, with `--take-over` as without: it can't ask a process it
  can't reach to close. `session list` shows the session `unreachable`,
  with that pid, its agent and cwd unknown here, not `inactive`. Commands
  on the session (`session close`, `session delete`, `prompt send`, …) say
  the same.
- An editor's load of such a session is answered with the process and
  `BRNR_DIR=… brnr session close <id>` to release it, or, if an editor's
  process holds it there, with that process and its runtime directory, to
  close it in that editor.
- The directory after `BRNR_DIR=` is one shell word: in single quotes when
  it has a space, a quote or another character a shell reads, each quote
  in it as `'\''`, so that what is shown runs as it is shown.

Where things live, fixed for 1.x and kept within the major version from
1.0.0 (P9, ADR 8.4):

- the runtime directory: `$BRNR_DIR`, else `$XDG_RUNTIME_DIR/brnr`, else
  `$TMPDIR/brnr-<uid>`. It holds each process's control socket and
  metadata;
- the state directory: `$BRNR_HOME`, else `~/.brnr`, laid out as
  `projects/<project_key>/<file_name>.jsonl` and `<file_name>.acp.jsonl`,
  `hosts/<host id>.jsonl` and `sessions/<file_name>.lock`;
- `file_name` (ADR 9.2): lowercase ASCII letters, digits, `-` and `_` stay;
  every other byte of the id's UTF-8 is `%` and two lowercase hex digits;
  the empty id is `%`;
- `project_key` (ADR 9.2): `<claude>-<hash>`, `<claude>` the session's cwd
  with every character but ASCII letters and digits replaced by `-`, as
  `~/.claude/projects` has it, and `<hash>` the low 32 bits of FNV-1a 64
  over the cwd's bytes, as 8 lowercase hex digits; the cwd as given, its
  trailing `/` removed.

What host logs and lock files contain, and what is in the runtime
directory, aren't covered: a lock's fields are for brnr's own commands and
can change in any release. What a transcript's records contain, and how a
reader tells formats apart, is the rest of #70, not this section's.

**Consequences.**

- A lock left in `$BRNR_DIR/sessions/` by an earlier version is read by
  nothing: remove the directory. No shim (P9).
- A lock file outlives a reboot: a file nobody holds, taken as it is.
- On a file system where `flock` doesn't work (some network file systems)
  the lock can't be taken, and a start is refused with the cause, not
  served as if free.
- A state directory two machines share: they contend for one lock only
  where the file system carries `flock` between them, and a holder's pid
  means nothing on the other machine. A gap in the threat model.
- An editor's session that can't be locked can be served by a headless
  process whose lock succeeds (a transient error, or the editor's process
  not answering when `session resume` asks): a caveat of the proxy role,
  which `session status` and `doctor` make visible.

**Why.** A session must be served by one process at a time (P11): two
processes on one session append to one transcript and answer one agent
session twice. ACP has no opinion on two clients resuming one session; the
agent copes, but the conversation splits. Claude Code chains a session's
messages by `parentUuid` in one file, so two processes that resume it each
go on from their own last message: two branches under one id, nothing
linking them, and a later resume continues one of them. Asking the running
processes can't settle it, since one that doesn't answer can't say (P4);
the kernel releases a lock when its holder dies. A lock that can't be taken
leaves brnr unsure, so by P4 it refuses in the client role and passes the
session through in the proxy role, and says so; a `session/new` can't be
refused anyway, as the agent has opened the session by the time brnr knows
its id. The lock guards the transcript, so it lives beside the transcripts:
a lock in the runtime directory guarded it only for processes that found
the same one, which an interactive shell (with `XDG_RUNTIME_DIR`) and a
cron job or an `ssh` command (without) don't. It is found from the session
id alone, which a resume has before it knows the cwd. Sockets and metadata
are a process's and go with the machine's run, so they stay in the runtime
directory. `sessions/` isn't repaired: nothing of the user's is in it, and
in a directory others can write to, another user can put a file where a
lock goes.

**Considered.**

- Refusing whenever any process doesn't answer: one hung process would
  block every resume.
- Refusing only if the transcript was last written by the silent process,
  or by a process not listed here: an inference, wrong for `log = "off"`.
- Each process listing its sessions in its `<pid>.json`: rewritten on
  every change, and trusting a file a dead process left behind.
- The editor's load taking over automatically: a click in the editor's
  history list would end a headless turn. A change of owner is always
  asked for in so many words (P11).
- `--take-over` stopping the owning process: it would take sessions forked
  beside it down too.
- Always letting the editor's load through, as two editors running the
  agent directly would: the conversation splits silently, and approvals
  could be answered from either side. It is what `shared_sessions` turns
  on.
- Refusing in the default mode and passing through in strict mode: this
  isn't a question of following the protocol.
- Serving a session that can't be locked and saying so in the host log:
  the host log is the record, not the telling (P3), and the session could
  have two owners (P11).
- Refusing an editor's `session/load` and `session/resume` of a session
  that can't be locked: brnr can't be sure another process holds it, and in
  the proxy role it doesn't refuse on a guess (P4). It would also treat a
  load unlike a new session, which can't be refused.
- `doctor --fix` removing what is in a lock's place: a directory may hold
  the user's files, and a symlink's target is the user's to judge.
- A lock in both directories: two locks are the problem.
- The lock next to the transcript, in the project folder: the lock is
  taken before the cwd is known (a resume of a session only the agent
  knows has none), and one id resumed in several folders has a transcript
  in each (ADR 9.2) and is one session.
- One directory for everything: the runtime directory exists for the
  socket's path length (`sun_path`), and because what is in it is a run's.
- Repairing an open `sessions/` as ADR 9.3 repairs transcripts: see Why.

## 3.10 Feature flags for process management

- `features = [...]` in a profile part names the process-management
  behaviours it turns on. Each is off by default; an unknown name fails to
  load (P7). There is one:
  - `shared_sessions`, in `[profiles.<p>.editor]`: an editor may load or
    resume a session another brnr process owns (ADR 3.9).
- Strict mode doesn't change them.

**Why.** Some choices aren't protocol but how brnr's processes share
sessions. They need a default that protects the user and a way to choose
otherwise, kept apart from strict mode, which is about the protocol
(ADR 2.1), and from experimental actions, which act on an editor's session
(ADR 7.1).

**Considered.**

- Deciding them by strict mode: they aren't protocol.
- Making them experimental actions: they don't add to an editor's session;
  they decide what brnr's own processes may do.

## 3.11 Resuming

- `brnr session resume <session>`, with the session's exact id (ADR 3.2).
  `--take-over` is on `session resume` only.
- brnr sends `session/resume` when the agent offers it (no history
  replay), else `session/load`. `session/load` is ACP v1's; v2 drops it.
- While a load replays history the transcript has, the host neither
  records nor emits the replayed updates. What they say the session is now
  (its title, mode, config options and commands) is kept. A load of a
  session brnr has no transcript of records the replay, marked `replayed`,
  and a `history` event says which it was (ADR 5.6).
- An id brnr has a transcript of brings its cwd, agent and profile from it,
  the session's own record (P4); `--cwd`, `--profile` or `-- <agent>`
  override them. Of an id with files in more than one project folder, they
  come from the newest record of them all, where the session was last served
  (ADR 5.1, #121). An id brnr doesn't
  know (one `session list` with an agent named shows) goes to the agent as
  given, in `--cwd` (or the profile's `cwd`, or here), with the agent after
  `--` (or the profile's).
- A session another process holds is refused, unless `--take-over`
  (ADR 3.9).
- With `--pid`, the session is resumed in that running process, in its
  recorded cwd (ADR 3.12).
- It appends to the session's file in its cwd's folder: the same file,
  unless `--cwd` names another cwd, where it starts one. Either way it is
  one session's transcript (ADR 5.1).

**Why.** Sessions are the agent's (P2): any id the agent knows can be
resumed, and brnr's own record of one is used where it has one, as a fixed
rule rather than a guess (P4).

**Considered.**

- Refusing an id brnr has no transcript of: `session list` would show
  sessions that `session resume` then refused.
- For an id with files in two folders, the cwd of the file in the folder
  the command runs in: a rule that depends on where it runs (P4). The
  options are in ADR 5.1.

## 3.12 Fork and close

- `brnr session fork <session>` forks the session (`session/fork`) into a
  new one in the same process and prints its id. It is refused on an
  editor's process (ADR 7.1). `session/fork` is unstable in ACP v1: a
  convention both adapters implement, used by default and refused in strict
  mode (ADR 2.1).
- `session new --pid <pid>` and `session resume --pid <pid>` open a
  session in a running process. They are refused where `session fork` is
  (an editor's process; `stop_when_idle` with an agent that can't close
  sessions, ADR 3.8), but not in strict mode: `session/new` and
  `session/resume` are stable ACP.
- `brnr session close <session>` cancels a running turn and closes that
  one session (`session/close`); the sessions beside it in the process keep
  running. A headless process with no session left stops, unless one is
  being opened. On an editor's session it is the experimental `close`
  (ADR 7.1). `session resume --take-over` closes the same way (ADR 3.9).
- Messages still held in a closed session are reported dropped, then
  `session_closed` (ADR 5.2).
- Both are refused when the agent can't do them (no `fork` or `close`
  session capability) (P7), and `session fork` when `stop_when_idle` is set
  and the agent can't close sessions (ADR 3.8).
- A fork's source is recorded and shown. The `session_opened` record that
  starts the fork's transcript, and the host log's, name the session it was
  forked from, `forked_from` (ADR 5.1). `session list` (a FORKED FROM
  column) and `session status` (`forked_from`) say it, in text and `--json`
  alike (P5), null where there is none. For a session open in a process,
  the process says it if it opened the session as a fork, else the
  transcript's first record does (a fork resumed since); for one that has
  ended, the transcript's first record.
- A fork an editor makes through `brnr acp` (`session/fork`) opens in its
  process as `session fork`'s does: as the agent answers, in the cwd the
  request names, with `forked_from`, and the request in its source's
  transcript and the fork's.

**Why.** A process serves several sessions of one agent, and ending one
must not end the others (P11).

**Considered.**

- `session fork` and `session close` refused outright while an editor is
  attached: closing is possible with compensation (ADR 7.1); forking stays
  refused.
- Closing by stopping the whole process: it would take sibling sessions
  with it.

## Tests

Run `cargo test --release adr_0003_`. Named claims, by section:

- 3.1:
  - [tests/security.rs](../../tests/security.rs):
    `adr_0003_acp_passes_bytes_unchanged`,
    `adr_0003_acp_passes_stderr_and_the_exit_status`.
  - [tests/headless.rs](../../tests/headless.rs):
    `adr_0003_acp_is_what_an_editor_runs`,
    `adr_0003_editor_gone_takes_the_agents_children`,
    `adr_0003_a_signal_doesnt_wait_behind_a_stalled_agents_stdin`,
    `adr_0003_an_editors_process_needs_its_signal_link`,
    `adr_0003_non_blocking_stdin_is_waited_on`,
    `adr_0003_acp_with_sigchld_ignored`.
- 3.2: [tests/cli.rs](../../tests/cli.rs):
  `adr_0003_ps_lists_the_processes`,
  `adr_0003_session_targets_are_exact_ids_not_prefixes_or_pids`.
- 3.3: [tests/headless.rs](../../tests/headless.rs):
  `adr_0003_start_hands_over_one_request`,
  `adr_0003_host_is_not_run_by_hand`,
  `adr_0003_a_bad_request_is_refused`,
  `adr_0003_prompt_is_not_on_the_command_line`,
  `adr_0003_a_start_with_sigchld_ignored`,
  `adr_0003_acp_reports_a_config_error`,
  `adr_0003_acp_hands_over_one_request`.
- 3.4:
  - [tests/headless.rs](../../tests/headless.rs):
    `adr_0003_abandoned_start_sends_no_prompt`,
    `adr_0003_start_gone_as_ready_is_written_sends_no_prompt`,
    `adr_0003_start_gone_after_ready_leaves_the_session_running`,
    `adr_0003_start_timeout_stops_the_host`,
    `adr_0003_empty_prompt_is_refused`,
    `adr_0003_large_prompt_from_stdin`.
  - [tests/chaos.rs](../../tests/chaos.rs):
    `adr_0003_seeded_setup_failure_never_commits_a_prompt`.
  - [src/host/fuzz.rs](../../src/host/fuzz.rs):
    `adr_0003_a_step_answered_after_the_start_failed_goes_no_further`.
- 3.5:
  - [tests/headless.rs](../../tests/headless.rs):
    `adr_0003_foreground_start_failure_is_reported`,
    `adr_0003_foreground_close_of_the_last_session`,
    `adr_0003_foreground_outlives_its_stdout`,
    `adr_0003_slow_foreground_reader_is_told_what_it_missed`,
    `adr_0003_foreground_with_sigchld_ignored`.
  - [tests/cli.rs](../../tests/cli.rs):
    `adr_0003_foreground_start_shows_the_session`.
- 3.6: [tests/headless.rs](../../tests/headless.rs):
  `adr_0003_foreground_passes_the_agents_stderr`,
  `adr_0003_failed_start_shows_the_agents_stderr`.
- 3.7:
  - [tests/headless.rs](../../tests/headless.rs):
    `adr_0003_a_panic_is_recorded`,
    `adr_0003_a_panic_while_starting_is_recorded`.
  - [src/log.rs](../../src/log.rs):
    `adr_0003_how_the_process_ended_is_never_skipped`.
  - [tests/chaos.rs](../../tests/chaos.rs):
    `adr_0003_seeded_agent_crash_records_death_and_releases_session`,
    `adr_0003_killed_proxy_mid_fault_stops_host_and_agent`.
- 3.8: [tests/cli.rs](../../tests/cli.rs):
  `adr_0003_fork_is_refused_when_it_could_never_close`,
  `adr_0003_stop_when_idle`,
  `adr_0003_a_closing_session_doesnt_wake_the_host`.
- 3.9:
  - [tests/security.rs](../../tests/security.rs):
    `adr_0003_session_locks_are_private_and_never_followed`; and, not
    named for a claim, `what_brnr_makes_is_private_whatever_the_umask`.
  - [tests/headless.rs](../../tests/headless.rs):
    `adr_0003_an_editors_load_of_a_held_session_is_refused`,
    `adr_0003_an_editors_prompt_of_a_held_session_is_refused`,
    `adr_0003_an_editors_session_that_cant_be_locked_is_passed_through`,
    `adr_0003_an_editors_load_held_from_another_runtime_dir_is_refused`,
    `adr_0003_the_release_command_runs_as_shown`,
    `adr_0003_an_editors_load_held_by_an_editor_elsewhere_names_its_runtime_dir`.
  - [tests/cli.rs](../../tests/cli.rs):
    `adr_0003_resume_of_a_held_session_is_refused`,
    `adr_0003_take_over_moves_a_session`,
    `adr_0003_a_silent_process_keeps_its_session`,
    `adr_0003_a_dead_process_lets_go`,
    `adr_0003_a_new_session_that_cant_be_locked_isnt_started`,
    `adr_0003_a_resume_that_cant_be_locked_isnt_started`,
    `adr_0003_a_fork_that_cant_be_owned_is_refused`,
    `adr_0003_a_second_owner_of_a_new_session_isnt_started`,
    `adr_0003_a_session_is_owned_once_however_the_runtime_dir_is_found`,
    `adr_0003_the_brnr_dir_said_is_one_shell_word`.
  - [tests/doctor.rs](../../tests/doctor.rs):
    `adr_0003_what_keeps_a_session_from_being_locked_fails`; and, not
    named for a claim, `stale_session_locks_are_removed`.
  - [tests/chaos.rs](../../tests/chaos.rs):
    `adr_0003_killed_host_mid_flood_is_diagnosed_and_releases_session`.
  - [src/paths.rs](../../src/paths.rs):
    `adr_0003_brnr_dir_is_one_shell_word`.
  - [src/ctl/ownership.rs](../../src/ctl/ownership.rs):
    `adr_0003_each_place_is_refused_in_the_same_words`,
    `adr_0003_the_lock_decides_and_closing_comes_before_opening`,
    `adr_0003_a_resume_into_its_own_process_is_refused`.
- 3.10: [tests/headless.rs](../../tests/headless.rs):
  `adr_0003_shared_sessions_let_an_editor_load_a_held_session`.
- 3.11: [tests/cli.rs](../../tests/cli.rs):
  `adr_0003_resume_a_session_only_the_agent_knows`,
  `adr_0003_resume_continues_a_session`,
  `adr_0003_resume_by_loading_keeps_the_replay_out_of_the_transcript`,
  `adr_0003_resume_takes_the_newest_records_cwd_agent_and_profile` (an id
  with files in two folders, #121). That a resume in another cwd starts a
  file in that folder is shown, for a bridge's resume, by ADR 5.6's
  `adr_0005_a_bridges_load_finds_the_transcript_in_another_folder`, and
  for a start's by ADR 5.1's
  `adr_0005_log_reads_a_sessions_files_in_every_folder_as_one`
  ([tests/cli.rs](../../tests/cli.rs)).
- 3.12:
  - [tests/cli.rs](../../tests/cli.rs):
    `adr_0003_fork_and_close`,
    `adr_0003_close_cancels_the_turn_first`,
    `adr_0003_a_fork_keeps_its_source`.
  - [tests/headless.rs](../../tests/headless.rs):
    `adr_0003_an_editors_fork_keeps_its_source`.
