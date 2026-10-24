# 1. Critical success factors and principles

Draft 2026-10-07, consolidated 2026-10-19.

What brnr must get right, its critical success factors, and the rules the
other decisions are measured against, its principles, which serve the
factors. The principles were drawn out of the former decision log when every
entry was reviewed (October 2026), and sharpened where earlier decisions
pulled against them. The factors were set over them after the architectural
review of October 2026, which found decisions that traded one principle for
another with nothing to rank them. Decisions cite the factor at stake, then
the principles (P1 to P15); one that gives up a factor or breaks a
principle says which, and why.

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
(ADR 2.1). brnr doesn't compensate for an agent that doesn't follow ACP
or JSON-RPC: what the agent does wrong shows, as it would without brnr.
Principles: P1, P2, P12.

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
(ADR 3.10). Principles: P1, P10, P11, P12.

#### F4. Actions on an editor's session, opted into

Some useful features act on an editor's session, as messages sent through
the side channel do, and need compensating behaviour that is not faithful
to ACP. brnr has them, each enabled by name as experimental (ADR 7.1).
They are kept apart from F3's feature flags: those change brnr's own process
management, not what the editor sees. Principles: P1, P12.

#### F5. A focused command line

brnr is a Unix command-line utility: a focused set of operations, each doing
one thing, that composes with other tools through arguments, standard
streams, exit statuses and JSON. Scripts and agents can rely on it: nothing
is lost silently (P3), it never guesses (P4), text and JSON carry the same
data (P5), a request that can't be met fails (P7), and from 1.0.0 what
ADR 8.4 covers stays compatible (P9). What another tool can do (a bridge,
an `event notify` command, `jq`) stays out of brnr. Principles: P3, P4, P5,
P7, P9.

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
about brnr today: what ADR 10.3 lists as not bounded by a count of bytes (a
message the agent is still sending, what a session keeps, a line on the
control socket) is a gap measured against it. Principles: P6, P10, P14.

#### N3. Performance

brnr is light: one small process per agent, that adds little latency
between editor and agent and little load to the machine. Principles: P6.

What it adds is measured on every pull request (#35): `cargo bench --bench
acp` (benches/acp.rs), which .github/workflows/bench.yml runs on Linux and
macOS, fails when a measure passes its budget in benches/budgets.toml, and
one comment on the pull request has the numbers. The measures, and their
budgets:

- Latency: a prompt's round trip through `brnr acp`, the median, over the
  same agent's on a direct pipe: at most 25 times.
- Throughput: the time a turn that streams 20 MB takes through `brnr acp`
  (1 KiB chunks, 16 to a message), over the direct pipe's: at most 20
  times.
- Memory at rest: the resident memory of `brnr acp`, 8 MiB at most, and of
  its process, 16 MiB, initialized with a session open; and of the process
  after the streams, 64 MiB (a busy machine queues more of the stream, up
  to ADR 10.1's caps, and the allocator keeps some of it).
- Threads: `brnr acp` 3; its process at rest, with an editor attached, 12;
  each control connection adds 2, and each bridge 4.

The agent is a fake that answers at once, so the numbers are brnr's own:
with a real agent, brnr adds the same time, and the ratio is smaller. Shared CI
runners are noisy, so latency and throughput are ratios to the direct
pipe, measured at the same time on the same machine, with wide margins,
and the job doesn't fail on a slow runner. Threads don't vary, so their
budgets are today's counts: a new thread is a change its pull request
chooses, raising the budget and saying why. A message assembled whole from
its chunks is ADR 10.3's gap, not measured here: the stream is many
messages. The README and the book's introduction give one run's numbers,
with the machine and the version.

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
  (ADR 2.1).
- Actions brnr adds to an editor's session through the side channel:
  experimental, each enabled by name (ADR 7.1).
- brnr's own process management, such as which sessions a process may serve:
  defaults that protect the user, and feature flags to change them
  (ADR 3.10). It isn't protocol, so strict mode doesn't change it.

What is outside ACP's scope and can't affect a party that follows it is none
of these, and needs no mark: headless operation (brnr is an ordinary client),
observing a session (`event watch`, `event log`, `event notify`, bridges),
brnr's own CLI.

With an editor attached, a faithful intermediary is invisible: `brnr acp`
must not change how the editor and the agent interact. Bytes, signals,
stderr, stdin EOF and the exit status pass through unchanged. What brnr does
through the side channel aims to keep the editor's view in step with the
agent's state; where that can't be done, the gap is a caveat, stated in the
documentation. The deliberate changes to the stream are few, and each is named:
requests reach the agent with the host's ids (ADR 2.4); by default the
editor's `fs` and `terminal` capabilities are dropped, unless the profile
names them (ADR 2.7); an editor's load of a session another process owns is
refused unless a feature flag allows it (ADR 3.9); and the experimental
actions, where enabled, add to and answer for the editor's session
(ADR 7.1, ADR 2.5).

### P2. ACP as ACP has it

Sessions, modes, config options, approvals and login are the agent's. brnr
adds a layer of its own only where ACP has nothing (message ids, held
messages, waiting, idle stops, response timeouts, events, transcripts), and
keeps it thin, named and visible. A convention agents and editors implement
alike can be followed (P1); one adapter's undocumented behaviour can't be
relied on, even where ACP has nothing: to brnr the adapter is the agent, and
adapters differ (what a prompt sent mid-turn means, ADR 4.2).

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
them: `event watch`, `event log`, `session status`, the foreground, bridges,
`event notify`, live or read back from the transcript. Text and JSON carry
the same events: an event chosen is shown in both, and what is too noisy for
the default is left out of the default in both. Beside the events, brnr
keeps the raw ACP by default (ADR 5.1), so the interpretation can be checked
against what was said; values brnr knows are secrets are recorded redacted
(ADR 9.1). The events are a summary, not the conversation, which is the
agent's: what each stream keeps and leaves out is ADR 5.10's.

### P6. Memory stays bounded

When a reader falls behind, who gives way depends on what it is. Observers
(bridges, watchers, the foreground display) are cut off or skip, and are told
(P3); they never slow the session. The session's own pipes (the editor link,
the agent's stdin and stdout) get backpressure, as a direct pipe would give
them (P1): nothing buffered past a cap, and no timeout of brnr's own
(ADR 10.1).

### P7. An explicit request that can't be met fails

With the reason, up front where it can be known (at start, against the
agent's capabilities); never quietly ignored or approximated.

### P8. The agent's text is untrusted

It never reaches a command line or a path unsanitized: values go in the
environment (ADR 5.8), and session ids become file names (transcripts, and
ADR 3.9's lock files) only escaped, as `paths::file_name` does (ADR 9.2):
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
From 1.0.0 on, compatibility is kept within a major version, of what
ADR 8.4 lists as covered, and of nothing else, not of every flag's
spelling beyond it. What brnr keeps on disk says its format, so that a
brnr that can't read it says so rather than misread it or pass it over
without a word (ADR 5.10).

### P10. A start is atomic

Whoever starts a brnr process hands it everything at launch, and the start
commits at the ready report (ADR 3.4, ADR 3.3).

### P11. One owner per session

A session's owner is the editor, or headless (later perhaps a web client).
Ownership changes only by closing the session under one owner and resuming
it under the other, when someone asks for it in so many words (ADR 3.9);
never as a side effect. A feature flag can let an editor's process serve a
session another process owns (ADR 3.10); the agent then splits the
conversation, and the process holding the session's lock stays its owner of
record.

### P12. Two roles

With an editor attached, brnr is a proxy: thin and transparent (P1). The
side channel is for observing and for notifying other services; any action
through it on the editor's session is experimental and enabled one by one
(ADR 7.1). Headless, brnr is the ACP client, and its CLI is a contract that
behaves the same with every agent (P2).

### P13. Only you

A runtime directory and sockets only the user can open, transcripts only the
user can read; brnr refuses a runtime directory others can use, and listens
on no network. A transcript is written only where the state directory, each
directory below it and the file are the user's own and private, whether
brnr created them or found them: what others can reach is made private
before anything is written, and a symlink, or what isn't the user's, is
refused (ADR 9.3). The config file is read only if it and its folder are
the user's own and nobody else can write them, nor read the file
(ADR 9.4). Reach beyond the machine is what a bridge the user
configures adds.

### P14. No orphans

Every agent process has an owner that knows it exists: the editor, or a
headless start that was told its session. When the owner goes before that,
the process goes too (ADR 3.1, ADR 3.4).

### P15. brnr never phones home

brnr makes no network requests of its own. It collects and sends no
telemetry, usage data, error reports or update checks. What reaches the
network does so because the user configured it: the agent's (and its
adapter's) traffic, a bridge's, an `event notify` command's, an MCP
server's.

- A feature that needs the network is a command the user runs or
  configures, never a default and never in the background.
- What brnr knows about a session it keeps on the machine, readable only by
  the user (P13).
- An adapter brnr packages (ADR 11.1) is checked against P15 when its
  version moves: it may reach the agent's provider, not anyone of brnr's.
- The principle is about brnr, the program. The project's website and
  repository are outside it; what they count is said where they say it
  (#40).

Why: whatever brnr sent, even a harmless counter, would ask the user to
trust that it sends nothing else. Saying it never sends anything is worth
more than what it could learn. Opt-in telemetry was considered and not
taken: it needs a server and someone to run it, and its counters would be
one more thing to trust in a tool whose point is that it is trusted.
Opt-out telemetry would send the user's data before they knew to stop it.

## Tests

The principles aren't proved by a finite suite: the decisions that apply
them have the tests. N3's measures are checked by `cargo bench --bench acp`
on every pull request (.github/workflows/bench.yml), not by a Rust test:
their numbers depend on the machine.
