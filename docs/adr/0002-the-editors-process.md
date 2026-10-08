# 2. The editor's process: `brnr acp` relays, the host owns the agent

Accepted (former decisions 32 and 40); reviewed 2026-10-07. Implemented.

## Context

The editor runs brnr as its agent. brnr needs the agent in a process the
editor doesn't own, so that other commands can reach its sessions, while the
editor notices nothing different (P1).

## Decision

- `brnr acp [--profile <p>] [-- <agent> [args...]]` is the proxy. It starts a
  brnr process (the host) detached (a new session, double fork, reparented to
  init or launchd), with one end of a socketpair, and from then on only
  relays: stdin and stdout as frames, the agent's stderr to its stderr, every
  signal it receives to the host, stdin EOF; at the end it exits with the
  agent's exact wait status, re-raising the signal that killed it. The host
  starts the agent in a process group of its own, with the editor's signal
  mask.
- It is named for what it is to the editor, an ACP agent command
  (`brnr acp -- brnr-claude-adapter`). It was `brnr proxy`; that name is gone,
  not kept as an alias (P9). Inside, it is still the proxy (`proxy.rs`,
  `proxy_pid` in metadata and transcripts).
- The editor's agent goes with the editor. When the editor goes away (stdin
  EOF, or its stdout closes), the agent's stdin is closed and it is stopped
  with its process group, as if the editor had run it. To carry on with a
  session headless, `brnr start --resume <session>`; the agents keep their
  sessions. Lost: a turn still running when the editor closes. A process's
  owner is set when it starts (P11, P14).
- By default the editor's `initialize` loses the `fs` and `terminal` client
  capabilities, one of the few deliberate changes to the stream (P1). ACP v2
  drops them (its `ClientCapabilities` has neither), they add nothing an
  agent needs, and without them now no agent comes to rely on them. In strict
  mode they pass through, as stable v1 has them (ADR 41).

## Caveats

What the editor sees differently, stated in the README:

- A SIGKILL sent to the proxy can't be passed on. The agent gets the graceful
  stop instead: stdin closed, SIGTERM, SIGKILL.
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
  reason now; the README still gives the old one, and is to be rewritten.
- Passing `fs` and `terminal` through always: by P1's test the agent is
  affected, since it can't use the editor's buffers or terminal. Kept for
  strict mode; the default follows v2.
