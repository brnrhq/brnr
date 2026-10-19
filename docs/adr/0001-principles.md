# 1. Critical success factors and principles

Draft 2026-10-07.

Amended by 44: adds P15, brnr never phones home.

Amended by 53: a session id in a file name is escaped, one name per id (P8).

Amended by 59: transcripts are written only where only the user can read
them, existing ones included (P13).

Amended by 64: from 1.0.0, P9's compatibility within a major version is of
what ADR 64 lists as covered, and of nothing else.

What brnr must get right, its critical success factors, and the rules the
other decisions are measured against, its principles, which serve the
factors. The principles were drawn out of the former decision log when every
entry was reviewed (October 2026), and sharpened where earlier decisions
pulled against them. The factors were set over them after the architectural
review of October 2026, which found decisions that traded one principle for
another with nothing to rank them. Decisions cite the factor at stake, then
the principles (P1 to P14, and P15 in ADR 44); one that gives up a factor or
breaks a principle says which, and why.

brnr plays two roles (P12): with an editor attached it is a proxy between the
editor and the agent; headless, it is the agent's ACP client. Several
principles apply differently to the two.

## Critical success factors

### What brnr is for

#### F1. ACP, faithfully

ACP is the protocol brnr implements; doing it faithfully and correctly is
the goal. Faithful means one thing in each role (P12): with an editor
attached, brnr is invisible, and what passes between editor and agent passes
unchanged (P1); headless, brnr is a correct ACP client, and behaves the same
with every agent (P2). In strict mode, only ACP's stable parts are there
(ADR 41). Principles: P1, P2, P12.

#### F2. Conventions ahead of the spec

brnr also follows conventions before ACP makes them stable, so that it stays
useful while the protocol moves: a pragmatic implementation is often more
useful than a strict one. The test is P1's: agents and editors implement the
convention alike. One adapter's behaviour is not a convention (P2). Strict
mode leaves them out. Principles: P1, P2.

#### F3. What ACP leaves out

brnr is useful for what ACP doesn't specify: headless sessions, session
locks, watchers, transcripts, bridges, notifications. These are outside
ACP's scope, and they are the value brnr adds. The limit is P1's test: what
brnr adds can't change what a party that follows ACP sees, so it needs no
flag. Its defaults protect the user, and feature flags change them
(ADR 42). Principles: P1, P10, P11, P12.

#### F4. Actions on an editor's session, opted into

Some useful features act on an editor's session, as messages sent through
the side channel do, and need compensating behaviour that is not faithful
to ACP. brnr has them, each enabled by name as experimental (ADR 4, ADR 67).
They are kept apart from F3's feature flags: those change brnr's own process
management, not what the editor sees. Principles: P1, P12.

#### F5. A focused command line

brnr is a Unix command-line utility: a focused set of operations, each doing
one thing, that composes with other tools through arguments, standard
streams, exit statuses and JSON. Scripts and agents can rely on it: nothing
is lost silently (P3), it never guesses (P4), text and JSON carry the same
data (P5), a request that can't be met fails (P7), and from 1.0.0 what
ADR 64 covers stays compatible (P9). What another tool can do (a bridge, a
`notify` command, `jq`) stays out of brnr. Principles: P3, P4, P5, P7, P9.

### How brnr must run

#### N1. Security

Security comes first. brnr has few crates, each audited by a person or
listed as an exemption until one is (`cargo vet`); an audit is a person's
claim to have read the code. The agent's text is untrusted (P8); only the
user can reach brnr's files and sockets (P13), and that includes what brnr
reads as well as what it writes; brnr calls no one (P15). The threat model
(docs/threat-model.md) says what enforces each claim, and its gaps.
Principles: P8, P13, P15.

#### N2. Reliability

brnr runs where reliability is required. Its processes respect the system
they run on: memory, threads, files and processes stay bounded whatever a
peer, the agent or the editor sends (P6); a start is atomic (P10); no
process is left without an owner (P14); a failure is reported and cleaned
up, and doesn't take down more than it must. This is the goal, not a claim
about brnr today: what ADR 51 lists as not bounded by a count of bytes (a
message the agent is still sending, what a session keeps, a line on the
control socket) is a gap measured against it. Principles: P6, P10, P14.

#### N3. Performance

brnr is light: one small process per agent, that adds little latency
between editor and agent and little load to the machine. Principles: P6.

#### N4. Simplicity

brnr has few concepts, each with one owner in the code, as a focused
utility should (F5). A layer of its own is thin, named and visible (P2). A
change that adds a concept says why an existing one doesn't do.
Principles: P2, P9.

### When they pull apart

Security (N1) first, then reliability (N2), then faithfulness to ACP (F1).
The other factors give way to these three. A record that gives up one factor
for another says which, and why.

F1 and F2 meet by mode, not by rank: by default brnr follows the
conventions ahead of the spec (F2), and strict mode is ACP's stable parts to
the letter (F1). A convention followed by default is not a conflict with F1,
as long as strict mode leaves it out.

## Principles

### P1. Follow ACP faithfully

brnr does what ACP specifies, as it specifies it. The test for anything it
does beyond that: can a client or an agent that follows the protocol be
affected by it, now or as the protocol moves on? Three kinds of "beyond" are
kept apart:

- The protocol itself. By default brnr speaks stable ACP plus the conventions
  current agents and editors implement alike, ahead of the spec
  (`_session/steering`, `session/fork` while it is unstable, dropping `fs`
  and `terminal` as ACP v2 does, but those a profile names). Strict mode,
  chosen per process, is stable ACP to the letter and nothing else
  (ADR 41).
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
default the editor's `fs` and `terminal` capabilities are dropped, unless
the profile names them (ADR 67); an
editor's load of a session another process owns is refused unless a feature
flag allows it (ADR 3); and the experimental actions, where enabled, add to
and answer for the editor's session (ADR 4, ADR 26).

### P2. ACP as ACP has it

Sessions, modes, config options, approvals and login are the agent's. brnr
adds a layer of its own only where ACP has nothing (message ids, held
messages, waiting, idle stops, permission timeouts, events, transcripts), and
keeps it thin, named and visible. A convention agents and editors implement
alike can be followed (P1); one adapter's undocumented behaviour can't be
relied on, even where ACP has nothing: to brnr the adapter is the agent, and
adapters differ (what a prompt sent mid-turn means, ADR 18).

### P3. Nothing is lost silently

Whatever brnr drops, refuses, cuts off or ends is told to whoever relied on
it, in their own channel: an event for bridges and watchers, an error and an
exit status for the CLI, a `session/update` for the editor (P1). The host log
is the record, not the telling.

### P4. brnr never picks or guesses

An argument means one thing: exact ids, no prefixes, never "the only one".
Defaults are fixed rules, documented, that don't depend on what happens to be
running. When brnr can't be sure (a process that doesn't answer, a line it
can't parse), it refuses in the client role and passes the thing through
untouched in the proxy role, and says so.

### P5. One interpretation, one stream

The host interprets ACP once, into events. Everything brnr shows is made of
them: `watch`, `log`, `status`, the foreground, bridges, `notify`, live or
read back from the transcript. Text and JSON carry the same events: an event
chosen is shown in both, and what is too noisy for the default is left out of
the default in both. Beside the events, brnr keeps the raw ACP by default
(ADR 22), so the interpretation can be checked against what was said; values
brnr knows are secrets are recorded redacted (ADR 25).

### P6. Memory stays bounded

When a reader falls behind, who gives way depends on what it is. Observers
(bridges, watchers, the foreground display) are cut off or skip, and are told
(P3); they never slow the session. The session's own pipes (the editor link,
the agent's stdin and stdout) get backpressure, as a direct pipe would give
them (P1): nothing buffered past a cap, and no timeout of brnr's own (ADR 6).

### P7. An explicit request that can't be met fails

With the reason, up front where it can be known (at start, against the
agent's capabilities); never quietly ignored or approximated.

### P8. The agent's text is untrusted

It never reaches a command line or a path unsanitized: values go in the
environment (ADR 36), and session ids become file names (transcripts, and
ADR 3's lock files) only escaped, as `paths::file_name` does (ADR 53):
lowercase ASCII letters, digits, `-` and `_` stay, and every other byte is
`%` and two hex digits, so the name stays in its folder and distinct ids
never share one. Wherever brnr shows
it, control characters and bidi overrides are escaped (`render::clean`); JSON
carries it as sent.

### P9. No backwards compatibility before 1.0

Until 1.0.0 a change of name, flag, format or behaviour is made outright: no
aliases for old names, no shims for old formats. Release notes say what
changed. The records are drafts until then too: a decision is rewritten,
renumbered, merged or removed where it is written (docs/adr/README.md).
From 1.0.0 on, compatibility is kept within a major version.

### P10. A start is atomic

Whoever starts a brnr process hands it everything at launch, and the start
commits at the ready report (ADR 7, ADR 8).

### P11. One owner per session

A session's owner is the editor, or headless (later perhaps a web client).
Ownership changes only by closing the session under one owner and resuming
it under the other, when someone asks for it in so many words (ADR 3); never
as a side effect. A feature flag can let an editor's process serve a session
another process owns (ADR 42); the agent then splits the conversation, and
the process holding the session's lock stays its owner of record.

### P12. Two roles

With an editor attached, brnr is a proxy: thin and transparent (P1). The
side channel is for observing and for notifying other services; any action
through it on the editor's session is experimental and enabled one by one
(ADR 4). Headless, brnr is the ACP client, and its CLI is a contract that
behaves the same with every agent (P2).

### P13. Only you

A runtime directory and sockets only the user can open, transcripts only the
user can read; brnr refuses a runtime directory others can use, and listens
on no network. A transcript is written only where the state directory, each
directory below it and the file are the user's own and private, whether
brnr created them or found them: what others can reach is made private
before anything is written, and a symlink, or what isn't the user's, is
refused (ADR 59). Reach beyond the machine is what a bridge the user
configures adds.

### P14. No orphans

Every agent process has an owner that knows it exists: the editor, or a
headless start that was told its session. When the owner goes before that,
the process goes too (ADR 2, ADR 7).
