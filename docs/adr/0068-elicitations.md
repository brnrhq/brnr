# 68. Elicitations are held and answered as approvals are

Accepted 2026-10-14. Implemented. Amends 4, 21, 27, 33, 35, 36, 58 and 63.
Resolves #195.

## Context

ACP's `elicitation/create` is the agent asking the user for structured
input: a form made from a JSON schema (`mode: form`), or a URL to open
(`mode: url`), with a message, scoped to a session, to a tool call in it, or
to a JSON-RPC request outside any session (a login). The agent sends
`elicitation/complete` when a URL's flow has finished. In
`agent-client-protocol-schema` 1.10.2 neither is behind an unstable
feature, so by ADR 41 and 43 both are stable v1.

What brnr did at 7b5a6cb, traced from the code:

```text
 Headless (brnr is the client)
 brnr host                                       agent
  │ 1 initialize, clientCapabilities: {} ────────▶│  (requests.rs, begin_headless_start)
  │                                               │ 2 no elicitation.form: claude-agent-acp
  │                                               │   turns AskUserQuestion off; MCP
  │                                               │   elicitations are auto-declined
  │◀── 3 elicitation/create (one sent anyway) ────│
  │ 4 {"action":"decline"} ──────────────────────▶│  (acp.rs, answer_as_client; ADR 27)

 Editor session (brnr acp)
 editor ── 1 initialize {elicitation:{form,url}} ──▶ brnr ──▶ agent   (passed through)
 agent  ── 2 elicitation/create ──▶ brnr ──▶ editor      no event, no bridge, not in
                                                         any `requests` list
```

A permission request, by contrast, goes through one mechanism end to end
(ADR 27, 63): `agent_request` gives it a handle (`p<n>`) and holds it in
`agent_requests` with a deadline; `permission_request` reaches bridges,
`event watch`, `event wait --for permission` and `prompt send --wait`'s
stderr; the socket's `pending`, `allow` and `reject` list and answer it
through `resolve_permission`, which responds to the agent, withdraws it from
an editor (ADR 4) and emits `permission_resolved`; `fire_permission_timers`
answers one nobody did. So a user on a bridge could answer "may I edit
this file?" but never "which option do you want?".

## Decision

Elicitations go through the same mechanism as approvals, with ACP's own
words for them (P1, P2):

- **Headless, brnr advertises both modes.** Its `initialize` carries
  `clientCapabilities.elicitation: {form: {}, url: {}}`, built with the
  schema's types (ADR 43): the protocol advertises a mode with an object,
  not a boolean (the issue's `form: true`), and the schema reads `false`
  as not advertised. An `elicitation/create` is held as a permission request
  is, with a handle `e<n>`, and answered only when someone answers it (P4):
  never declined for the user.
- **Events.** `elicitation_request` carries the request's `message`,
  `mode` (`form`, `url`, or another the agent names), `scope` (`session`,
  `tool_call` or `request`) with the `tool_call` or `scope_request` it
  names, the form's `schema` (the `requestedSchema` as sent), and a URL
  mode's `url` and `elicitation_id`; `owner` says `headless` or `editor`.
  `elicitation_resolved` carries the `action` sent (`accept`, `decline`,
  `cancel`, or an editor's other action), the `content` an accept carried,
  and `by` (a connection or bridge, `editor`, `cancel` or `timeout`).
  `elicitation_completed` is the agent's `elicitation/complete`: the URL's
  flow is done, with the `elicitation_id` and the handle it belonged to.
  `event wait --for elicitation` waits for one (ADR 21); `event notify`
  runs its command for `elicitation_request` by default, as it does for
  `permission_request` (ADR 36); `prompt send --wait` announces one on
  stderr with how to see it. The events' shapes follow what the code does
  today (snake_case keys, the agent's values as sent); when #149, #150 and
  #151 land, they follow those decisions.
- **The command group** is `brnr elicitation`, its verbs ACP's response
  actions (ADR 63):

  ```text
  brnr elicitation requests [<session>] | show <session> <request>
                  | accept <session> <request> [--field <k>=<v>]... | -
                  | decline <session> <request> | cancel <session> <request>
  ```

  `requests` is a table (SESSION, REQUEST, OWNER, MODE, MESSAGE), over the
  processes that answer as `permission requests` is (ADR 27). `show` is one
  in full: the message, mode and scope, a form's fields with their types,
  constraints and choices, a URL as the agent sent it. brnr never opens the
  URL (P8: the agent's text is untrusted); the user opens it and then
  `accept`s. `accept` takes the form's values as `--field name=value`
  (repeated for a multi-select) or as one JSON object on stdin (`-`), and
  the host checks them against the request's schema before anything is
  sent (P4, P7): every field named in the schema, the required ones given,
  each value of its type (a `--field` string is read as the type asks:
  `42` for an integer, `true` for a boolean), within `minimum`/`maximum`,
  `minLength`/`maxLength` and `minItems`/`maxItems`, and one of `enum`,
  `oneOf` or the items' choices where there are any. A value that doesn't
  fit fails the command, naming the field, and nothing is sent. `pattern`
  and `format` are passed on as given: checking them takes a regex engine
  brnr doesn't have (a crate needs a reason), and the agent checks them
  itself. The response is written with the schema's types
  (`CreateElicitationResponse`), so an accept's `content` has ACP's value
  types. `accept` of a URL elicitation takes no fields; one of a mode brnr
  doesn't know can only be declined or cancelled. A request-scoped
  elicitation has no session: it is listed with none (`--pid <pid>` in
  the table's SESSION column) and named by its process, `--pid <pid>` in
  place of `<session>`, as `event watch` names one.
- **Socket and bridges** (ADR 35): `pending` answers with `elicitations`
  beside `pending`, each in full with `answerable` and `why_not`;
  `elicitation {session, request, action: accept|decline|cancel,
  content?}` answers one, `session` null for a request-scoped one. The
  turn's `cancel` keeps its name.
- **One timeout.** `response_timeout` (and `--response-timeout`) is the one
  timeout for the user's responses: it covers permission requests and
  elicitations alike. It was `permission_timeout` (and
  `--permission-timeout`), renamed outright, with no alias (P9), now that
  it times out more than permissions. Both are the agent waiting on the
  user mid-turn, and a second number would be one more setting to get wrong
  for the same question. As before, there is no default: without it, a
  request waits for as long as nobody answers. What a timeout sends differs because ACP's answers differ: a
  permission request has no outcome for "nobody chose", so its timeout
  rejects once or cancels the turn (ADR 63); an elicitation has `cancel`
  for exactly that, so a timed-out elicitation is answered `cancel`, `by`
  `timeout`, never `decline`, which says the user said no, nor `accept`.
  `elicitation show` says so before it happens, as `permission show` does.
  `prompt cancel` answers the session's pending elicitations `cancel` too,
  `by` `cancel`, as it answers its permission requests `cancelled`: the
  turn they were asked in is gone, and nothing is left waiting unsaid
  (P3).
- **Editor sessions** (ADR 4): the request passes through to the editor
  unchanged, and `elicitation_request` is emitted with `owner: editor`, so
  watchers and bridges see it; the editor's answer is
  `elicitation_resolved` `by` `editor`. Answering from the side channel is
  the experimental action `elicitation`, refused unless the editor's
  profile enables it, and in strict mode: when it is taken, the request is
  withdrawn from the editor with `$/cancel_request` and the editor is told
  in the session ("Accepted via brnr", "Declined via brnr"); the editor's
  late answer is dropped and it is told who answered first, as `permission`
  does. `elicitation show` says of an editor's request that it is waiting
  in the editor, and whether it can be answered here (`answerable`,
  `why_not`).
- **Status.** `session status` counts `elicitations` beside `pending`, and
  a session with one waiting is `waiting`.

## Consequences

- claude-agent-acp turns AskUserQuestion back on for headless sessions once
  it sees `elicitation.form`, and MCP servers' elicitations and URL logins
  reach the user. Not checked here: it needs the real adapter with
  credentials, past what `adapters/check.py` can run;
  `adr_0068_headless_advertises_elicitation` checks what brnr advertises.
- A headless session's turn now waits on an elicitation where it was
  declined at once: set `response_timeout` for unattended runs.
- Bridges that list `pending` see a new `elicitations` key; one that
  answers elicitations sends `elicitation`.

## Considered

- `form` only (the issue's alternative): URL mode is how MCP servers log in
  (OAuth), and showing a URL without opening it is as safe as showing a
  command.
- A timeout of its own (`elicitation_timeout`): two settings for one
  question; nothing found that needs them to differ.
- Keeping the name `permission_timeout` for both: it would name one of the
  two things it times out.
- A timed-out elicitation answered `decline`: that is the user's "no", and
  nobody said it.
- Checking the values in the CLI: the host holds the schema and the bridges
  answer too, so the check is the host's, once.
- Opening a URL (`open`, `xdg-open`): the agent's text on a command line
  (P8), and a URL the user hasn't seen.
- `brnr elicitation answer <action>`: ADR 63's rule is ACP's verbs.
- Holding an elicitation of a mode brnr doesn't know as nothing: it would
  wait unseen; held, it can be shown, declined or cancelled.

## Tests

Run `cargo test --release adr_0068_`. Named claims and their assertions:

- [tests/cli.rs](../../tests/cli.rs)
  - `adr_0068_headless_advertises_elicitation`: `initialize` carries
    `elicitation: {form: {}, url: {}}`, and the fake agent, which asks only
    a client that advertised the mode, asks.
  - `adr_0068_requests_and_show_list_an_elicitation`: a form and a URL
    elicitation held, listed by `requests` and shown by `show` with their
    message, mode, scope, fields and URL, in text and `--json`, the URL
    printed and not opened.
  - `adr_0068_accept_checks_fields_against_the_schema`: valid fields, from
    `--field` and from stdin, are typed as the schema asks and sent as
    `{"action": "accept", "content": …}`, with `elicitation_resolved`; a
    field not in the schema, a missing required one, a wrong type, a value
    out of range, too long, not a choice or too many items each fails
    naming the field, and nothing is sent.
  - `adr_0068_decline_and_cancel`: each sends its action and emits
    `elicitation_resolved`; `accept` of a URL elicitation takes no fields.
  - `adr_0068_url_accept_is_followed_by_complete`: the agent's
    `elicitation/complete` after an accepted URL is `elicitation_completed`.
  - `adr_0068_wait_for_elicitation`: `event wait --for elicitation` returns
    when one is waiting, at once if one already is.
  - `adr_0068_send_wait_reports_an_elicitation`: `prompt send --wait` says
    on stderr that input is asked for, and how to give it.
  - `adr_0068_a_bridge_sees_the_elicitation`: a started bridge gets
    `elicitation_request` and `elicitation_resolved`.
  - `adr_0068_a_request_scoped_elicitation_is_answered_by_pid`: a URL
    elicitation scoped to `authenticate`, with no session, is listed with
    none and answered with `--pid`.
  - `adr_0068_prompt_cancel_cancels_the_sessions_elicitations`.
- [tests/security.rs](../../tests/security.rs)
  - `adr_0068_a_timeout_cancels_never_accepts`: `response_timeout`
    answers `cancel`, `by` `timeout`, which `show` said it would.
- [tests/headless.rs](../../tests/headless.rs)
  - `adr_0068_an_editors_elicitation_is_seen_and_reaches_the_editor`: the
    editor gets the request as sent, `elicitation_request` is emitted with
    `owner: editor`, the editor's answer is `elicitation_resolved`,
    answering from outside is refused without `elicitation` in the
    profile, and with it the request is withdrawn, the editor told, and
    its late answer dropped.
- [src/render.rs](../../src/render.rs)
  - `adr_0068_elicitation_events_have_lines`.
