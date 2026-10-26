# Agents with ACP of their own

Some agents speak ACP themselves, and need no adapter: JetBrains Junie
(`junie --acp true`) and Cursor (`cursor-agent acp`). brnr runs them as it
runs any agent, with the agent's own login: log in with the agent first
(`cursor-agent login`, or Junie's own login), as you would without brnr.

```sh
brnr acp -- junie --acp true          # your editor's agent command
brnr acp -- cursor-agent acp
brnr session new --prompt "fix the failing tests" -- junie --acp true
```

In a [profile](profiles.md#profiles):

```toml
[profiles.junie]
agent = ["junie", "--acp", "true"]

[profiles.cursor]
agent = ["cursor-agent", "acp"]
```

Then `brnr acp --profile junie` in the editor, or `brnr session new
--profile cursor --prompt …`.

What each agent can do is what it advertises, and brnr does no more
([ADR 1](../adr/0001-principles.md), F1): a command that needs what an agent
doesn't advertise fails, `capability_missing`. Cursor 2026.10.19 advertises
no `session/close`, `session/fork`, `session/resume`, `session/delete` or
steering, so with it:

- `brnr session close` and `session fork` fail; stop its process with
  `brnr process stop <pid>`.
- `brnr session resume` loads the session (`session/load`).
- `brnr prompt send --steer` fails; `--interrupt` works.
- It gives no `agentInfo`, so `session status` has no agent name or
  version.

## The smoke check

`adapters/smoke.py` checks an agent through brnr, with the agent itself
([ADR 11.9](../adr/0011-building-releasing-and-support.md#119-agents-with-acp-of-their-own-are-checked-by-hand-with-a-smoke-check)).
It runs only when you run it: it uses the agent's login and your quota, with
short prompts (four turns at most). From a checkout, with brnr built:

```text
cargo build --release
adapters/smoke.py junie                  # junie --acp true
adapters/smoke.py cursor                 # cursor-agent acp
adapters/smoke.py -- <agent> [args...]   # any other agent
```

It prints brnr's version and the agent's, then a line for each check:
initialize, session/new, a prompt answered "ok", a cancelled turn, fork,
a clean shutdown (the process, the agent and what the agent started all
gone, and the host log ending with `process_exited`), list, resume and
close. A check whose capability the agent doesn't advertise is
`unsupported`, not run. `BRNR_SMOKE_KEEP=1` keeps brnr's transcripts and
host logs, to look into a failure.

## Results

2026-10-24, brnr 0.10.0 (main at 81ae7d0), macOS 26.4.1 (Darwin 25.4.0,
arm64). Two runs of the check with Junie, three with Cursor; each result
held in every run but where it says.

| Check | Junie 26.10.5 (3579.5) | Cursor 2026.10.19-6bbf2c4 |
|---|---|---|
| initialize | ok: agentInfo `@jetbrains/junie`; resume, load, list, fork, close, delete, images, steering, MCP over HTTP and SSE | ok: no agentInfo; load, list, images, MCP over HTTP and SSE |
| session/new | ok | ok |
| prompt | ok: `ok` | **fail** (the agent): `ok`, then `Error: RetriableError: WritableIterable is closed` |
| cancel | ok: ends `cancelled` | ok: ends `cancelled` |
| fork | ok: the fork answers, and closes | unsupported |
| shutdown | ok: the agent exits 0 at once | ok: the agent exits 143, SIGTERM after 5 s |
| list | ok | ok |
| resume | ok: `session/resume` | **fail** (the agent): `session/load`, then the same reply as the prompt's |
| close | ok: the process stops with the session | unsupported |

What the agents do that ACP or a direct run doesn't explain is recorded
here, and brnr doesn't work around it (F1):

- Cursor ends every reply with a second message chunk,
  `\n\nError: RetriableError: WritableIterable is closed`, and the turn
  ends `end_turn`. It does so without brnr too, talking ACP to
  `cursor-agent acp` directly.
- Cursor's `session/load` replays none of the session's history, which
  ACP says it must before it answers. brnr's own record of the session is
  whole: `brnr event log` shows it.
- Cursor doesn't always exit when its stdin closes. Through brnr it
  didn't after a turn, in every run, and in two of six stops with no turn;
  without brnr, with no turn, in one of nine. `brnr process stop` sends
  SIGTERM to its process group 5 s later
  ([ADR 3.2](../adr/0003-processes-sessions-and-ownership.md#32-sessions-are-the-agents-ids-processes-are-pids)),
  and it and its child processes go.
- In the first run, Cursor's reply after `session/load` had the
  conversation in it, in Cursor's own tags (`<timestamp>`,
  `<user_query>`). Its transcript wasn't kept, and later runs didn't
  show it again.
- Junie records a session's cwd with its symlinks resolved, and
  `session/list` matches the cwd it is asked for exactly. On macOS,
  `brnr session list --cwd /tmp/x -- junie --acp true` lists none of the
  sessions opened in `/tmp/x` (Junie has them in `/private/tmp/x`); ask
  with the resolved path. brnr asks with the cwd as you give it.
