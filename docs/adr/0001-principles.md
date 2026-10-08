# 1. Principles

Accepted 2026-10-07.

Amended by 44: adds P15, brnr never phones home.

The rules the other decisions are measured against. They were drawn out of
the former decision log when every entry was reviewed (October 2026), and
sharpened where earlier decisions pulled against them. Decisions cite them as
P1 to P14; one that breaks a principle says so, and why.

brnr plays two roles (P12): with an editor attached it is a proxy between the
editor and the agent; headless, it is the agent's ACP client. Several
principles apply differently to the two.

## P1. Follow ACP faithfully

brnr does what ACP specifies, as it specifies it. The test for anything it
does beyond that: can a client or an agent that follows the protocol be
affected by it, now or as the protocol moves on? Three kinds of "beyond" are
kept apart:

- The protocol itself. By default brnr speaks stable ACP plus the conventions
  current agents and editors implement alike, ahead of the spec
  (`_session/steering`, `session/fork` while it is unstable, dropping `fs`
  and `terminal` as ACP v2 does). Strict mode, chosen per process, is stable
  ACP to the letter and nothing else (ADR 41).
- Actions brnr adds to an editor's session through the side channel:
  experimental, each enabled by name (ADR 4).
- brnr's own process management, such as which sessions a process may serve:
  defaults that protect the user, and feature flags to change them (ADR 42).
  It isn't protocol, so strict mode doesn't change it.

What is outside ACP's scope and can't affect a party that follows it is none
of these, and needs no mark: headless operation (brnr is an ordinary client),
observing a session (`watch`, `log`, `notify`, bridges), brnr's own CLI.

With an editor attached, a faithful intermediary is invisible: `brnr acp`
must not change how the editor and the agent interact. Bytes, signals,
stderr, stdin EOF and the exit status pass through unchanged. What brnr does
through the side channel aims to keep the editor's view in step with the
agent's state; where that can't be done, the gap is a caveat, stated in the
README. The deliberate changes to the stream are few, and each is named: by
default the editor's `fs` and `terminal` capabilities are dropped (ADR 2); an
editor's load of a session another process owns is refused unless a feature
flag allows it (ADR 3); and the experimental actions, where enabled, add to
and answer for the editor's session (ADR 4, ADR 26).

## P2. ACP as ACP has it

Sessions, modes, config options, approvals and login are the agent's. brnr
adds a layer of its own only where ACP has nothing (message ids, held
messages, waiting, idle stops, permission timeouts, events, transcripts), and
keeps it thin, named and visible. A convention agents and editors implement
alike can be followed (P1); one adapter's undocumented behaviour can't be
relied on, even where ACP has nothing: to brnr the adapter is the agent, and
adapters differ (what a prompt sent mid-turn means, ADR 18).

## P3. Nothing is lost silently

Whatever brnr drops, refuses, cuts off or ends is told to whoever relied on
it, in their own channel: an event for bridges and watchers, an error and an
exit status for the CLI, a `session/update` for the editor (P1). The host log
is the record, not the telling.

## P4. brnr never picks or guesses

An argument means one thing: exact ids, no prefixes, never "the only one".
Defaults are fixed rules, documented, that don't depend on what happens to be
running. When brnr can't be sure (a process that doesn't answer, a line it
can't parse), it refuses in the client role and passes the thing through
untouched in the proxy role, and says so.

## P5. One interpretation, one stream

The host interprets ACP once, into events. Everything brnr shows is made of
them: `watch`, `log`, `status`, the foreground, bridges, `notify`, live or
read back from the transcript. Text and JSON carry the same events: an event
chosen is shown in both, and what is too noisy for the default is left out of
the default in both. Beside the events, brnr keeps the raw ACP by default
(ADR 22), so the interpretation can be checked against what was said; values
brnr knows are secrets are recorded redacted (ADR 25).

## P6. Memory stays bounded

When a reader falls behind, who gives way depends on what it is. Observers
(bridges, watchers, the foreground display) are cut off or skip, and are told
(P3); they never slow the session. The session's own pipes (the editor link,
the agent's stdin and stdout) get backpressure, as a direct pipe would give
them (P1): nothing buffered past a cap, and no timeout of brnr's own (ADR 6).

## P7. An explicit request that can't be met fails

With the reason, up front where it can be known (at start, against the
agent's capabilities); never quietly ignored or approximated.

## P8. The agent's text is untrusted

It never reaches a command line or a path unsanitized: values go in the
environment (ADR 36), and session ids become file names (transcripts, and
ADR 3's lock files) only sanitized, as `paths::session_log` does: anything
but ASCII letters, digits, `-`, `_` and `.` becomes `_`. Wherever brnr shows
it, control characters and bidi overrides are escaped (`render::clean`); JSON
carries it as sent.

## P9. No backwards compatibility before 1.0

Until 1.0.0 a change of name, flag, format or behaviour is made outright: no
aliases for old names, no shims for old formats. Release notes say what
changed. From 1.0.0 on, compatibility is kept within a major version.

## P10. A start is atomic

Whoever starts a brnr process hands it everything at launch, and the start
commits at the ready report (ADR 7, ADR 8).

## P11. One owner per session

A session's owner is the editor, or headless (later perhaps a web client).
Ownership changes only by closing the session under one owner and resuming
it under the other, when someone asks for it in so many words (ADR 3); never
as a side effect. A feature flag can let an editor's process serve a session
another process owns (ADR 42); the agent then splits the conversation, and
the process holding the session's lock stays its owner of record.

## P12. Two roles

With an editor attached, brnr is a proxy: thin and transparent (P1). The
side channel is for observing and for notifying other services; any action
through it on the editor's session is experimental and enabled one by one
(ADR 4). Headless, brnr is the ACP client, and its CLI is a contract that
behaves the same with every agent (P2).

## P13. Only you

A runtime directory and sockets only the user can open, transcripts only the
user can read; brnr refuses a runtime directory others can use, and listens
on no network. Reach beyond the machine is what a bridge the user configures
adds.

## P14. No orphans

Every agent process has an owner that knows it exists: the editor, or a
headless start that was told its session. When the owner goes before that,
the process goes too (ADR 2, ADR 7).
