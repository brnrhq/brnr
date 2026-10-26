# Approvals

```sh
brnr permission requests           # approvals waiting, in every session
brnr permission show $s p1         # one in full: the command, paths, the diff
brnr permission allow $s p1        # with its allow_once option; --always: allow_always
brnr permission reject $s p1       # with its reject_once option; --always: reject_always
```

`allow` and `reject` answer with the request's option of that kind, whatever
the order the agent gives them in. No other kind stands in for it: a request
without one, or with two, isn't answered, and the command fails, listing the
options. `--option-id <id>` names the option instead (for two of one kind, or a
kind ACP doesn't have): one of an ACP kind must be on the verb's side, and the
always kind with `--always`; one of a kind brnr doesn't know goes with either
verb. `reject` never answers `cancelled`, which ACP keeps for a cancelled
turn: `prompt cancel` is what does that.

A headless request waits until it is answered, or for `response_timeout`
(in the profile's headless part), if set: then it is answered with its
`reject_once` option, or, without exactly one, its turn is cancelled
(`session/cancel`), which answers it and every request pending in the session
`cancelled`. A timeout never allows, nor answers `reject_always`: nobody chose
that. `response_timeout` has no default (P4): without it, a request nobody
answers waits as long as the process runs. An unattended run, with nobody to
answer (a script, CI, a worker left alone), must set it.

The agent can give up on a request itself (`$/cancel_request`): it is no
longer waiting, `permission_resolved` says `by: agent`, and brnr answers it
`cancelled`, as ACP has it; an answer to it after that fails, and nothing is
sent. On an editor's session the editor gets the agent's notification and
answers it. A turn that ends with its request unanswered doesn't end the
request: it waits as any does, and the session stays `waiting`.

Whatever brnr shows of the agent's text has its control characters escaped
(`\u001b`), so a command can't be dressed up as another; `permission show`
says when a command has any. `--json` has the text as the agent sent it. On an
editor's session `permission show` says the request is waiting in the editor,
and that it can be answered here too, experimentally, where `permission` is
enabled, or else why it can't (`answerable` and `why_not` in `--json`).

# Elicitations

```sh
brnr elicitation requests          # the agent asking for input, in every session
brnr elicitation show $s e1        # one in full: the message, the form's fields, or the URL
brnr elicitation accept $s e1 --field name=Ada --field color=green   # checked against the form
brnr elicitation decline $s e1     # the user's no
brnr elicitation cancel $s e1      # no answer given, as a timeout sends
```

An elicitation is the agent asking for input (ACP's `elicitation/create`, ADR
68): a form, or a URL to open, such as an MCP server's login. Headless, brnr
advertises both modes, and holds each request until it is answered, as it
holds an approval. `accept` takes the form's values as `--field name=value`
(again for each value of a list) or as one JSON object on stdin (`-`), and the
process checks them against the form before anything is sent: each field is
one of the form's, the required ones are given, and each value is of its type
(`42` for an integer, `true` for a boolean), within its bounds and one of its
choices; `pattern` and `format` are left to the agent. What doesn't fit fails
the command, naming the field, and nothing is sent. A URL is shown, never
opened: open it yourself, then `accept` it, with no fields. A mode brnr
doesn't know can only be declined or cancelled. One asked outside any session
(a login, while the agent authenticates) is named by its process, `--pid
<pid>` in place of the session.

`response_timeout` covers elicitations too: one nobody answered is answered
`cancel`, never `decline` or `accept`, and `elicitation show` says when. `prompt
cancel` answers the session's elicitations `cancel` too, and one the agent
gives up on (`$/cancel_request`) is answered `cancel`, `by: agent`, as an
approval is. On an editor's session the request reaches the editor as the
agent sent it, and is seen (`elicitation_request`, `elicitation_resolved`) as
approvals are; answering it here is the experimental `elicitation`.
