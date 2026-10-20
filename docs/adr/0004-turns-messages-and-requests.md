# 4. Turns, messages and the agent's requests

Draft 2026-10-20. Implemented. Factor: F3 (ADR 1).
Brings together former ADRs 17, 56, 18, 19, 71, 27, 68, 29 and 30
(docs/adr/README.md maps them).

How a headless session carries the user's messages into turns, and how the
agent's requests to the user reach someone who can answer them: message
ids, held and steered messages, cancelling, a turn's tokens, approvals and
elicitations, the requests brnr sends the agent, and logging in. ACP has
none of message ids, held messages or waiting; they are the thin layer brnr
adds where ACP has nothing (F3, P2), and the agent's own mode, requests and
login stay the agent's.

## 4.1 Message ids, and when a turn ends

- Every message the host accepts gets an id, `m<n>`.
  `prompt send --context` gets none: context has no turn of its own.
- The id is in `prompt send`'s response, in `session new --json` and
  `session resume --json` for the start's prompt, in `user_message` when
  the message is actually sent, and in the `turn_ended` of the turn that
  carried it.
- A turn can carry more than one message: a steered one has no turn of its
  own (ADR 4.2). `turn_ended` lists them, `messages: [m1, m3]`.
- A steer is a request of its own, `_session/steering`, answered apart from
  the `session/prompt` of the turn it goes into, in either order. A turn
  that ends while steers sent into it are unanswered has its `turn_ended`
  held back until the agent has answered them all, or they are dropped:
  until then, which messages it carried isn't known.
- An `injected` answer puts the message in that turn: its `user_message`
  names the turn's prompt, and the `turn_ended`, when it goes, lists it. A
  `promptRequired` answer holds the message as ADR 4.2 says, ahead of what
  is held. An error, or any other answer, is a `message_dropped`, its `reason`
  `steer` (ADR 5.2). Then the held `turn_ended` goes, and then the next
  held message.
- While a turn's `turn_ended` waits, no other turn starts. `prompt send`
  and `prompt send --interrupt` are held, the interrupt ahead of what is
  held as ADR 4.2 has it, but with no turn to cancel (status `held`, no
  `session/cancel`). `prompt send --steer` is steered into that turn.
- `prompt cancel`, closing the session and the agent exiting drop
  unanswered steers (`message_dropped`, its `reason` `cancel`, `close` or `exit`),
  and then the held `turn_ended` goes. Every accepted message ends in one
  `turn_ended` that lists it, or in one `message_dropped`, whatever the
  order of the answers.
- `prompt send --steer --wait` for a message answered `injected` after its
  turn's own answer exits as that turn does. What the agent said before it
  answered isn't shown as the message's reply: `--wait` prints what follows
  the message's `user_message`. The transcript has it.
- `steer_after_turn` in the host log is for an `injected` answer with no
  turn at all. A steer brnr sends only while a turn runs or waits, so it
  can't get one; if it does, the message is dropped, `reason` `steer`, never
  left without an end. The editor's own steers (ADR 7.1) have no message
  id; one answered with no turn is logged `steer_after_turn`.

**Why.** `prompt send --wait` must know which turn answers its message,
even when the message is held, steered into a running turn, or sent after
an interrupt. A steer's answer and its turn's can come in either order.
If `turn_ended` went at once, a late `injected` answer would find no turn,
and a waiter would wait on an idle session until its timeout (P3).

**Consequences.** An agent that never answers a steer keeps the turn's
`turn_ended`, and what is held behind it, waiting, as held messages wait
for a turn; `prompt cancel` or `session close` ends it.

**Considered.**

- Matching a message to its turn by its text: fragile.
- Emitting `turn_ended` at once, and a second event for the late steer:
  two endings for one turn, or a new event for waiters and bridges to
  learn, for what is one turn's list of messages.
- Dropping a late `injected` steer as `message_dropped`: it would say a
  message the agent took never went (P3 the other way).

## 4.2 What sending a message does

For headless sessions:

| | Idle | A turn is running |
|---|---|---|
| `prompt send` | sent as a prompt | held; sent as its own turn when the running one ends, in order |
| `prompt send --steer` | sent as a prompt, not steered (as `promptRequired` would have it) | injected into the running turn; refused if the agent doesn't advertise steering (P7), or in strict mode |
| `prompt send --interrupt` | sent as a prompt; held, ahead of what is held, while the last turn's steers are unanswered (ADR 4.1) | `session/cancel`, then sent ahead of what is held |
| `prompt send --context` | appended to the next prompt | appended to the next prompt |

- Each held message is its own prompt and turn, in the order sent.
  Interrupts go ahead of held messages, and keep their own order.
- A second `session/prompt` is never sent while one runs.
- `_session/steering` is an ACP extension, a convention both adapters
  implement alike: used by default, refused in strict mode (ADR 2.1).
- The response's status is `delivered`, `held`, `steered` or
  `interrupting`.
- On an editor's session the editor controls its turns. `prompt send` is
  ADR 7.1's experimental action there: refused while a turn runs, and never
  held, steered or interrupting.

**Why.** ACP doesn't say what a prompt sent mid-turn means, and the
adapters differ: claude-agent-acp 0.85.1 queues it as a turn of its own
(`_meta.claudeCode.promptQueueing`); codex-acp 2.1.1 replaces the active
prompt with it and stops following the first one's turn (read from the
code, not run). The CLI of a headless session is a contract (P12) and
can't depend on either (P2). Both adapters implement steering,
`_session/steering`, advertised as `_meta.steering.supported` in
`initialize`. With `_meta.steering.idleBehavior: "promptRequired"`, a
steer answers `injected` while a turn runs, its output streaming within
that turn, and `promptRequired` when the session is idle, leaving the
client to send it as a normal prompt.

**Considered.**

- Passing a mid-turn prompt through and saying per agent what it does: the
  CLI couldn't promise one behaviour, and an adapter release could change
  what `prompt send` does.
- Steering by default where the agent offers it, holding elsewhere: the
  default's meaning would vary by agent.
- Sending everything held during a turn as one prompt when it ends: closer
  to how Claude Code batches queued messages, but `--wait` would no longer
  map one message to one turn.

## 4.3 What cancelling does with held messages

`prompt cancel` cancels the running turn (`session/cancel`, answering the
agent's pending permission requests `cancelled`, as ACP requires, and its
elicitations `cancel`, ADR 4.5). It drops the session's held messages, each
with a `message_dropped` event (`by: cancel`) and listed in its response.
`--keep-held` keeps them, to go out as usual.

**Why.** A user who cancels wants the session to stop. Held messages that
went out as usual would start the next turn at once.

**Considered.**

- Cancelling the turn and letting held messages go out as usual:
  surprising, since "stop" would start the next held message.
- Cancelling the turn and pausing the queue: a paused state to explain.

## 4.4 A turn's tokens are what the agent reported

- The host reads `result.usage` of a `session/prompt` answer as a
  `serde_json::Value`, as ADR 2.3 does for the unstable `fork` capability.
  The schema crate's unstable feature `unstable_end_turn_token_usage`
  stays off.
- `turn_ended` and the session's `last_turn` have a field `tokens`, after
  `error`:
  - `null` when the answer has no `usage`, or its `usage` isn't an object:
    the tokens are unknown.
  - Otherwise `{"total", "input", "output", "thought", "cached_read",
    "cached_write"}`, from `totalTokens`, `inputTokens`, `outputTokens`,
    `thoughtTokens`, `cachedReadTokens` and `cachedWriteTokens`. Each is
    the agent's number when it is a whole number that isn't negative (a
    `u64`), and `null` otherwise: left out, null, a string, negative, a
    fraction, or written with an exponent.
- No count is calculated: no total from the parts, nothing from
  `usage_update`'s `used` or `size`, and no sum over turns.
- brnr says nothing of what the counts cover. They are the counts the
  agent reported for the turn, as the agent counts them: brnr doesn't say
  whether subagents or earlier model calls are in them.
- The answer goes to the editor as it came, `usage` included (P1). Only
  brnr's copy is read.
- `session status` has `last_turn`, in `--json` and in text.
  `prompt send --wait --json` and `session new --wait --json` have `tokens`
  beside `stop_reason` and `error`, from the turn's `turn_ended`.
- Text (P5): `turn ended: end_turn (control); tokens: 1.2k total, 1.0k
  input, 234 output, 50 thought, 500 cached read, 20 cached write`, and the
  status has `last turn ended: end_turn; tokens: …`. Each count the agent
  gave is shown, and one it didn't give is left out, as the `usage` line
  leaves out a cost it doesn't have (ADR 5.3). With none given, the line
  ends after `(control)`. The numbers are shown as the `usage` line shows
  them (`950`, `12.3k`, `1.2M`); `--json` has them exact.
- Strict mode doesn't change it: to read what the agent said is to
  observe, which affects no party (P1, ADR 2.1). An agent that keeps to
  stable ACP sends no `usage`, and its turns have `tokens: null`.

**Why.** An agent can say in its answer to `session/prompt` how many tokens
the turn used. ACP's schema 1.10.2 has this as `PromptResponse.usage`, only
behind an unstable feature: `totalTokens`, `inputTokens` and
`outputTokens` required, `thoughtTokens`, `cachedReadTokens` and
`cachedWriteTokens` optional, and `_meta`. brnr builds the crate without
unstable features (ADR 2.3), so the counts were only in the raw ACP
(ADR 5.1). What brnr shows is what the agent said, read once (P5), with
nothing worked out or guessed (P4). What a count covers is the agent's to
say: research of October 2026 (research#7) found that the Codex adapter
reports its last model call, and that Claude's ACP counts leave out
subagents. Neither is in the protocol.

**Consequences.**

- Every `turn_ended` and `last_turn` has one field more, `null` for agents
  that don't send `usage`.
- If ACP changes the field (a name, a type, where it is) or stabilizes it,
  brnr sees the changed form as unknown counts or no `usage` until it is
  read again here; it doesn't fail. A stabilized `usage` can then be read
  with the schema's type.
- `tokens` is one of the names #150 and #151 settle, as ADR 10.2's are.

**Considered.**

- Enabling `unstable_end_turn_token_usage`: against ADR 2.3. The feature
  can change or go before it is stable, and with it the crate's types
  would no longer be what strict mode means by stable ACP (ADR 2.1).
- Calling the field `usage`: it would clash with the `usage` event and the
  status's `usage`, which are the context window and the cost (ADR 5.3).
- Taking a turn's tokens from `usage_update`, or a total from its `used`:
  that is how full the context window is, not what a turn used.
- Passing the agent's `usage` object on as it is, camelCase: brnr's events
  are snake_case, and a copy would pass on any value, of any type. Each
  count read on its own is the agent's number or unknown.
- A total from `input` and `output` where the agent gives no `total`: a
  guess (P4). What a total includes is the agent's to say.

## 4.5 Approvals and elicitations: the agent's mode is the policy

A permission request (`session/request_permission`) and an elicitation
(`elicitation/create`, the agent asking for a form's values or for a URL
to be opened) go through one mechanism, with ACP's own words for each
(P1, P2).

The policy:

- How much the agent asks is its mode (P2), set at start
  (`session new --mode`, or `mode` in the profile's headless part, applied
  before the first prompt) or later (`config set --mode`). brnr has no
  approval policy of its own: a config with a `permissions` key fails to
  load, and `brnr doctor` says so.
- brnr always asks when the agent does. Headless, a permission request is
  held with a handle `p<n>`, and an elicitation with a handle `e<n>`, until
  someone answers it: a command below, or a bridge. brnr never answers one
  for the user (P4), and never declines an elicitation for the user, but
  by the `response_timeout` the user set (below).
- With no editor attached, brnr is the agent's client: permission requests
  and elicitations as here, anything else answered "method not found".
- Headless, `initialize` carries `clientCapabilities.elicitation:
  {form: {}, url: {}}`, built with the schema's types (ADR 2.3): a mode is
  advertised with an object, not a boolean, and the schema reads `false`
  as not advertised.

Events:

- `permission_request`, and `permission_resolved`, which says `allowed` or
  `rejected`, and which kind (ADR 8.1).
- `elicitation_request` carries the request's `text` (ACP's `message`),
  `mode` (`form`, `url`, or another the agent names), `scope` (`session`,
  `tool_call` or `request`) with the `tool_call_id` or `scope_request_id`
  it names, the form's
  `schema` (the `requestedSchema` as sent), and a URL mode's `url` and
  `elicitation_id`; `owner` says `headless` or `editor`.
- `elicitation_resolved` carries the `action` sent (`accept`, `decline`,
  `cancel`, or an editor's other action), the `content` an accept carried,
  and who and why, as `permission_resolved` has them (ADR 5.9): `by`
  `editor`, or `control` with the connection or bridge that answered
  (`peer`) or `reason` `cancel` or `timeout`.
- `elicitation_completed` is the agent's `elicitation/complete`, the URL's
  flow done, with the `elicitation_id` and the handle it belonged to.
- `event wait --for permission` and `--for elicitation` wait for one
  (ADR 8.3). `event notify` runs its command for `permission_request` and
  `elicitation_request` by default (ADR 5.8). `prompt send --wait`
  announces one on stderr, with how to see it and answer it.
- The events' names follow ADR 5.9 (snake_case, the agent's values as
  sent); when #149 and #150 land, their shapes follow those decisions.

Listing and showing:

- `permission requests [<session>]` and `elicitation requests [<session>]`
  are tables. With no session, they list the requests of each process
  that answers; one that doesn't is passed over, as `process list` and
  `session list` pass it over (ADR 3.9), and said on stderr (P3).
  `elicitation requests` has SESSION, REQUEST, OWNER, MODE, MESSAGE.
- `permission show <session> <request>` shows one in full: the tool, its
  kind, paths, the command or input, and for edits a diff made from the
  tool call's `oldText`/`newText` with a small line diff (a very large one
  shows the new text, saying so). It warns about a command with control
  characters in it (P8).
- `elicitation show <session> <request>` shows one in full: the message,
  mode and scope, a form's fields with their types, constraints and
  choices, and a URL as the agent sent it. brnr never opens the URL (P8):
  the user opens it, and then `accept`s.
- Each `show` says what the timeout will send before it happens, and, of
  an editor's request, that it is waiting in the editor and whether it can
  be answered here (`answerable`, `why_not`).

Answering a permission request:

- `permission allow` and `reject` take `<session> <request>`: brnr doesn't
  pick the only request waiting (P4).
- Each answers with the option of one ACP kind: `allow` with
  `allow_once`, `allow --always` with `allow_always`, `reject` with
  `reject_once`, `reject --always` with `reject_always`, whatever their
  order in the request. No other kind stands in for it, and `reject` never
  answers `cancelled`: a request without that kind, or with two options of
  it, fails, listing the options, and nothing is answered (P4, P7).
- `--option-id <id>` names the option instead, for a kind brnr doesn't know
  or two options of one kind. One of an ACP kind must be on the verb's side
  (`allow_*` for `allow`), and with `--always` of the always kind; one of a
  kind brnr doesn't know goes with either verb.
- `prompt cancel` is how a request is answered `cancelled`, as ACP has it
  (ADR 4.3).

Answering an elicitation, with ACP's response actions as verbs (ADR 8.1):

```text
brnr elicitation requests [<session>] | show <session> <request>
                | accept <session> <request> [--field <k>=<v>]... | -
                | decline <session> <request> | cancel <session> <request>
```

- `accept` takes the form's values as `--field name=value` (repeated for a
  multi-select) or as one JSON object on stdin (`-`).
- The host checks them against the request's schema before anything is
  sent (P4, P7): every field named in the schema, the required ones given,
  each value of its type (a `--field` string is read as the type asks: `42`
  for an integer, `true` for a boolean), within `minimum`/`maximum`,
  `minLength`/`maxLength` and `minItems`/`maxItems`, and one of `enum`,
  `oneOf` or the items' choices where there are any. A value that doesn't
  fit fails the command, naming the field, and nothing is sent.
- `pattern` and `format` are passed on as given: checking them takes a
  regex engine brnr doesn't have (a crate needs a reason), and the agent
  checks them itself.
- The response is written with the schema's types
  (`CreateElicitationResponse`), so an accept's `content` has ACP's value
  types.
- `accept` of a URL elicitation takes no fields. One of a mode brnr
  doesn't know can only be declined or cancelled.
- A request-scoped elicitation (a login, outside any session) is listed
  with no session (`--pid <pid>` in the SESSION column), and named by its
  process, `--pid <pid>` in place of `<session>`, as `event watch` names
  one.

Socket and bridges (ADR 5.7):

- `pending` answers with `elicitations` beside `pending`, each in full,
  with `answerable` and `why_not`.
- `allow` and `reject`, with `always` and `option_id`, answer a permission
  request (ADR 8.1). `elicitation {session, request, action:
  accept|decline|cancel, content?}` answers an elicitation, `session` null
  for a request-scoped one. The turn's `cancel` keeps its name.

One timeout:

- `response_timeout` in the profile's headless part, and
  `--response-timeout <s>`, is the one timeout for the user's responses:
  permission requests and elicitations alike. It has no default: without
  it, a request waits for as long as nobody answers. An unattended run
  must set it.
- A permission request nobody answered in time is answered with its
  `reject_once` option. One without it has its turn cancelled
  (`session/cancel`), so it and every request pending in the session are
  answered `cancelled`, as ACP requires. Never `reject_always`, and never
  an allow.
- An elicitation nobody answered in time is answered `cancel`, `by`
  `control`, `reason` `timeout`: never `decline`, which says the user said no, nor `accept`.
- `prompt cancel` answers the session's pending elicitations `cancel`, `by`
  `control`, `reason` `cancel`, as it answers its permission requests `cancelled`: the turn
  they were asked in is gone, and nothing is left waiting unsaid (P3).

A request the agent gives up on (#162):

- The agent's `$/cancel_request` for a permission request or an
  elicitation it sent ends the request, in brnr's role either way. It is
  no longer waiting: not in `permission requests`, `elicitation requests`
  or a bridge's `pending`, and not counted in the session's `waiting`.
  Its `permission_resolved` (`answer: cancelled`) or
  `elicitation_resolved` (`action: cancel`) says `by: agent`. An answer
  to it after that, from a command or a bridge, fails as one to a request
  that isn't waiting, and nothing is sent.
- Headless, brnr is the receiver ACP names, and answers the request the
  agent cancelled, as ACP requires: a permission request with the outcome
  `cancelled`, an elicitation with the action `cancel`, the answers a
  cancelled request has.
- In an editor's session the notification reaches the editor unchanged
  (P1), and the editor's answer reaches the agent unchanged. The event
  says `by: agent` once; the editor's answer makes none.
- A `$/cancel_request` for a request already answered, or for an id brnr
  holds no request of, ends nothing: there is nothing waiting to end.
- A turn that ends while a request of it is still waiting is the agent's
  fault: ACP has the agent cancel what it no longer waits for. brnr
  doesn't end the request for it (F1, ADR 4.6). The request waits, as any
  request does, until it is answered, `response_timeout` passes or
  `prompt cancel` cancels it; meanwhile the session is `waiting`, and
  `stop_when_idle` doesn't count (ADR 3.8). What the agent does wrong
  shows, as it would without brnr.
- A request brnr doesn't hold (an editor's `fs` or `terminal` request) is
  the editor's: the agent's `$/cancel_request` for it reaches the editor,
  and brnr ends nothing.

Editor sessions (ADR 7.1):

- A permission request or an elicitation passes through to the editor
  unchanged. `elicitation_request` is emitted with `owner: editor`, so
  watchers and bridges see it; the editor's answer is
  `elicitation_resolved`, `by` `editor`.
- Answering from the side channel is the experimental action `permission`
  or `elicitation`, refused unless the editor's profile enables it.
  `elicitation` is refused in strict mode too. When it is taken, the
  request is withdrawn from the editor with `$/cancel_request` and the
  editor is told in the session ("Accepted via brnr", "Declined via
  brnr"); the editor's late answer is dropped, and it is told who answered
  first, as `permission` does.

Status: `session status` counts `elicitations` beside `pending`, and a
session with one waiting is `waiting`.

**Why.** A policy of brnr's own for headless approvals (ask, auto-allow or
auto-deny, by ACP tool kind) overlapped the agent's own mode, which already
says when to ask at all (Claude's `acceptEdits` or `bypassPermissions`),
and the two composed confusingly: mode `default` with auto-allow approved
everything one round trip at a time, `plan` with auto-allow approved
leaving plan mode, and neither setting knew of the other. Elicitations are
the same question, the agent waiting on the user mid-turn. Declined at
once, a user on a bridge could answer "may I edit this file?" but never
"which option do you want?". In `agent-client-protocol-schema` 1.10.2
neither `elicitation/create` nor `elicitation/complete` is behind an
unstable feature, so by ADR 2.1 and ADR 2.3 both are stable v1. A timeout
never allows: it would approve whatever an unattended agent asked for. What
a timeout sends differs because ACP's answers differ: a permission request
has no outcome for "nobody chose"; an elicitation has `cancel` for exactly
that. One number covers both: a second would be one more setting to get
wrong for the same question.

`$/cancel_request` is stable ACP (no unstable feature gates it in
`agent-client-protocol-schema` 1.10.2), and ACP has its receiver answer
the request it names. Passed over, as brnr does now, a request the agent
gave up on stays listed: the session is `waiting` and never idle, and
`permission allow` answers an agent that no longer asks (#162). A correct
client acts on it (F1). A turn that ends with a request unanswered is
another case: nothing in ACP ends the request, and ending it would be
brnr making up for the agent (F1), as answering a request the agent never
answers would be (ADR 4.6).

**Consequences.**

- claude-agent-acp turns AskUserQuestion on for headless sessions once it
  sees `elicitation.form`, and MCP servers' elicitations and URL logins
  reach the user. Not covered by a Rust test: it needs the real adapter
  with credentials, past what `adapters/check.py` can run;
  `adr_0004_headless_advertises_elicitation` checks what brnr advertises.
- A headless turn waits on an elicitation. `response_timeout` has no
  default (P4: brnr doesn't pick a deadline for the user), so an
  unattended run must set it, and the README, interface.md and the skill
  say so.
- Bridges that list `pending` see an `elicitations` key; one that answers
  elicitations sends `elicitation`.
- Both adapters can send `$/cancel_request` for a request of their own:
  as their code reads (claude-agent-acp 0.85.1, codex-acp 2.1.1), Claude's
  permission requests and elicitations go with the abort signal of Claude
  Code's tool call, and Codex's approvals with the turn's cancellation
  signal, and the ACP SDK sends `$/cancel_request` when one aborts.
  Neither has been seen to end a turn with a request open.
  `adapters/check.py` drives no turn that asks (that needs the real
  agents, with credentials), so neither is run there.

**Considered.**

- Keeping brnr's own policy beside the mode: agent-agnostic rules by tool
  kind and auditable auto-answers in the transcript, but two policies for
  one question.
- `permission requests -v` for the details: `requests` stays a table,
  `show` is one request in full.
- Allowing on timeout: see above.
- Advertising `form` only: URL mode is how MCP servers log in (OAuth), and
  showing a URL without opening it is as safe as showing a command.
- A timeout of its own for elicitations (`elicitation_timeout`): two
  settings for one question; nothing found that needs them to differ.
- Naming the one timeout `permission_timeout`: it would name one of the
  two things it times out.
- A timed-out elicitation answered `decline`: that is the user's "no", and
  nobody said it.
- Checking the values in the CLI: the host holds the schema and the
  bridges answer too, so the check is the host's, once.
- Opening a URL (`open`, `xdg-open`): the agent's text on a command line
  (P8), and a URL the user hasn't seen.
- `brnr elicitation answer <action>`: ADR 8.1's rule is ACP's verbs.
- Holding an elicitation of a mode brnr doesn't know as nothing: it would
  wait unseen; held, it can be shown, declined or cancelled.
- Passing the agent's `$/cancel_request` over, as ACP lets a receiver of a
  `$/` notification do: brnr can act on it, and a request left listed
  misleads whoever answers it (P3).
- Answering the cancelled request with the error `-32800`: ACP allows it,
  but a permission request and an elicitation each have an answer for a
  cancelled request, and brnr already sends those for `prompt cancel`.
- Ending the requests of a turn that ended, answered `cancelled`: it makes
  up for an agent that doesn't follow ACP (F1), and hides that it doesn't.
- Waiting until either adapter is shown to cancel a request or end a turn
  with one open: the rule is ACP's, whichever agent does it.

## 4.6 Requests the agent answers

Setting the mode or a config option (`config set`), forking
(`session fork`) and closing (`session close`) are answered when the agent
answers, on the same connection (matched by `req_id`); the host doesn't
block meanwhile. `brnr` waits up to 120 seconds for them, since switching
model or forking can take a while.

**Why.** These are the agent's to do (P2), and some take a while; the host
serves its other sessions and peers meanwhile.

The host has no timeout of its own for them, and never answers one in
the agent's place. A request the agent never answers (or answers in a line
brnr can't read, ADR 2.5) stays waiting: only the caller gives up, `brnr`
after its 120 seconds, saying so (P3), and a bridge when it chooses to.

**Consequences.** A bridge whose request the agent never answers waits as
long as it keeps waiting, and so does the `session/close` of a
`--take-over` (ADR 3.9) once `brnr` has stopped waiting.

**Considered.**

- The host answering such a request with an error after a time of its
  own: an agent that never answers doesn't follow ACP and JSON-RPC, and
  it isn't brnr's to compensate for that (F1). What the agent does wrong
  shows, as it would without brnr.

## 4.7 Authentication

- By default the host doesn't run `authenticate`. A start that fails
  because the agent needs a login fails with the agent's methods, each by
  name and by the id `--auth` takes, and a hint: log in with the agent's
  own CLI first, or name a method that needs no terminal with
  `--auth <id>` (`auth_hint` in src/host/requests.rs).
- `auth = "<method id>"` in a profile's headless part, or
  `session new --auth <id>` (and `session resume`), runs `authenticate`
  with that method after `initialize`, before the session opens. The user
  names a method they know works without a terminal; brnr never picks one
  (P4), and needs to know nothing about any adapter (P2).
- A method the agent doesn't offer fails the start before `authenticate`,
  naming the ones it does (P7). A method that tries a browser anyway runs
  into the start's timeout.
- With an editor, logging in is the editor's business.

**Why.** ACP's `authenticate` runs one of the login methods the agent
offers in `initialize`. Most log in through a terminal or a browser, which
a headless process has neither of. Not all: codex-acp's `api-key` reads
`OPENAI_API_KEY` from the environment. Nothing in ACP tells the two kinds
apart for the adapters brnr ships (neither uses ACP's typed methods), and
codex-acp's other method, `chat-gpt`, opens a browser.

**Considered.**

- Never authenticating: headless codex with an API key, in CI say, would
  need an interactive login first.
- brnr choosing a method that looks non-interactive: it can't tell, and a
  guess could open a browser on a server.

## Tests

Run `cargo test --release adr_0004_`. Named claims, by section:

- 4.1: [tests/headless.rs](../../tests/headless.rs):
  `adr_0004_steer_goes_into_the_running_turn`,
  `adr_0004_steer_answered_after_its_turn_is_in_that_turn`,
  `adr_0004_steer_refused_after_its_turn_is_dropped`,
  `adr_0004_steer_never_answered_is_dropped_by_close`,
  `adr_0004_interrupt_waits_for_the_steers_into_the_turn_before`;
  [tests/cli.rs](../../tests/cli.rs):
  `adr_0004_start_json_gives_the_prompts_message`;
  [src/host/turns.rs](../../src/host/turns.rs):
  `adr_0004_a_turn_ended_waits_for_its_steers_and_so_does_what_is_held`,
  `adr_0004_a_steer_injected_while_its_turn_runs_is_in_that_turn`,
  `adr_0004_a_steer_turned_back_goes_next_ahead_of_what_is_held`,
  `adr_0004_a_refused_steer_is_dropped_before_its_turn_ends`,
  `adr_0004_an_interrupt_while_steers_wait_is_held_with_no_turn_to_cancel`,
  `adr_0004_a_dropped_steer_lets_its_turn_end`,
  `adr_0004_the_editors_turns_are_its_own`.
- 4.2: [tests/headless.rs](../../tests/headless.rs):
  `adr_0004_interrupts_keep_their_order`,
  `adr_0004_send_while_a_turn_runs_is_held`,
  `adr_0004_steer_without_a_turn_is_a_prompt`,
  `adr_0004_steer_needs_the_agents_steering`;
  [src/host/turns.rs](../../src/host/turns.rs):
  `adr_0004_interrupts_go_ahead_of_held_messages_in_their_order`,
  `adr_0004_context_goes_with_the_next_prompt`,
  `adr_0004_queue_drops_held_messages_and_interrupts`;
  [src/host/fuzz.rs](../../src/host/fuzz.rs):
  `adr_0004_a_bridge_sends_through_the_sessions_turns`.
- 4.3: [tests/cli.rs](../../tests/cli.rs):
  `adr_0004_cancel_drops_held_messages_and_says_so`,
  `adr_0004_cancel_can_keep_held_messages`;
  [src/host/turns.rs](../../src/host/turns.rs):
  `adr_0004_cancel_drops_what_is_held_and_unanswered_steers`.
- 4.4: [tests/cli.rs](../../tests/cli.rs):
  `adr_0004_a_turns_tokens_are_what_the_agent_reported`,
  `adr_0004_a_count_the_agent_didnt_give_is_null`,
  `adr_0004_a_turn_without_usage_has_its_tokens_unknown`;
  [tests/security.rs](../../tests/security.rs):
  `adr_0004_the_editor_gets_the_answers_usage_as_the_agent_sent_it`;
  [src/render.rs](../../src/render.rs):
  `adr_0004_text_shows_the_counts_the_agent_gave`.
- 4.5: [tests/security.rs](../../tests/security.rs):
  `adr_0004_an_unanswered_request_waits`,
  `adr_0004_a_timeout_never_allows`,
  `adr_0004_a_request_is_answered_only_in_its_session`,
  `adr_0004_a_timeout_cancels_never_accepts`;
  [tests/headless.rs](../../tests/headless.rs):
  `adr_0004_allow_during_stop_fails`,
  `adr_0004_an_editors_elicitation_is_seen_and_reaches_the_editor`;
  [tests/cli.rs](../../tests/cli.rs):
  `adr_0004_show_explains_a_permission_request`,
  `adr_0004_show_escapes_a_spoofed_command`,
  `adr_0004_requests_passes_over_a_silent_process`,
  `adr_0004_headless_advertises_elicitation`,
  `adr_0004_requests_and_show_list_an_elicitation`,
  `adr_0004_accept_checks_fields_against_the_schema`,
  `adr_0004_decline_and_cancel`,
  `adr_0004_url_accept_is_followed_by_complete`,
  `adr_0004_wait_for_elicitation`,
  `adr_0004_send_wait_reports_an_elicitation`,
  `adr_0004_a_bridge_sees_the_elicitation`,
  `adr_0004_a_request_scoped_elicitation_is_answered_by_pid`,
  `adr_0004_prompt_cancel_cancels_the_sessions_elicitations`;
  [src/render.rs](../../src/render.rs):
  `adr_0004_elicitation_events_have_lines`;
  [src/host/approvals.rs](../../src/host/approvals.rs):
  `adr_0004_handles_are_never_used_again`,
  `adr_0004_every_way_a_request_ends_leaves_nothing_behind`,
  `adr_0004_an_answer_that_cant_be_sent_leaves_the_request_waiting`,
  `adr_0004_a_timeout_rejects_once_or_cancels_the_turn`,
  `adr_0004_the_agents_cancel_ends_only_what_waits`. A request the agent
  cancels with `$/cancel_request`: [tests/cli.rs](../../tests/cli.rs):
  `adr_0004_a_request_the_agent_cancels_ends_by_the_agent`,
  `adr_0004_a_turn_that_ends_leaves_its_request_waiting`;
  [tests/headless.rs](../../tests/headless.rs):
  `adr_0004_an_editor_gets_the_agents_cancel_and_answers_it`.
- 4.6: [tests/cli.rs](../../tests/cli.rs):
  `adr_0004_agent_requests_wait_for_their_answer_without_blocking_the_host`.
- 4.7: [tests/cli.rs](../../tests/cli.rs):
  `adr_0004_login_needed_is_explained`,
  `adr_0004_auth_runs_the_login_method_named`.
