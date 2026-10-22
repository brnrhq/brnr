# 2. ACP, faithfully

Draft 2026-10-19. Implemented, but the real-editor checks for 2.7's `fs`
and `terminal` (in ADR 7.1) are not run yet. Factor: F1, F2 (ADR 1).
Brings together former ADRs 41, 54, 43, 61, 26, 62 and 67
(docs/adr/README.md maps them).

How brnr speaks ACP: which version, which types, which conventions ahead
of the spec, and what strict mode leaves out; how it keeps the editor's
stream byte for byte where it can, and names each change it makes. By
default brnr follows the conventions agents and editors implement alike
(F2); strict mode is stable ACP to the letter (F1).

## 2.1 Strict mode, and the conventions brnr follows by default

- By default brnr speaks stable ACP plus the conventions current agents
  and editors implement alike. Each is listed here, with what brnr does
  with it:
  - `_session/steering`, advertised as `_meta.steering.supported` in
    `initialize`, with `_meta.steering.idleBehavior: "promptRequired"`
    (both adapters): `prompt send --steer` (ADR 4.2).
  - `session/fork`, unstable in ACP v1 (both adapters): `session fork`
    (ADR 3.12).
  - Dropping the editor's `fs` and `terminal` capabilities, as ACP v2
    does, but those the profile names (ADR 2.7).

  A convention is added to brnr by adding it to this list.
- Strict mode, per process (`strict = true` in a profile, `--strict` on
  `acp`, `session new` and `session resume`), is stable ACP to the letter
  and nothing else: no extension methods, no unstable methods, no `_meta`
  field acted on, the editor's capabilities passed through, and none of
  ADR 7.1's experimental actions, whatever the profile enables. What
  strict mode refuses says why ("`--steer` uses `_session/steering`, an
  ACP extension, which strict mode doesn't").
- Strict mode is about the protocol only. Headless operation, observing a
  session, brnr's CLI and its process management (`process stop`, which
  isn't one of ADR 7.1's actions; the feature flags, ADR 3.10) are the
  same with it and without.
- Stable means what the ACP schema doesn't mark unstable, as
  `agent-client-protocol-schema` builds it without its unstable features
  (ADR 2.3).

**Why.** ACP to the letter (P1) leaves out what agents and editors already
implement ahead of the spec, and what people use: steering a running turn,
forking a session (F2). The practical setup leans on conventions a
stricter one may not want: an extension method, a `_meta` field, a method
the schema marks unstable. F1 and F2 meet by mode (ADR 1). As of
`@agentclientprotocol/sdk` 1.6.0 the schema marks, in v1, `session/list`,
`resume`, `close` and `delete` stable, `session/fork` unstable, and `fs`
and `terminal` stable client capabilities; v2 drops `fs`, `terminal`,
`session/load` and `session/set_mode`. `$/cancel_request` is stable in
both. Extension methods start with `_`.

**Considered.**

- Stable ACP only, always: it leaves out steering and forking, which both
  adapters implement and people use.
- Each convention as an experimental action, enabled by name like
  ADR 7.1's: conventions don't act on an editor's session behind its back;
  a switch per process is enough.
- Strict mode also deciding process management (whether an editor may
  share a session another process owns): that isn't protocol (ADR 3.10).

## 2.2 brnr speaks ACP version 1

- brnr speaks ACP version 1 (`schema::PROTOCOL_VERSION`, the schema
  crate's `ProtocolVersion::V1`), and asks for it in `initialize`.
- An answer to its `initialize` that chooses another version, or none
  brnr can read as one (missing, a string, not a whole number up to
  65535), ends there: nothing else of the answer is read, and nothing more
  is sent. A start fails before it commits (ADR 3.4), as any start does:
  `session new` or `session resume` exits with status 1 and says "the
  agent speaks ACP version 999; brnr speaks only version 1", or "the
  agent's answer to initialize has no ACP version brnr can read
  (protocolVersion: …)", and the process stops, and with it the agent.
  `session list` with an agent named fails with the same, and stops its
  agent.
- Strict mode and the default are the same here: version 1 is what brnr
  implements, so it is the only one either accepts. A version is added
  when it is implemented, by a decision that says how.
- An editor's process isn't the client: the editor's `initialize` and the
  agent's answer are passed through (P1), and the version they agree on is
  theirs to judge.

**Why.** In ACP's initialization the client asks for the latest version it
speaks; an agent that speaks it answers with the same one, and one that
doesn't answers with the latest it does. A client that doesn't speak the
version chosen is to close the connection and tell the user. brnr's
schema types are version 1 (ADR 2.3): to read another version's messages
as version 1 is a guess (P4). Resolves #66.

**Considered.**

- Going on with a version brnr doesn't speak, and reading what it can (P4
  read as "as far as brnr can"): what brnr reads and sends would be a
  guess at what another version means, and ACP says the client stops.
- Accepting a lower version (0, the pre-release one) as close enough: brnr
  doesn't implement it, and the schema crate calls it unsupported.
- Checking the editor's negotiated version too, and refusing for it: the
  editor is the client there, and brnr in the proxy role doesn't refuse
  what it can pass through (P4, as in ADR 3.9).

## 2.3 ACP's types come from the official schema crate

- brnr keeps its byte relay and its threads, and interprets its copy of
  each message with `agent-client-protocol-schema`'s types (1.10.2,
  without schemars and without its unstable features; src/schema.rs),
  where they take what it reads. Each part is read as its type, in place
  of hand-written lookups.
- Read with its types: session updates, the capabilities and login
  methods in `initialize`'s answer, the session `session/new` or
  `session/fork` opened, permission option kinds, JSON-RPC errors, and
  the `session/list` of `session list` with an agent named. Written with
  its types: `session/set_config_option`'s params (ADR 6.2).
- Left to `serde_json::Value`, read as far as brnr can, as the types don't
  take them or brnr passes them on as they were said: the JSON-RPC
  envelope and the editor's requests, extension methods and `_meta`, the
  unstable `fork` capability, a `session/prompt` answer's unstable `usage`
  (read for the turn's tokens, ADR 4.4), update kinds the schema lacks,
  what brnr rewrites, and what events and the status carry as it was said
  (a prompt's content, a permission request's tool call and options,
  locations, plan entries, config options, commands, modes, stop
  reasons).
- The crate's stable types are what P1 and strict mode (ADR 2.1) mean by
  stable ACP; strict mode's stable methods are the crate's. Its unstable
  features stay off.
- The crate parses with serde_json too, so reading lone surrogates, and
  nesting past serde_json's 127 levels up to brnr's limit, stays brnr's
  own work (ADR 2.5).
- A message the types don't know (an extension method, a newer field) is
  still forwarded untouched, and interpreted as far as brnr can (P4).

**Why.** brnr forwards the original bytes and interprets a copy of each
message (P1). With hand-written lookups (`msg["params"]["sessionId"]`),
nothing tells brnr what is stable ACP and what isn't. The official Rust
libraries, as of October 2026: `agent-client-protocol` (3.1.0, built on
Symposium's `sacp`), with `Client`, `Agent`, `Proxy` and `Conductor`
roles, async, whose proxies follow the proxy-chains RFD, still a
prototype (a conductor starts a proxy with `proxy/initialize` and talks to
it through `proxy/successor` envelopes); and
`agent-client-protocol-schema` (1.10.2), the protocol's types with serde,
ACP v2's draft behind `unstable_protocol_v2`, and no runtime.

**Considered.**

- The full SDK's `Proxy` and `Conductor`: a rewrite onto async; every
  message decoded and encoded again, so byte fidelity would depend on
  routing everything through its raw paths; and brnr isn't a proxy in the
  RFD's sense, since the editor runs no conductor.
- A spike of an SDK-based proxy first, to measure that.
- Keeping hand-written access to `serde_json::Value`: nothing tells brnr
  what is stable ACP and what isn't.
- Later, perhaps: brnr as a component in someone else's conductor chain,
  an experimental feature once the proxy-chains RFD is stable.

## 2.4 Every request to the agent has an id of the host's

- Every request to the agent has an id the host gives it, `brnr-<n>`, from
  one counter: the editor's too. Ids on the agent's side are the host's
  alone, so no two requests waiting for an answer have the same one,
  whatever ids the editor writes.
- The host keeps, for each request waiting for its answer, who sent it:
  the editor, the host itself (a start's or a bridge's request), or a
  message it sent as a prompt. The answer goes to that one only. The
  editor's goes back to it with the id as the editor wrote it, byte for
  byte: `7` and `"7"` are two requests, and `1e3` comes back as `1e3`.
- In a line, only the id changes: the host finds the request's top-level
  `id` members in the line (an escaped name, and one given twice, too) and
  writes its own id there, and the editor's back into the answer. The rest
  goes on byte for byte, as ADR 3.1 has it.
- The editor's `$/cancel_request` names one of its own requests by its
  id: it goes to the agent with that request's id there. One naming an id
  the host gave a request, which the editor was never told, doesn't go,
  and the host log says so (`editor_cancel_dropped`): it can't be the
  editor's to cancel. One naming neither goes as it came, for a request in
  a line the host couldn't read (ADR 2.5).
- What brnr records of a request and its answer is as the agent had them,
  with the host's id; the `prompt` of `user_message` and `turn_ended` is
  that id too.
- The agent's own requests to the editor keep the agent's ids: only the
  agent sends those, and the editor answers them.

**Why.** The host sends the agent requests of its own (a headless start's,
a bridge's, a message sent as a prompt or a steer), and the editor may
choose any id, the host's included. The agent's answer carries only the
id, so to know whose answer it is, the id must say so. With one shared
namespace, an editor's prompt could end with the answer to a host
request, or the other way round, and a `$/cancel_request` could cancel
the other's request (GHSA-84pw-hh9c-w2m8).

**Consequences.**

- A request in a line the host doesn't read (one that isn't JSON, nested
  past 128 or past 32 MiB, ADR 2.5 and 10.3, or what the editor wrote
  after its last newline) reaches the agent with the editor's id, untracked. Should that
  id be one the host has given a request still waiting, the answer is
  taken for that request's: the editor can only bring that about with an
  id it wasn't told, and with a line nobody can read.
- An answer the host can't read reaches the editor as the agent wrote it,
  with the host's id, so the editor can't place it (ADR 2.5).
- Only the agent's ids are on the editor's side, so an agent that reuses
  an id after the host answered a request with it (a cancel, an allow)
  can have the editor's late answer to the first taken for the second's.
  No agent brnr knows reuses ids; the editor's side isn't rewritten for
  it.

**Considered.**

- A prefix of the host's (`brnr-`), with the editor asked not to use it,
  or its ids that have it refused or rewritten: the editor may use any id,
  and any prefix is one it may choose.
- Rewriting only the editor's ids that look like the host's: whether two
  ids collide then turns on how each side reads them (`1` and `1.0`, an
  escaped string), which a separate namespace doesn't.
- Re-encoding the whole line with the new id: simpler, but every other
  byte of the editor's requests and the agent's answers would change too
  (number forms, escapes, key order and spacing), which ADR 3.1 promises
  they don't.
- Random host ids: no better than a prefix for the lines the host reads,
  and unneeded for them.

## 2.5 Lines brnr can't parse, and how deep a line may nest

- The host reads every RFC 8259 text nested up to 128 levels (an array or
  object is one level, as `[[1]]` is two). In the copy it interprets, a
  lone surrogate becomes U+FFFD. The line itself is forwarded byte for
  byte, as always. This stays brnr's own work with the official schema
  types (ADR 2.3): they parse with serde_json, which reads 127 levels, and
  no lone surrogate.
- A line nested deeper than 128 isn't read, as a line past 32 MiB isn't
  (ADR 10.3):
  - with an editor it goes on as it came, to the agent or the editor
    (P1). The host treats it as a line it can't parse (below): nothing of
    it is tracked;
  - headless, from the agent, it is dropped: brnr is the client, and
    refuses what it can't read (P4).

  Either way it isn't recorded as raw ACP (what ADR 9.1 redacts in it
  can't be found), and a `line_too_deep` event (`from`: `agent` or
  `editor`; `max_depth`: 128, in levels; `relayed`: whether it went on) says
  so: in every session's events file, to watchers and bridges, and in the
  foreground (P3). As `line_too_long`, it belongs to no session. So is
  what the editor sends after its last newline as it closes its stdin: a
  line too, it goes to the agent, and past 128 isn't recorded.
- A line the host read is recorded as JSON (`msg`), as it came: at 128
  levels too, past serde_json's 127, and with a lone surrogate. Only what
  isn't JSON is a string in `raw` (ADR 5.1).
- What the host keeps of a line nests no deeper than 128, and what it
  writes (events, answers, records) wraps that in a few levels of its own.
  The CLI reads what brnr wrote up to 256 levels, twice the limit: the
  host's answers and events (`permission requests` and `show`,
  `elicitation requests` and `show`, `status` for `discover`, `prompt send
  --wait`, `event wait`, `event watch`, `event notify`, from the socket or
  `--stdin`), and transcripts (`event log`, the catch-up of ADR 10.2, and
  what a transcript is for `session list`, `session resume` and `doctor`,
  ADR 5.1). A line deeper than that is none this brnr wrote: a command
  that reads it as an answer or an event says so ("its answer nests 300
  deep, past the 256 brnr reads") and exits non-zero (P3); a record that
  deep is passed over as one brnr can't read, as a broken one is, and
  `event log` and `doctor --report` say so.
- `session list` with an agent named (ADR 3.2) is the agent's client too:
  a line from it nested past 128 fails the command, which says so.
- Both limits are fixed numbers, the same everywhere, and every thread's
  stack takes a value that deep, to read it and to copy, show or let go of
  it. No thread is made, or sized, for a line.
- A line past 32 MiB isn't read either: with an editor it goes on as it
  comes; headless it is dropped, and a `line_too_long` event says so
  (ADR 10.3).
- An editor's request the host reads goes with an id of the host's
  (ADR 2.4), so an answer to it the host can't read reaches the editor
  with that id.
- For a line that isn't JSON at all, or nests too deep, nothing is
  tracked. The host keeps the ids of agent requests it answered itself
  (permission requests cancelled, or answered from outside, ADR 7.1), and
  drops only the editor's late answers to those, telling the editor in the
  session that its late answer was dropped, and who answered first
  (ADR 7.1). An answer from the editor with any other unknown id goes to
  the agent, which ignores an answer to something it never asked.
- Lines aren't scanned for ids (P4), and nothing is withheld from the
  editor (P1). An editor ignores a response to a request it never sent. A
  host request whose answer can't be read goes unanswered: a start fails
  at its timeout (ADR 3.4), and for a bridge's request `brnr` stops
  waiting after 120 s (ADR 4.6; the host itself never gives up).
- An editor prompt the host doesn't read isn't counted as a turn; nothing
  hangs on it.

**Why.** A line the host can't read goes untracked: an agent's request in
it gets an editor's answer the host doesn't know, and a turn can hang. So
the host reads what RFC 8259 allows and serde_json doesn't. A lone
surrogate (`"\ud83d"`, what `JSON.stringify` makes of a title cut in the
middle of an emoji) is allowed by RFC 8259's grammar, which only calls its
meaning unpredictable: the case that happens. Nesting has no limit in RFC
8259 either, but reading a value takes stack in proportion to its depth,
and so do copying, showing and letting go of it. Until #268, brnr read
any depth on a thread whose stack was sized to the line (depth × 4 KiB):
a 32 MiB line of `[` asked for 128 GiB of stack, and while the host held
a deep value each event and each timer ran on a thread of its own. That
put faithfulness (F1) over reliability (N2), which ADR 1 ranks first:
what the agent or the editor sends chose the host's threads and their
stacks (P6). A fixed limit keeps them bounded, and is documented, so brnr
doesn't guess (P4): in the proxy role a line past it passes through
untouched, as one brnr can't parse does (P1); in the client role it is
refused; either way it is said (P3).

Why 128: it is what serde_json reads by default, so ACP's Rust SDK and the
editors and agents built on serde_json already refuse a line deeper. A
line past it is one such a client couldn't read without brnr. ACP's own
messages nest a few levels, and what an agent puts inside them (a tool
call's input, an elicitation's schema) nests about as deep as what made it.

Why the CLI has a limit of its own: brnr's lines wrap what the host read
(an `acp` event in one level, `pending`'s request in three, `status`'s
`last_turn.error` in four). With one limit for both, the CLI would refuse
what the host wrote about a line at the limit, or the host would read
less than 128. Twice the limit leaves room for that many times over, and
any stack still takes it. The CLI still needs a limit: a transcript an
older brnr wrote, or a process that isn't this brnr, can nest deeper.

**Consequences.**

- An agent that nests a line past 128 works with an editor, which gets
  every byte, but brnr tracks nothing of the line: a permission request
  that deep isn't listed, can't be answered from outside and doesn't time
  out, and a prompt's answer that deep doesn't end the turn for watchers.
  Headless, its request reaches no one, and the turn waits until it is
  cancelled (`prompt cancel`). Agents and editors built on serde_json
  fail on such a line anyway.
- What `--json` prints indented (`event wait --json`, `permission show
  --json`) is small: indenting grows as the square of the depth, about
  35 KB of spaces at 128 levels, where a line a million deep was about
  2 TB (#259).
- No thread of brnr's is sized to what it reads, and the stack a value
  takes is known: about 1 KiB a level to read an object, so 256 KiB at
  256 levels, an eighth of a spawned thread's 2 MiB.

**Considered.**

- Any depth, on a stack sized to the line (brnr until #268): faithful to
  what RFC 8259 allows (F1), but the agent or the editor chooses how much
  stack the host takes, and how many threads (N2, P6).
- A deeper limit (1000, or 10,000): a value that deep needs a stack sized
  for it again, or a parser that doesn't recurse, and serde's derive,
  clone, show and drop all recurse; nothing ACP sends needs it.
- serde_json's limit as it is (127): the number would be serde_json's to
  change, not documented by brnr, and brnr's own lines, which wrap the
  agent's, nest deeper.
- One limit for the host and the CLI: see Why.
- Reading without recursion, with a parser of brnr's own: no stack to
  size, but every use of the value recurses as well, and it is one more
  thing to keep (N4).
- Cutting a deep line, or putting a marker in place of what is too deep:
  it changes the editor's bytes (P1), and headless it is a guess at what
  the rest meant (P4).
- Pulling the `id` and `method` out of a broken or deep line leniently: a
  guess that would have to be right about every way JSON can break.
- Also keeping agent lines that contain `"id":"brnr-` from the editor: a
  heuristic, which withholds the editor's bytes.
- Leaving serde_json's limits and only forwarding unknown ids: the lone
  surrogate, the case that happens, would still go untracked.

## 2.6 The editor's stdin and stdout end when the agent's do, and its stderr comes as written

- The editor's stdout ends when the agent's does, after the last of what
  the agent wrote, whether the agent runs on or not. The host sends the
  proxy `EOF` on the link after the last `DATA` frame (frame.rs), and the
  proxy ends its stdout. Its fd 1 is left open on /dev/null, so no file
  opened later can take its place. The same happens when the editor
  stopped reading and the host closed the agent's stdout for it
  (ADR 3.1). The host log records `agent_stdout_ended`.
- The editor's stdout is the agent's (ADR 3.1). Once it has ended, brnr
  writes nothing more there of its own. A line it would have written (a
  message's tool call, ADR 7.2; a refusal of a load, ADR 3.9; the
  `$/cancel_request` that withdraws a request answered from outside,
  ADR 7.1) isn't sent, and the host log says so with
  `editor_message_dropped`, its `reason` (`agent_stdout_ended`), `session`,
  `method` and `rpc_id` (P3). What the
  line was about still happens: a message sent from outside still reaches
  the agent's stdin, which is open. Only the editor's copy is lost, as
  the agent's answer to it would be.
- The editor's stdin ends when the agent's does, while the editor is
  attached and hasn't ended it itself: when a stop closes it
  (`brnr process stop`, a signal), when the agent closes it, and when the
  agent exits. The host sends the proxy `STDIN_CLOSED` on the link
  (frame.rs), and the proxy ends its stdin, fd 0 left open on /dev/null as
  fd 1 is, so the editor's next write fails (EPIPE), as on the agent's
  pipe. The host log records `agent_stdin_closed`.
- What the editor wrote that reached the host after the agent's stdin
  closed, before the proxy closed its own, goes nowhere: it isn't sent, nor
  recorded in the raw ACP, nor a request waiting for an answer, nor
  answered by brnr. Nor is the start of a line the editor hadn't finished.
  The host log records `editor_bytes_dropped`, with how many `bytes`
  (P3). What the proxy hadn't read goes with its stdin, as what the agent
  hadn't read would go with its pipe.
- The agent's stderr goes on as it is read, newline or not. Each read (up
  to 64 KiB) goes to the editor's stderr through the proxy, and in the
  foreground to its stderr (ADR 3.6), unchanged: every byte once and in
  order, before the exit status.
- What keeps stderr as lines does so alongside, without holding anything
  back. The host log has a record for each line, or for each 64 KiB of a
  longer one (ADR 10.3). The line still coming at the end (when the
  agent's stderr ends, or the host exits) gets one too. A failed start's
  tail (ADR 3.6) ends with the line the agent is in the middle of: a
  prompt's text, without its answer.
- The editor's stderr ends when `brnr acp` exits, not when the agent
  closes its stderr. It is also where `brnr acp` reports its own failures
  (losing its process, a panic's link: ADR 3.7 and 11.5), which ending it
  early would lose (P3).

**Why.** Run directly, an agent's stdout and stderr are two pipes of the
editor's. Each ends when the agent closes it, and each delivers what is
written as soon as it is written (P1). An editor that reads stdout to EOF
must see an agent that closes its stdout and runs on as gone, and a
prompt that waits for an answer (`login required: `) ends no line. Once
the agent's stdout has ended, lines of brnr's own there would be answers
and updates that no agent stands behind. The agent's stdin is the editor's
pipe too: once the agent has closed it, or gone, the editor's next write
fails. Taken and thrown away, the editor's lines would be written without
an error, waiting for answers that never come, and recorded as sent to an
agent that never got them (P1, P3, P5).

**Consequences.**

- An editor that reads stdout to EOF to see the agent go sees EOF when
  the agent closes stdout. It gets the exit status when the agent exits,
  as it would directly.
- The proxy gets a `STDERR` frame for each read rather than each line: 5
  bytes of header each.
- What the editor still sees differently. Run directly, each stream has
  the kernel's buffer (up to 64 KiB on Linux and macOS), and only its own
  reader holds it back. Through brnr both go on one link, in the order the
  host read them, under one cap (16 MiB, ADR 10.1):
  - An editor that stops reading one stream eventually stops the other.
    The proxy writes frames in order, so stderr waits behind a stdout
    write that blocks, and the other way round. Past the cap the host
    stops reading both of the agent's pipes, and the agent blocks writing
    to either. Run directly, an agent blocked on stdout could still write
    to stderr. Editors read both streams, and an ACP agent waits on its
    stdout's reader anyway.
  - Until the cap is reached, the agent's writes complete without the
    editor having read them: up to 16 MiB, rather than a pipe's 64 KiB.
    Nothing is lost or reordered within a stream.
  - An editor that closes its end of stderr gives the agent no EPIPE. The
    proxy's writes there fail and are dropped, and the agent's stderr
    still goes to the host log. (Closing stdout does give EPIPE,
    ADR 3.1.)
  - The editor's write fails later than on a pipe. The host hears that
    the agent closed its stdin only when a write to it fails, so the line
    that finds it closed is recorded, and its request waits, as if it had
    been sent; a write after it fails. `STDIN_CLOSED` goes to
    the proxy behind what the host sent before it, so an editor that
    isn't reading its stdout gets it as late as that. Until the proxy has
    closed its stdin, writes go through, and the host log counts what of
    them it dropped. A stdin macOS can't poll (some devices) closes once
    the read waiting on it returns: one write later.

**Considered.**

- Keeping the editor's stdout open for brnr's own lines after the agent's
  has ended: the editor wouldn't see the agent's stdout end, and the lines
  would be answers and updates that no agent stands behind.
- Closing fd 1 outright in the proxy: the next file it opened would get
  fd 1, and a stray write to stdout would go there. Fd 0 likewise.
- Closing the proxy's stdin from the thread that reads the link: the
  thread relaying stdin, blocked reading it, holds the pipe open until its
  read returns (Linux), so the editor's next write would still go through.
  That thread waits in poll(2) instead, on stdin and on a pipe that the
  link's thread closes for `STDIN_CLOSED`, and closes stdin itself.
- Going on reading the editor's stdin, and answering its requests with an
  error once the agent's stdin has closed: run directly, the agent answers
  nothing, and the editor's write fails (P1).
- Ending the editor's stderr with the agent's: `brnr acp`'s own failures
  would go nowhere (P3).
- A link of its own for stderr (a third socketpair, with its own cap and a
  thread on each side): independent backpressure, as with two pipes, at a
  cost every editor's process pays, for a difference no editor is known
  to depend on. Still open, if one turns out to.
- Sending stderr a line at a time, with a timeout for a line that doesn't
  end: still late, by a delay brnr chooses, and a timeout of brnr's own,
  which ADR 10.1 rules out.

## 2.7 The editor's file system and terminal reach the agent in strict mode, or when its profile names them

- By default, `brnr acp` removes `fs` and `terminal` from the editor's
  `initialize`, as ACP v2 does (F2). The host log notes
  `capabilities_dropped`, with what it dropped. The agent answers as to a
  client that has neither.
- A profile passes them by name, in its editor part (ADR 6.1):
  `client_capabilities = ["fs", "terminal"]`, or one of them. Each passes
  as the editor sent it. An unknown name fails to load (P7).
- In strict mode both pass, whatever the profile says, as stable ACP v1
  has them (F1, ADR 2.1).
- When the agent was offered them and sends `fs/*` or `terminal/*`, the
  requests go to the editor as the agent wrote them, and the editor's
  answers, results or errors, go back to the agent. It is the same path
  every request of the agent's takes to the editor (ADR 3.1); only the
  editor's own requests get the host's ids (ADR 2.4).
- When ACP v2 is stable, brnr follows it, and this section is rewritten
  from what editors and agents speaking v2 do.
- A headless process offers the agent neither: its `initialize` has empty
  `clientCapabilities`, as no editor is there to serve them. That covers a
  session started headless, one forked in a headless process, and one
  taken over from an editor (`session resume --take-over`), which goes on
  in a new headless process with an agent of its own. A request for them
  there is answered "method not found". No headless session runs in an
  editor's process, where the agent may have been offered them:
  `session fork`, `session new --pid` and `session resume --pid` are
  refused there (ADR 7.1).
- What transparent means for the editor's process:
  - Observing (`event watch`, `event log`, `session status`,
    `event notify`, bridges) never changes the stream.
  - The named changes to the stream are: requests reach the agent with
    the host's ids (ADR 2.4); the editor's `fs` and `terminal`
    capabilities are dropped unless named or in strict mode (above); an
    editor's load of a session another process holds is refused unless a
    feature flag allows it (ADR 3.9, ADR 3.10); and the experimental
    actions a profile enables (ADR 7.1).
  - Headless operation is brnr as an ordinary ACP client (P1, P12).
- The real-editor checks for `fs` and `terminal` (the default, a profile
  that names them, strict mode, and a take-over) are in ADR 7.1. None has
  been run yet.

**Why.** ACP v1 has two client capabilities that v2 drops: `fs`
(`readTextFile`, `writeTextFile`) and `terminal`. With them, an agent
reads the editor's state of a file, unsaved changes included, has its
writes applied by the editor, and runs commands in the editor's terminals
(https://agentclientprotocol.com/protocol/v1/file-system). Without them,
it reads and writes the files on disk and runs commands in processes of
its own. F2 is the conventions brnr follows ahead of the spec, by
default; F1 is stable ACP to the letter, in strict mode; they meet by
mode, not by rank (ADR 1, #272). Agents that rely on `fs` and `terminal`
rely on what v2 takes away, and brnr discourages that, while strict mode
stays faithful to v1, the version brnr speaks (ADR 2.2). Resolves the
decision part of #69.

**Consequences.**

- By default, an agent run through `brnr acp` doesn't see the editor's
  unsaved buffers, its writes aren't applied by the editor, and it runs
  commands in processes of its own, where run directly it might do all
  three through the editor. That is the cost of following v2 ahead of it;
  a profile's `client_capabilities`, or strict mode, gives them back. The
  README says so.
- With them passed, the editor serves those requests
  (docs/threat-model.md, B3). What it answers them with (files as its
  buffers have them, unsaved changes and a `.env` included, and the
  commands' output) is in the session's raw ACP as it passed, not
  redacted. That is accepted: the threat model lists it as a gap brnr
  takes, not one to close.
- A session taken over from an editor, or resumed headless after it, goes
  on without them: the agent reads the files on disk, and what was unsaved
  in the editor isn't there.
- When the editor goes, its agent goes with it (ADR 3.1), so an agent
  never waits on an `fs` or `terminal` request no editor will answer. A
  request that comes while the process is ending gets "method not found"
  from the host.
- An `initialize` longer than 32 MiB goes to the agent as it is, with
  them (ADR 10.3). Editors don't send one that long.

**Considered.**

- Passing them in every mode: faithful to v1 (F1), but agents then build
  on what v2 drops, and following the conventions ahead of the spec is
  what the default is for (F2). Strict mode keeps the v1 behaviour for
  whoever wants it.
- Dropping them in strict mode too: strict mode is stable ACP to the
  letter, and v1 has them.
- Dropping them by default with no setting: strict mode would be the only
  way to get them, and it also turns off steering and fork (ADR 2.1).
- Dropping `terminal` only, keeping `fs`: both are stable v1 client
  capabilities that v2 drops, and the argument is the same for each; the
  profile names them one by one.
- Redacting file and terminal data in transcripts: redaction is by field
  (ADR 9.1), and a file's or a command's contents have no field a secret
  is known to be in, so it would be a guess at what is secret (P4), or
  the data dropped from the record of what passed (ADR 5.1). The
  transcripts are only the user's to read (P13).
- Serving `fs` and `terminal` from the host for headless sessions: brnr
  would read, write and run commands for the agent, a boundary of its own
  (docs/threat-model.md), where the agent does all three itself when not
  offered them.

## Tests

Run `cargo test --release adr_0002_`. Named claims, by section:

- 2.1: [tests/headless.rs](../../tests/headless.rs):
  `adr_0002_strict_mode_refuses_steering_and_fork`,
  `adr_0002_strict_mode_has_no_experimental_actions`.
- 2.2: [tests/cli.rs](../../tests/cli.rs):
  `adr_0002_a_start_in_an_acp_version_brnr_doesnt_speak_fails`: a start
  whose agent answers with version 999, 0, `"1"`, 1.5, 70000 or none,
  strict or not, exits 1 with the cause, sends nothing after
  `initialize`, and leaves no process or agent running; `session list`
  stops at `initialize` too; version 1 starts.
- 2.3: [tests/cli.rs](../../tests/cli.rs):
  `adr_0002_an_update_the_schema_doesnt_know_changes_nothing`;
  [src/schema.rs](../../src/schema.rs):
  `adr_0002_what_the_adapters_send_is_read_as_the_schemas_types`,
  `adr_0002_a_part_of_a_line_at_the_limit_reads_as_its_type`,
  `adr_0002_what_the_types_dont_take_is_left_to_value`,
  `adr_0002_stable_is_what_the_schema_has_without_its_unstable_features`.
- 2.4: [tests/headless.rs](../../tests/headless.rs):
  `adr_0002_an_editors_prompt_ends_only_with_its_own_answer`,
  `adr_0002_an_editors_request_with_a_host_requests_id_is_the_editors`,
  `adr_0002_ids_come_back_as_the_editor_wrote_them`,
  `adr_0002_the_editors_cancel_request_is_for_its_own_request`;
  [src/json.rs](../../src/json.rs):
  `adr_0002_an_id_is_found_where_it_is_and_nowhere_else`;
  [src/host/outbound.rs](../../src/host/outbound.rs):
  `adr_0002_ids_are_never_given_twice`,
  `adr_0002_an_answer_is_its_requests_once`,
  `adr_0002_a_second_answer_reaches_nobody`,
  `adr_0002_an_answer_for_a_closed_session_goes_to_whoever_asked`. Also
  [tests/security.rs](../../tests/security.rs):
  `adr_0003_acp_passes_bytes_unchanged` (ADR 3.1's), which checks that
  only the ids change.
- 2.5: [src/json.rs](../../src/json.rs):
  `adr_0002_a_line_is_read_to_its_limit_and_no_deeper`,
  `adr_0002_any_stack_takes_a_value_at_the_limit`;
  [src/host/fuzz.rs](../../src/host/fuzz.rs):
  `adr_0002_the_host_holds_nothing_of_a_line_past_the_limit`;
  [src/log.rs](../../src/log.rs):
  `adr_0002_a_message_at_the_limit_is_recorded_as_json`;
  [src/ctl/client.rs](../../src/ctl/client.rs):
  `adr_0002_an_answer_is_read_to_the_limit_and_no_deeper`,
  `adr_0002_a_status_with_an_error_at_the_limit_is_an_answer`;
  [tests/headless.rs](../../tests/headless.rs):
  `adr_0002_a_lone_surrogate_is_read`, `adr_0002_a_line_at_the_limit_is_read`,
  `adr_0002_a_line_past_the_limit_passes_through_unread`,
  `adr_0002_an_editors_rest_past_the_limit_passes_through_unread`,
  `adr_0002_headless_a_line_past_the_limit_is_dropped_and_said`,
  `adr_0002_an_answer_the_host_cant_place_reaches_the_agent`,
  `adr_0002_a_late_answer_to_a_cancelled_request_is_dropped`,
  `adr_0002_a_deep_request_is_listed`,
  `adr_0002_a_deep_request_is_shown`,
  `adr_0002_a_deep_request_is_followed`,
  `adr_0002_a_deep_request_is_notified`,
  `adr_0002_a_deep_event_is_caught_up`,
  `adr_0002_a_deep_request_is_logged`;
  [src/host/outbound.rs](../../src/host/outbound.rs):
  `adr_0002_an_answer_to_an_unknown_id_goes_on_as_it_came`;
  [src/host/approvals.rs](../../src/host/approvals.rs):
  `adr_0002_a_late_answer_from_the_editor_is_dropped`;
  [src/transcript.rs](../../src/transcript.rs):
  `adr_0002_a_record_is_read_to_the_limit_and_no_deeper`.
- 2.6: [tests/security.rs](../../tests/security.rs):
  `adr_0002_the_editors_stdout_ends_with_the_agents`,
  `adr_0002_stderr_comes_as_it_is_written`,
  `adr_0002_stdout_and_stderr_end_on_their_own`;
  [tests/headless.rs](../../tests/headless.rs):
  `adr_0002_a_timed_out_start_shows_the_line_stderr_is_in`,
  `adr_0002_the_editors_write_fails_once_the_agents_stdin_closes`;
  [src/host/fuzz.rs](../../src/host/fuzz.rs):
  `adr_0002_brnrs_own_lines_end_with_the_agents_stdout`,
  `adr_0002_stderr_goes_on_before_its_line_ends`,
  `adr_0002_the_editors_stdin_ends_with_the_agents`. Not covered by a Rust
  test: the editor's stdin closing when the agent exits on its own, which
  takes a stop's path (`close_agent_stdin`).
- 2.7: [tests/headless.rs](../../tests/headless.rs):
  `adr_0002_fs_and_terminal_reach_the_agent_in_strict_mode_or_when_named`,
  `adr_0002_the_agents_file_and_terminal_requests_reach_the_editor`,
  `adr_0002_headless_sessions_offer_neither`. The real-editor checks
  (ADR 7.1) have no automated test.
