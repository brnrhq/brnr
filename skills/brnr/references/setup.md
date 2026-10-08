# Setting brnr up

Ask before installing anything or changing the user's config: say what you
would run or write, and where.

## Install

On macOS or Linux, with Homebrew:

```sh
brew install brnrhq/tap/brnr
brew install brnrhq/tap/brnr-claude-adapter   # for Claude Code, compiled on your machine
brew install brnrhq/tap/brnr-codex-adapter    # for Codex
```

The adapters run the user's own `claude` or `codex` CLI, which must be
installed and logged in already; brnr doesn't log in for the user. Then check:

```sh
brnr doctor --json
```

It prints the checks, each `{level, check, message}`, and exits non-zero if
one fails: the runtime directory, transcripts, the config and every profile,
where the adapters are, and the running processes. `brnr doctor --fix`
repairs what it safely can (permissions, files left by processes that are
gone); run it when the user agrees.

When brnr itself misbehaves (a panic, a process that died without saying
why), the user may want to report it:

```sh
brnr doctor --report
```

It prints, as Markdown for an issue, brnr's version, the OS, the adapters,
the checks that aren't ok, and the last lines of the latest host logs
(`--json`: one object, `brnr`, `os`, `adapters`, `checks`, `host_logs`). The
log lines can include prompts: show it to the user to read before anything
is pasted anywhere, and don't file it yourself. Nothing is sent by brnr.

## An editor

An editor runs brnr as its ACP agent: the command `brnr`, with the arguments
`acp -- brnr-claude-adapter` (or `acp -- brnr-codex-adapter`, or
`acp --profile <p>`).

```sh
brnr acp -- brnr-claude-adapter
brnr acp --profile work
```

In Zed, in `settings.json`:

```json
{
  "agent_servers": {
    "Claude Code (brnr)": {
      "type": "custom",
      "command": "brnr",
      "args": ["acp", "--", "brnr-claude-adapter"]
    }
  }
}
```

The editor's sessions then show in `brnr list`, with `owner` `editor`. You
can watch them; acting on them is off by default, and is the user's to turn
on (`experimental` in the profile's `editor` part, in brnr's README).

## Profiles

`~/.config/brnr/config.toml` (or `$BRNR_CONFIG`) holds profiles; without
`--profile`, the `default` one applies, if there is one. A profile for
workers:

```toml
[profiles.work]
agent = ["brnr-claude-adapter"]

[profiles.work.headless]
cwd = "~/work/project"
stop_when_idle = 600                # close a worker idle 10 minutes
permission_timeout = 1800           # deny what nobody answered in 30 minutes
```

```sh
brnr start --profile work --json --prompt - < task.md
```

What goes at the top applies to every process of the profile (`agent`,
`log`, `strict`, `bridges`); under `headless`, only to `brnr start` (`cwd`,
`mode`, `config`, `mcp_servers`, `permission_timeout`, `stop_when_idle`,
`auth`); under `editor`, only to `brnr acp`. A key in the wrong part fails
to load; `brnr doctor` says where it goes. Set `mode` only to what the user
chose: it decides what the agent asks approval for.

## This skill

```sh
brnr skill
brnr skill orchestrate
brnr skill install
brnr skill install --dir .claude/skills
```

`brnr skill` prints this skill's `SKILL.md`, `brnr skill <name>` one of its
references (`orchestrate`, `approvals`, `observe`, `setup`). `install`
writes it to `~/.claude/skills/brnr/` (Claude Code) and
`~/.agents/skills/brnr/` (Codex), or to `<dir>/brnr/` for each `--dir`
given, such as a project's `.claude/skills` or `.agents/skills`. Install
again after upgrading brnr: the skill is the one for the brnr that wrote it.
