# permission

The agent's permission requests (`session/request_permission`), which brnr
holds as approvals until they are answered: by these commands, a bridge, the
editor, the response timeout, or the agent giving them up.

## `brnr permission requests`

Lists the approvals waiting, in every process that answers, or in one
session.

    brnr permission requests [<session>] [--json]

A process that doesn't answer is passed over, as `process list` and `session
list` pass over it (they show it `unreachable`), and said on stderr, `brnr:
process <pid> is not answering; its approvals aren't listed`, in text and
`--json` alike. The exit status stays 0.

**Output** (`--json`): each row is `permission show`'s object, with fewer
fields.

| Field | Type | Meaning |
|---|---|---|
| `requests` | list of objects | One for each approval waiting. |
| `requests[].session` | string | Its session. |
| `requests[].request` | string | Its handle, `p1`. |
| `requests[].owner` | string | `editor` or `headless`. |
| `requests[].kind` | string or null | The tool call's kind, as the agent gave it (ACP's `toolCall.kind`); null if it gave none. |
| `requests[].title` | string or null | The tool call's title, as the agent gave it; null if it gave none. |
| `requests[].options` | list of objects | The agent's options, as ACP has them: `{optionId, name, kind}`. |

<!-- example: brnr permission requests --json; before: Approval -->
```json
{
  "requests": [
    {
      "session": "sess-1",
      "request": "p1",
      "owner": "headless",
      "kind": "edit",
      "title": "Edit src/lib.rs",
      "options": [
        {
          "optionId": "allow",
          "name": "Allow",
          "kind": "allow_once"
        },
        {
          "optionId": "reject",
          "name": "Reject",
          "kind": "reject_once"
        }
      ]
    }
  ]
}
```

Text output: a table of the approvals; `nothing waiting` when there are none.

**Exit codes:** 0; 1 if it fails.
**Errors:** with `<session>`, [those of a running
session](errors.md#a-running-session).

## `brnr permission show`

Shows one approval waiting, in full, so that it can be judged before it is
answered: the tool, its kind and paths, the command or input, the diff of an
edit, and the options.

    brnr permission show <session> <request> [--json]

**Output** (`--json`):

| Field | Type | Meaning |
|---|---|---|
| `request` | string | Its handle, `p1`. |
| `session` | string | Its session. |
| `owner` | string | `editor` or `headless`. |
| `answerable` | boolean | Whether `permission allow` and `reject` would be taken: an editor's session needs the experimental `permission` in its profile. |
| `why_not` | string or null | Why not; null when it is `answerable`. |
| `title` | string or null | The tool call's title, as the agent gave it. |
| `kind` | string or null | The tool call's kind, as the agent gave it. |
| `tool_call` | object | The agent's tool call, whole, as ACP has it (`toolCall`). |
| `options` | list of objects | The agent's options, `{optionId, name, kind}`. |
| `timeout_left` | number or null | Seconds until the response timeout answers it; null without one. |

<!-- example: brnr permission show $s p1 --json; before: Approval -->
```json
{
  "request": "p1",
  "session": "sess-1",
  "owner": "headless",
  "answerable": true,
  "why_not": null,
  "title": "Edit src/lib.rs",
  "kind": "edit",
  "tool_call": {
    "toolCallId": "perm-1",
    "title": "Edit src/lib.rs",
    "kind": "edit",
    "locations": [
      {
        "path": "src/lib.rs",
        "line": 2
      }
    ],
    "rawInput": {
      "file_path": "src/lib.rs"
    },
    "content": [
      {
        "type": "diff",
        "path": "src/lib.rs",
        "oldText": "one\nold line\nthree\n",
        "newText": "one\nnew line\nthree\n"
      }
    ]
  },
  "options": [
    {
      "optionId": "allow",
      "name": "Allow",
      "kind": "allow_once"
    },
    {
      "optionId": "reject",
      "name": "Reject",
      "kind": "reject_once"
    }
  ],
  "timeout_left": null
}
```

Text output: the tool call, its options, and how to answer it.

**Exit codes:** 0; 1 if it fails.
**Errors:** [those of a running session](errors.md#a-running-session),
`request_not_found`.

## `brnr permission allow`, `brnr permission reject`

Answers an approval. `allow` answers with the agent's option of kind
`allow_once` (`allow_always` with `--always`), `reject` with its option of
kind `reject_once` (`reject_always` with `--always`).

    brnr permission allow <session> <request> [--always] [--option-id <id>] [--json]
    brnr permission reject <session> <request> [--always] [--option-id <id>] [--json]

| Flag | Meaning |
|---|---|
| `--always` | The `always` kind, in place of the `once` kind. |
| `--option-id <id>` | Answer with the agent's option of this `optionId` instead. Where its kind is one of ACP's, it must be of the verb's side, and of the `always` kind with `--always`. |
| `--json` | Print the answer as JSON. |

A kind missing or there twice, or an option of the other side, fails and
lists the options; nothing is answered (`answer_invalid`).

**Output** (`--json`):

| Field | Type | Meaning |
|---|---|---|
| `session` | string | The session. |
| `request` | string | The handle. |
| `outcome` | object | The answer sent to the agent, ACP's `outcome`: `{"outcome": "selected", "optionId": …}`. |

<!-- example: brnr permission allow $s p1 --json; before: Approval -->
```json
{
  "session": "sess-1",
  "request": "p1",
  "outcome": {
    "outcome": "selected",
    "optionId": "allow"
  }
}
```

Text output: `p1 allow`, the request and the option chosen.

**Exit codes:** 0; 1 if it fails.
**Errors:** [those of a running session](errors.md#a-running-session),
`request_not_found`, `answer_invalid`, `experimental_disabled`,
`process_stopping`.
