# Session details

Edge cases for the [`session`](session.md) commands. Syntax, flags, JSON
fields, exit codes, text output and examples stay on the
[session reference](session.md).

## Lifecycle

Without `--pid`, `session new` and `session resume` start a host. With
`--pid`, they open a session in a host that is already running, over its
control socket (ADR 3).

```text
editor ──stdio── brnr acp ──socketpairs── brnr host ──pipes── agent
                                          │
                               control socket ── brnr
```

`session status`'s `state` is derived from whether a turn is running and
whether an approval or elicitation is held: `waiting` if anything is held,
else `busy` if a turn runs, else `idle`.

| From | Event | To |
|---|---|---|
| `idle` | A prompt starts a turn | `busy` |
| `busy` | The turn ends and nothing is held | `idle` |
| `busy` | A permission or elicitation is held | `waiting` |
| `waiting` | The held request is answered and no turn runs | `idle` |
| `waiting` | The held request is answered and a turn still runs | `busy` |
| any open | `session close` | The session ends; if it was the last headless session and nothing is opening, the process stops |

When the editor goes away, the agent ends with it (a turn still running is
lost); `session resume` carries the session on headless. Watching an
editor's session always works; acting on it is experimental
([editor](../guide/editor.md#experimental-actions)).

## `session new`: start commit and pid

`--pid <pid>` opens the session in a running headless process, with its
agent, profile and MCP servers. The process flags and `-- <agent>` are a
usage error with it.

- An editor's process is refused with `process_owned_by_editor`, and so is a
  process with `stop_when_idle` whose agent can't close sessions
  (`capability_missing`).
- The session, its settings (over the profile's) and the prompt commit as
  one. A setting that fails after the agent opened the session closes it
  again, and the error says so; for an agent that can't close sessions, the
  error names the session left open.
- The start timeout goes to the process, which abandons the opening the same
  way once it passes; the command waits 10 s longer. A process stopping
  meanwhile fails it. A command gone first (Ctrl-C) leaves no session. None
  of these sends the prompt.
- A process with a session being opened doesn't stop as its other sessions
  close or go idle meanwhile.

## `session new`: flags and input

Session flags are set before the first prompt, and win over the profile's,
setting by setting. Two values for one setting fail before anything is sent,
as do a mode, model or thought level the agent has no option for
([`config set`](config.md#brnr-config-set) has the rules).

A word left over, such as a prompt given as words without `--prompt`, is a
usage error. A file or image that can't be read stops the command before
anything is sent (`input_invalid`).
[Files and images](../adr/0006-settings-and-profiles.md#64-files-and-images-in-a-message)
has their encoding and what the agent must support.

## `session resume`: which agent and cwd

Which agent and cwd it uses:

- A session brnr has a transcript of resumes with the cwd, agent and profile
  of the newest record of all its files, in every project folder. `--cwd`,
  `--profile` and `-- <agent>` override them.
- A session brnr has no transcript of (one only the agent knows) is resumed
  by its id with the agent after `--`, else the profile's (`--profile`, else
  `default`); with neither, it fails (`agent_not_started`). Its cwd is
  `--cwd`, else the profile's `cwd`, else the current directory.
- With `--pid`, the agent is that process's. The cwd is `--cwd`, else the
  recorded cwd, else the current directory.

With `--pid`, a session open in that process already is refused, and one
another process holds is refused unless `--take-over`.

## `session status`: counts vs lists

The `_count` fields are counts. Use `permission requests`, `elicitation
requests` and `queue list` for what they count. Tool, plan, usage and token
data come from the agent.

## `session delete`: agent copy vs brnr transcript

Through the agent recorded for it (started for that), or `--profile` /
`-- <agent>`. Without `--purge`, brnr's transcript stays with a
`session_deleted` record.

A session open in a process is refused: close it first. So is an agent
without `sessionCapabilities.delete`. The session's lock is held while the
agent deletes it. A transcript made private first is said on stderr,
`path_made_private: <path> (its mode was …)`.

With `--purge`, for an agent that doesn't have the session (ACP's
`resource_not_found`, `-32002`), exit 0 and brnr's transcript is deleted.
The pinned `brnr-claude-adapter`
([adapters/package.json](https://github.com/brnrhq/brnr/blob/main/adapters/package.json))
doesn't answer that: it answers `-32603`. If a file couldn't be deleted or
recorded in, exit 1; the others are done all the same.

## `session prune`: age, locks, retention

Nothing is asked of an agent: its own copy of a session stays. By default
brnr keeps everything; the config's [`retention`](configuration.md#retention)
has every process do this as it starts
([ADR 9.5](../adr/0009-security-and-privacy.md#95-transcripts-are-kept-until-they-are-pruned)).

- A session's age is its last activity, `session list`'s `last_active`:
  the newest record a process wrote in any of its files, not a deletion.
- Never pruned: a session open in a process or whose lock is held, the
  host log of a running process, and a file brnr can't read the age of (in
  another format, or with no record).
- A session's lock is held while its files go, and its age read again: one
  used since it was listed stays. Its events file goes last.

## `session list`: cwd matching and agent merge

With `--profile` or `-- <agent>`, only the cwd's, joined on the session id
with the agent's own list (`session/list`, every page, from the agent
started just for that).

- A cwd is the same however it is spelled (a trailing slash, a symlink).
- With an agent, a session open in a process that the agent lists in the cwd
  is the cwd's, whatever its recorded cwd. An `unreachable` session is the
  cwd's if the agent lists it there or brnr has its transcript there.
- An agent that can't list sessions fails the command (`capability_missing`).

Rows are ordered by the time `last_active` names. An agent's `updatedAt` that
isn't an RFC 3339 time is passed on as the agent gave it, after the rows with
a time, and said on stderr. A session only the agent knows is `inactive`,
with `pid` null. `forked_from` is the session a fork came from, by `session
fork` or an editor's `session/fork` through `brnr acp`.
