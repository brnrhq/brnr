# 27. Approvals: the agent's mode is the policy

Accepted (former decisions 7, 12, 45 and 47); reviewed 2026-10-07.
Implemented; approvals on an editor's session are ADR 4's experimental
`approve`, and `show` says of an editor's request that it is waiting in the
editor, and whether it can be answered here (`answerable`, `why_not`).

## Context

brnr had a policy of its own for headless approvals: how the host answers
`session/request_permission` when no editor is attached
(`permissions = "ask" | "auto-allow" | "auto-deny"`, then a table by ACP tool
kind, and `--permissions`). It overlapped the agent's own session mode, which
already says when to ask at all (Claude's `acceptEdits` or
`bypassPermissions`), and the two composed confusingly: mode `default` with
`auto-allow` approved everything one round trip at a time, `plan` with
`auto-allow` approved leaving plan mode, and neither setting knew of the
other.

## Decision

- How much the agent asks is its mode (P2), set at start (`start --mode`,
  `mode` in the profile's headless part, applied before the first prompt) or
  later (`brnr mode`). brnr always asks when the agent does: a request waits
  for `brnr approve` or `deny`, or a bridge.
- `permission_timeout = <seconds>` denies a request nobody answered in time.
  Only deny: allowing on timeout would approve whatever an unattended agent
  asked for.
- `brnr pending [<session>]` lists the approvals waiting, as a table.
  `brnr show <session> <request>` shows one in full: the tool, its kind,
  paths, the command or input, and for edits a diff made from the tool call's
  `oldText`/`newText` with a small line diff (very large ones show the new
  text, saying so). `show` warns about a command with control characters in
  it (P8).
- `approve` and `deny` take `<session> <request>`: brnr doesn't pick the only
  request waiting (P4). `--option <id>` chooses an option; without it,
  `approve` picks the `allow_once` option, else `allow_always`, and `deny`
  picks `reject_once`, else `reject_always`, whatever their order in the
  request: allow, or reject, once. `deny --option` can't pick an allow
  option, nor `approve` a reject one; denying a request that offers no reject
  option cancels it.
- With no editor attached, brnr is the agent's client: approvals as above,
  elicitation declined, and anything else answered "method not found".
- The `permissions` key and `--permissions` flag are gone; a config that
  still has them fails to load, and `brnr doctor` says so.

## Considered

- Keeping both layers: agent-agnostic rules by tool kind and auditable
  auto-answers in the transcript, but two policies for one question.
- `pending -v` for the details: `pending` stays a table, `show` is one
  request in full.
- Approving on timeout: see above.
