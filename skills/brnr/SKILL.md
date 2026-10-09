---
name: brnr
description: Run, supervise and talk to coding-agent sessions (Claude Code, Codex) through brnr, the CLI that keeps ACP agents in a process other commands can reach. Use when asked to start headless agent workers, send them work, wait for their turns, read what they did, bring their approvals to the user, or set brnr up. Not for working on brnr's own source code.
---

# brnr

brnr runs an ACP agent (Claude Code through `brnr-claude-adapter`, Codex
through `brnr-codex-adapter`) in a process of its own. You start sessions
headless, send them messages, wait for their turns, and read their events,
all with `brnr` commands. A session is a worker; you are its supervisor.

Use it when the user asks you to hand work to other agents, to run several
in parallel, or to check on, talk to or answer for agent sessions brnr runs.
`brnr --help` lists every command, most of them grouped by what they act on
(`brnr event wait`; `brnr event --help` lists a group's); this skill is for
the brnr that printed it (`brnr skill`).

## Rules

1. **Use the CLI as it is.** Run `brnr` commands with `--json` and read the
   JSON. Don't write wrapper scripts, and don't parse the text output.
2. **Use exact ids.** A session is the `session` that `brnr start --json`
   printed, or one from `brnr list --json`; a request the `request` from
   `brnr permission requests --json`; a message the `message` from
   `brnr prompt send --json`. Never guess, never pick "the only one", never reuse an id you didn't get
   in this task without finding it in `brnr list --json`.
3. **Wait, don't poll.** Use `prompt send --wait`, `start --wait` or
   `event wait --for`, always with `--timeout`, never a loop of `sleep` and
   `session status`. The exit
   status is the result: 0 the turn ended normally, 1 it failed, stopped or
   its message was dropped, 124 your timeout passed (the worker goes on).
4. **Approvals belong to the user.** Don't `approve` or `deny` unless the
   user told you to, explicitly, for that session and that kind of request.
   Otherwise show the request (`brnr permission show`), say in a sentence
   what it would do, and wait for the user's answer. Don't switch a worker to a mode that
   asks less to get around this.
5. **Clean up what you start; touch nothing else.** Start workers with
   `--stop-when-idle`, and `brnr session close` each when its work is done.
   Sessions you didn't start, an editor's above all, you may watch and read, never
   send to, cancel, close, stop or take over, unless the user asks.
6. **Say what went wrong.** A failed turn, a dropped message, a timeout or
   an exited agent: tell the user what brnr said. Don't retry quietly.

## Core commands

`$s` is a session id you were given by brnr; `p1` a request id from
`permission requests`.

```sh
brnr start --json --stop-when-idle 600 --prompt - -- brnr-claude-adapter < task.md   # {session, pid, message}
brnr prompt send $s --json "now add tests for it"     # {status, session, message}: delivered, held, …
brnr event wait $s --timeout 600 --json               # until idle; exit 0, 1 or 124
brnr prompt send $s --wait --timeout 600 --json "what did you change?"   # {session, message, reply, stop_reason, error, dropped}
brnr event log $s --last 1 --json                     # the latest turn's events, one per line
brnr session status $s --json                         # state, mode, model, tools, plan, pending, held
brnr list --json                                      # running sessions: session, title, state, pid
brnr permission requests --json                       # approvals waiting, in every session
brnr permission show $s p1 --json                     # one in full: tool, paths, command, diff
brnr session close $s                                 # done with it: the session closes
```

Use `brnr-codex-adapter` for Codex. A start with `--cwd <dir>` works there;
without it, in the current directory.

## More, when you need it

Each is in `references/` next to this file, or printed by
`brnr skill <name>`:

- `orchestrate`: a worker's life, several workers at once, exit statuses,
  steering, interrupting and held messages.
- `approvals`: answering approvals, what counts as a delegation, what to
  show the user.
- `observe`: `session status`, `event log`, `event watch`, `event notify`,
  choosing events, and the events' JSON.
- `setup`: installing brnr and the adapters, editors, profiles, `doctor`,
  installing this skill.
