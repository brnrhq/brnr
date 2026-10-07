# 33. Profiles: shared, headless and editor parts

Accepted 2026-10-07. Implemented, except that the README's note on tool
kinds is still there.

## Context

With `brnr acp --profile work`, most of a profile was silently ignored:
`cwd`, `mode`, `config`, `mcp_servers`, `permission_timeout` and
`stop_when_idle` apply only to headless sessions. brnr can't warn about it,
since the proxy's stderr is the agent's (P1). And ADR 4 and ADR 42 add
settings that apply only to an editor's process.

## Decision

The role is in the config itself (P7, P12):

```toml
[profiles.work]                     # every process of the profile
agent = ["brnr-claude-adapter"]
log = "all"                         # "events", or false (ADR 22)
strict = false                      # stable ACP only (ADR 41)

[[profiles.work.bridges]]
command = ["brnr", "notify", "--stdin", "--",
           "sh", "-c", "terminal-notifier -title \"$BRNR_TITLE\" -message \"$BRNR_TEXT\""]

[profiles.work.headless]            # brnr start
cwd = "~/work/project"
mode = "plan"
config = { model = "opus" }
permission_timeout = 600
stop_when_idle = 600
auth = "api-key"

[[profiles.work.headless.mcp_servers]]
name = "github"
command = "github-mcp-server"
args = ["stdio"]
env = { GITHUB_TOKEN = "…" }

[profiles.work.editor]              # brnr acp
experimental = ["send", "approve"]  # actions on the editor's session (ADR 4)
features = ["shared_sessions"]      # process management (ADR 42)
```

- At the top, what every process of the profile uses: `agent`, `log`,
  `strict`, `bridges`. Under `headless`, what only `brnr start` uses (ADR 12,
  27, 28, 30, 31). Under `editor`, what only `brnr acp` uses (ADR 4, 42).
- A key in the wrong part, an unknown key, and the old flat layout fail to
  load (P7, P9); `brnr doctor` says which and where.
- The README's leftover note on ACP tool kinds, from the permission rules
  ADR 27 removed, goes.

## Considered

- Keeping the profile flat and marking the headless-only keys in the README
  and `doctor`: an editor's profile would still carry settings it silently
  ignores.
