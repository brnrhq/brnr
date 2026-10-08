# 42. Feature flags for process management

Accepted 2026-10-07. Implemented.

## Context

Some of brnr's choices aren't protocol at all, but how its processes share
sessions. ACP has no opinion on two processes serving one session; the agent
copes by splitting the conversation (ADR 3). Choices like that need a default
that protects the user and a way to choose otherwise, kept apart from strict
mode, which is about the protocol (ADR 41), and from experimental actions,
which act on an editor's session (ADR 4).

## Decision

- `features = [...]` in a profile part names the process-management
  behaviours it turns on. Each is off by default; unknown names fail to load
  (P7). The first:
  - `shared_sessions`, in `[profiles.<p>.editor]`: an editor may load or
    resume a session another brnr process owns (ADR 3).
- Strict mode doesn't change them.

## Considered

- Deciding them by strict mode: they aren't protocol.
- Making them experimental actions: they don't add to an editor's session;
  they decide what brnr's own processes may do.
