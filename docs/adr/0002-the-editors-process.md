# 2. The editor's process: `brnr acp` relays, the host owns the agent

Accepted (former decisions 32 and 40); reviewed 2026-10-07. Implemented.

Amended by 62: the editor's stdout ends when the agent's does, while the
agent runs on, and brnr writes nothing more there of its own.

## Context

The editor runs brnr as its agent. brnr needs the agent in a process the
editor doesn't own, so that other commands can reach its sessions, while the
editor notices nothing different (P1).

## Decision

- `brnr acp [--profile <p>] [--strict] [-- <agent> [args...]]` is the proxy.
  It starts a brnr process (the host) detached (a new session, double fork,
  reparented to init or launchd), with one end of each of two socketpairs,
  and from then on only relays. On the link: stdin and stdout as frames, the
  agent's stderr to its stderr, stdin EOF after what came before it. On the
  signal link, which the host never stops reading, so that nothing on it
  waits behind the editor's input (ADR 6): the signals it catches (HUP, INT,
  QUIT, TERM, USR1, USR2, but those the editor ignores, which the agent
  inherits ignored), and its stdout failing. At the end it exits with the
  agent's exact wait status, re-raising the signal that killed it. The host
  starts the agent in a process group of its own, with the editor's signal
  mask.
- It is named for what it is to the editor, an ACP agent command
  (`brnr acp -- brnr-claude-adapter`). It was `brnr proxy`; that name is gone,
  not kept as an alias (P9). Inside, it is still the proxy (`proxy.rs`,
  `proxy_pid` in metadata and transcripts).
- The editor's agent goes with the editor, as if the editor had run it. When
  the proxy's stdin ends, the agent's stdin is closed; when writing to the
  proxy's stdout fails, the agent's stdout is closed (its next write fails,
  EPIPE); and when the proxy goes, the agent's stdin is closed and its
  process group SIGKILLed (`link_gone`). To carry on with a session headless,
  `brnr start --resume <session>`; the agents keep their sessions. Lost: a
  turn still running when the editor closes. A process's owner is set when
  it starts (P11, P14).
- By default the editor's `initialize` loses the `fs` and `terminal` client
  capabilities, one of the few deliberate changes to the stream (P1). ACP v2
  drops them (its `ClientCapabilities` has neither), they add nothing an
  agent needs, and without them now no agent comes to rely on them. In strict
  mode they pass through, as stable v1 has them (ADR 41).

## Caveats

What the editor sees differently, stated in the README:

- A SIGKILL sent to the proxy can't be passed on as such. The host sees its
  links go without the editor's stdin ending first, closes the agent's stdin
  and SIGKILLs its process group itself: the same end, a moment later, from
  the host (`link_gone`).
- The editor's child is the proxy, not the agent, so its pid and process tree
  differ. That separation is what lets the side channel exist.
- Side-channel actions, where a profile enables them: ADR 4.

## Considered

- `--on-disconnect direct|headless`, and the profile's `on_disconnect`: the
  agent could carry on headless when the editor went, the host taking over
  the editor's pending agent requests (`go_headless`) and announcing it
  (`owner_changed`). Gone: one owner per session (P11), and a directly run
  agent goes with its editor (P1).
- The old reason for dropping `fs` and `terminal`, "the host won't implement
  them for when the editor is gone", went with `go_headless`. ACP v2 is the
  reason now, and the README gives it.
- Passing `fs` and `terminal` through always: by P1's test the agent is
  affected, since it can't use the editor's buffers or terminal. Kept for
  strict mode; the default follows v2.

## Tests

Run `cargo test --release adr_0002_`. Named claims and their assertions:

- [tests/security.rs](../../tests/security.rs)
  - `adr_0002_acp_passes_bytes_unchanged`.
  - `adr_0002_acp_passes_stderr_and_the_exit_status`.
- [tests/headless.rs](../../tests/headless.rs)
  - `adr_0002_acp_is_what_an_editor_runs`.
  - `adr_0002_editor_gone_takes_the_agents_children`.
  - `adr_0002_a_signal_doesnt_wait_behind_a_stalled_agents_stdin`.
  - `adr_0002_an_editors_process_needs_its_signal_link`.
  - `adr_0002_non_blocking_stdin_is_waited_on`.
