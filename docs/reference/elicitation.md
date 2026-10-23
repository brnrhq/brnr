# elicitation

The agent asking for input (`elicitation/create`,
[ADR 4.5](../adr/0004-turns-messages-and-requests.md)): a form to fill in,
or a URL to open. brnr holds each until it is answered with ACP's action,
`accept`, `decline` or `cancel`. One scoped to a request outside any session
(a login) has no session, and is named by its process, `--pid <pid>`.

## `brnr elicitation requests`

Lists the elicitations waiting, in every process that answers, or in one
session. A process that doesn't answer is passed over and said on stderr, as
for [`permission requests`](permission.md#brnr-permission-requests).

    brnr elicitation requests [<session>] [--json]

**Output** (`--json`): each row is `elicitation show`'s object, with fewer
fields.

| Field | Type | Meaning |
|---|---|---|
| `requests` | list of objects | One for each elicitation waiting. |
| `requests[].session` | string or null | Its session; null for one outside any session. |
| `requests[].pid` | number | Its process. |
| `requests[].request` | string | Its handle, `e1`. |
| `requests[].owner` | string | `editor` or `headless`. |
| `requests[].mode` | string or null | `form`, `url`, or a mode brnr doesn't know, as the agent sent it. |
| `requests[].scope` | string | `session`, `tool_call` or `request`. |
| `requests[].text` | string or null | The agent's message. |

<!-- example: brnr elicitation requests --json; before: Elicitation -->
```json
{
  "requests": [
    {
      "session": "sess-1",
      "pid": 4466,
      "request": "e1",
      "owner": "headless",
      "mode": "form",
      "scope": "session",
      "text": "Tell me about you"
    }
  ]
}
```

Text output: a table, `SESSION`, `REQUEST`, `OWNER`, `MODE`, `MESSAGE`; one
outside any session has `--pid <pid>` as its session.

**Exit codes:** 0; 1 if it fails.
**Errors:** with `<session>`, [those of a running
session](errors.md#a-running-session).

## `brnr elicitation show`

Shows one elicitation waiting, in full: the message, the mode and scope, the
form's fields with their types, bounds and choices, or the URL. A URL is
printed, never opened.

    brnr elicitation show (<session> | --pid <pid>) <request> [--json]

| Flag | Meaning |
|---|---|
| `--pid <pid>` | The process, for one outside any session, in place of `<session>`. |
| `--json` | Print it as JSON. |

**Output** (`--json`):

| Field | Type | Meaning |
|---|---|---|
| `mode` | string or null | `form`, `url`, or a mode brnr doesn't know, as the agent sent it. |
| `text` | string or null | The agent's message (ACP's `message`). |
| `scope` | string | `session`, `tool_call` (a tool call in the session) or `request` (a request outside any session). |
| `tool_call_id` | string or null | The tool call, for scope `tool_call`; else null. |
| `scope_request_id` | string, number or null | The request, for scope `request`; else null. |
| `schema` | object or null | A form's `requestedSchema`, as the agent sent it; null for a URL. |
| `url` | string or null | A URL's address; null for a form. |
| `elicitation_id` | string or null | A URL's `elicitationId`; null for a form. |
| `request` | string | Its handle, `e1`. |
| `session` | string or null | Its session; null for one outside any session. |
| `pid` | number | Its process. |
| `owner` | string | `editor` or `headless`. |
| `answerable` | boolean | Whether `elicitation accept`, `decline` and `cancel` would be taken: an editor's session needs the experimental `elicitation` in its profile. |
| `why_not` | string or null | Why not; null when it is `answerable`. |
| `timeout_left` | number or null | Seconds until the response timeout answers it `cancel`; null without one. |

<!-- example: brnr prompt send $s --prompt "elicit url" && brnr event wait $s --for elicitation && brnr elicitation show $s e1 --json; before: Idle -->
```json
{
  "mode": "url",
  "text": "Log in to the fake",
  "scope": "session",
  "tool_call_id": null,
  "scope_request_id": null,
  "schema": null,
  "url": "https://login.example/a?b=1",
  "elicitation_id": "el-1",
  "request": "e1",
  "session": "sess-1",
  "pid": 4466,
  "owner": "headless",
  "answerable": true,
  "why_not": null,
  "timeout_left": null
}
```

Text output: the message, the mode and scope, the fields or the URL, and how
to answer it.

**Exit codes:** 0; 1 if it fails.
**Errors:** [those of a running session](errors.md#a-running-session),
`process_not_found` (`--pid`), `request_not_found`.

## `brnr elicitation accept`, `decline`, `cancel`

Answers an elicitation with ACP's action.

    brnr elicitation accept (<session> | --pid <pid>) <request> [--field <k>=<v>]... [-] [--json]
    brnr elicitation decline (<session> | --pid <pid>) <request> [--json]
    brnr elicitation cancel (<session> | --pid <pid>) <request> [--json]

| Flag | Meaning |
|---|---|
| `--pid <pid>` | The process, for one outside any session, in place of `<session>`. |
| `--field <k>=<v>` | `accept`: a form's field. Repeat it for a list's values. |
| `-` | `accept`: the form's fields as one JSON object on stdin, in place of `--field`. |
| `--json` | Print the answer as JSON. |

The process checks an accept's values against the form's schema before
anything is sent: fields in the schema, the required ones given, their types
(a `--field` string read as the type asks), bounds and choices. `pattern`
and `format` are not checked. A value that doesn't fit fails, naming the
field, and nothing is sent (`answer_invalid`). A URL's accept takes no
fields. A mode brnr doesn't know can't be accepted, only declined or
cancelled.

**Output** (`--json`):

| Field | Type | Meaning |
|---|---|---|
| `session` | string or null | The session; null for one outside any session. |
| `request` | string | The handle. |
| `action` | string | `accept`, `decline` or `cancel`. |
| `content` | object or null, ACP's | An accept's form values, with ACP's value types; null for a URL's accept, `decline` and `cancel`. |

<!-- example: brnr elicitation accept $s e1 --field name=Ada --field color=green --json; before: Elicitation -->
```json
{
  "session": "sess-1",
  "request": "e1",
  "action": "accept",
  "content": {
    "color": "green",
    "name": "Ada"
  }
}
```

Text output: `e1 accepted` (`declined`, `cancelled`).

**Exit codes:** 0; 1 if it fails.
**Errors:** [those of a running session](errors.md#a-running-session),
`process_not_found` (`--pid`), `request_not_found`, `answer_invalid`,
`input_invalid` (stdin isn't one JSON object), `experimental_disabled`,
`process_stopping`.
