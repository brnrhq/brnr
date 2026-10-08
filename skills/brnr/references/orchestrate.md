# Orchestrating workers

A worker is a headless session: brnr starts the agent, opens a session and
sends it the prompt. You keep its id, give it work, wait for it, read what it
did, and close it.

## A worker's life

**Start.** Put the task in a file (or pass `--prompt "<text>"`), and keep the
`session` from the JSON. `--stop-when-idle` closes the session once it has
been idle that many seconds (no turn running, nothing held, no approval
waiting), so a worker you lose track of doesn't run forever; its process
stops with its last session.

```sh
brnr start --json --stop-when-idle 600 --cwd ~/work/project --prompt - -- brnr-claude-adapter < task.md
```

```json
{"session": "0f6c…", "pid": 4466, "message": "m1"}
```

`message` is the prompt's id, `pid` the process (for `brnr stop`). `--mode`,
`--model` and `--set <option>=<value>` set the session up before the first
prompt; leave them out unless the user chose them. A start is atomic: if it
fails (exit non-zero, the reason on stderr), nothing was sent and nothing is
left running. Tell the user why; don't start it again with other settings on
your own.

**Send.** A message to a worker that is busy is held, and sent as its own
turn when the running one ends; brnr never sends two prompts at once.

```sh
brnr send $s --json "also update the changelog"
```

```json
{"status": "held", "session": "0f6c…", "message": "m2"}
```

`status` is `delivered` (sent now), `held`, `steered` or `interrupting`.

**Wait.** For the session to be idle (no turn running, nothing held), or for
one message's turn, always with a timeout:

```sh
brnr wait $s --timeout 900 --json
brnr wait $s --for turn --timeout 900 --json
brnr send $s --wait --timeout 900 --json "what did you change, and why?"
```

`wait` prints what ended it (`{"event": "idle", "session": …}`, or the
`turn_ended` event). `send --wait` prints the turn as one object at its end:

```json
{"session": "0f6c…", "message": "m3", "reply": "…", "stop_reason": "end_turn", "error": null, "dropped": null}
```

**Read.** The reply is in `send --wait`'s `reply`. For what the worker did
(its tool calls, its plan, its messages), read the log: `--last 1` is from
the latest message sent to it on.

```sh
brnr log $s --last 1 --json
```

**Close.** When its work is done, or the user says to stop:

```sh
brnr close $s
```

A running turn is cancelled first. The process stops with its last session.

## Exit statuses

`wait`, `send --wait` and `start --wait` exit with the turn's result:

| Status | Means | Do |
|---|---|---|
| 0 | the turn ended normally (`end_turn`) | carry on |
| 1 | it failed (`error`), stopped for another reason (`stop_reason`), its message was dropped (`dropped`), the agent exited, or the session closed first | tell the user what brnr said on stderr or in the JSON; don't retry quietly |
| 124 | your `--timeout` passed | the worker is still going: wait again, `brnr cancel`, or ask the user |

A worker waiting for an approval doesn't end its turn: `wait` runs until your
timeout. `send --wait` and `start --wait` say on stderr when an approval
comes (`brnr: waiting for approval p1: …`); with `wait`, wait for it too
(`--for permission`) or check `brnr pending --json` when it times out. Then
see the approvals reference.

## One task, one command

For a task with one answer, start, wait and stop in one command. stdout is
the reply and nothing else; `started …` goes to stderr.

```sh
brnr start --wait --timeout 900 --json --stop-when-idle 0 --prompt - -- brnr-claude-adapter < task.md
```

With `--json` the result is one object, as `send --wait`'s, with the `pid`.

## Several workers at once

Start each, keep each `session`, and wait for each. Give each its own
directory (a git worktree, say) when they edit files: two agents in one
checkout overwrite each other.

```sh
brnr start --json --stop-when-idle 600 --cwd ~/work/api --prompt "make the api tests pass" -- brnr-claude-adapter
brnr start --json --stop-when-idle 600 --cwd ~/work/web --prompt "make the web tests pass" -- brnr-codex-adapter
```

Then `brnr wait <session> --timeout 900 --json` for each, in turn or in
parallel; a session that is idle already returns at once, so the order
doesn't matter. Report each worker's result to the user by its task, not by
its id alone.

## Changing course

```sh
brnr send $s --steer --json "use the existing helper in src/util.rs"
brnr send $s --interrupt --json "stop: wrong branch, switch to main first"
brnr send $s --context --json "the API key is in .env.local"
brnr queue $s --json
brnr cancel $s --json
```

- `--steer` puts the message into the running turn (refused if the agent
  can't steer); on an idle session it is sent as a prompt.
- `--interrupt` cancels the running turn and sends the message ahead of what
  is held.
- `--context` adds text to the next prompt, without a turn of its own.
- `queue` lists what is held; `--drop <message>` and `--clear` drop it.
- `cancel` stops the running turn and drops what is held (`dropped` lists
  them; `--keep-held` keeps them). Each dropped message is one you sent and
  the worker never got: say so if it mattered.

## Processes

```sh
brnr ps --json
brnr stop 4466
```

`ps` lists brnr's processes (`pid`, `owner`, `agent`, `sessions`). Stop only
a process you started (its `pid` from `start --json`), when closing its
sessions isn't enough: `stop` ends every session in it. Never stop one whose
`owner` is `editor`.
