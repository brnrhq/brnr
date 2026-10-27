# Errors

Wherever brnr reports an error as data, it is `"error": {"code",
"message"}`
([ADR 8.7](../adr/0008-the-command-line-and-compatibility.md#87-an-error-is-a-stable-code-and-a-message)):

- a bridge or socket reply that fails: `{"ok": false, "error": {…}}`
  ([bridge protocol](bridge.md#requests-and-replies));
- an event or note that reports a failure: [`turn_ended`](events.md#turn_ended),
  [`process_exited`](events.md#process_exited),
  [`events_missed`](events.md#events_missed),
  [`records_skipped`](events.md#records_skipped);
- the `error` of `--wait`'s turn, of `last_turn` and of `session delete`;
- a failed command with `--json`: its stdout is `{"error": {code, message}}`
  ([output](overview.md#output)), unless it printed its JSON before it failed:
  a stream's events, or a wait's turn, which a failed turn's wait prints too
  (exit 1), its `error` in it.

On stderr a failure is `brnr: <message> [<code>]`, in text and with `--json`
alike. A usage error (exit 2), a wait's timeout (124) and a cut-off (3) have
their exit status instead, and no code; `doctor` exits 1 with its checks.
The library's facade returns a usage error as `usage_invalid`.

| Field | Type | Meaning |
|---|---|---|
| `error` | object | The failure, where a command or a reply has one. |
| `error.code` | string | One of the [codes](#codes) below. |
| `error.message` | string | What failed, for people. Not covered. |
| `error.rpc_error` | object, optional, ACP's | With `agent_failed` only: the agent's JSON-RPC error, whole, as it sent it (`{code, message, data}`). |

Where the agent's answer failed, the code is `agent_failed`, and `rpc_error`
is the agent's error, whole. Its numeric code is ACP's; brnr's codes are
words. A script that needs ACP's reads `rpc_error.code`.

<!-- example: brnr session new --json --prompt "reply hi" -- brnr-claude-adapter; before: Nothing; agent: AUTH=1; exit: 1 -->
```json
{
  "error": {
    "code": "agent_failed",
    "message": "session/new failed: Authentication required",
    "rpc_error": {
      "code": -32000,
      "message": "Authentication required"
    }
  }
}
```

**Covered** from 1.0.0: each code below and what it means. A minor release
can add a code, for a new failure or for one that was `unclassified`.
**Not covered**: `message`, and `unclassified`, which promises nothing.

## A running session

A command on a running session, `<session>`, finds it first. These are the
errors of that, whatever the command:

- `session_not_found`: brnr knows no session of that id.
- `session_inactive`: it isn't open in any process; resume it.
- `session_active`: it is open where the command needs it not to be.
- `session_closing`: it is closing; it is free once its records are written,
  2 s at most.
- `session_opening`: it is opening, and takes no requests until it has.
- `process_unreachable`: the process that holds it doesn't answer, or can't
  be reached from here.

Every command that asks a process can also get `command_unknown`, from a
process of another brnr ([ADR 8.8](../adr/0008-the-command-line-and-compatibility.md#88-a-process-and-the-commands-can-be-different-brnrs)),
and `unclassified`.

## Codes

| Code | The failure |
|---|---|
| `acp_version_unsupported` | The agent speaks an ACP version brnr doesn't. |
| `agent_exited` | The agent exited, and its process with it, before the command was done: a start, a wait, a request. |
| `agent_failed` | The agent answered with a JSON-RPC error; `rpc_error` is that error, as the agent sent it. |
| `agent_not_started` | No agent to start: none given or recorded, or its program can't be run. |
| `answer_invalid` | An answer that doesn't fit its permission request or elicitation: no such option, a kind missing or doubled, the other side's option, content that doesn't fit the form, an action that isn't one. |
| `capability_missing` | The agent doesn't advertise what was asked: forking, closing, resuming, listing or deleting sessions, steering, images, an MCP server's transport. |
| `command_invalid` | A socket request that isn't one: not JSON, no `cmd`, a field missing or of the wrong type, fields that don't go together. |
| `command_unknown` | A `cmd` the process doesn't have, or a field of one it doesn't take: a newer brnr's request to an older brnr's process; or a `status` answer without a field this brnr reads, an older brnr's. |
| `config_invalid` | The config file can't be read, isn't private, or doesn't load. |
| `event_unknown` | A note's name in `subscribe`'s `events`: no process sends one. An event brnr doesn't have, or a note, in a profile's bridge `events`. In `subscribe`, an event the process doesn't have isn't an error: it is named in `unknown_events`. |
| `experimental_disabled` | An action on an editor's session its profile doesn't enable. |
| `input_invalid` | What the command reads besides its command line won't do: an empty prompt on stdin, stdin that isn't the form's fields, a file or image that can't be read or sent, a `--cwd` that isn't a directory. |
| `message_not_found` | No held message has that id. |
| `process_not_found` | No brnr process has that pid. |
| `process_owned_by_editor` | An editor's process: no session is opened or forked in it. |
| `process_panicked` | brnr itself failed, and its process ended: a bug to report. |
| `process_starting` | The process hasn't committed its start. |
| `process_stopping` | The process is stopping, or its agent takes no more input. |
| `process_unreachable` | The process doesn't answer, or can't be reached from here (another `BRNR_DIR`). |
| `profile_not_found` | The config has no profile of that name. |
| `request_not_found` | No permission request or elicitation of that handle is waiting. |
| `session_active` | The session is open in a process, where the command needs it not to be: close it first, or take it over. |
| `session_closed` | The session closed before the command was done. |
| `session_closing` | The session is closing; it is free once its records are written (2 s at most). |
| `session_inactive` | The session isn't open in any process; it can be resumed. |
| `session_not_found` | brnr knows no session of that id. |
| `session_opening` | The session is opening, and takes no requests until it has. |
| `setting_invalid` | A mode, model, thought level, config option or login method the agent doesn't offer, or a value it doesn't take. |
| `start_timed_out` | The session didn't open in the time its start had. |
| `strict_mode_refused` | Strict mode refuses what isn't stable ACP. |
| `transcript_not_kept` | The process keeps no transcript to read, or no raw ACP (`log`). |
| `transcript_unsupported` | The transcript is in a format this brnr doesn't read. |
| `turn_owned_by_editor` | The editor's session's turns are the editor's: a steer, an interrupt, or a prompt while its turn runs. |
| `unclassified` | Anything else. It promises nothing: don't match it. |
| `usage_invalid` | The arguments don't make a command. Only the library's [facade](overview.md) returns it: the command line exits 2 for it, with its usage and no code. |
