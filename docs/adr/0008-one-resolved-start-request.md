# 8. A process is started with one resolved request

Accepted 2026-10-07. Implemented.
Amends former decision 45.

## Context

brnr processes were started three ways: by `brnr acp` with flags
(`--link-fd`, `--profile`, `--sigmask`, `--proxy-pid`, the agent), by
`brnr start` with flags and `--ready-fd`, and by hand as `brnr host` with
`--prompt` (former decision 45 kept it "for brnr itself and by hand"), which
gave the process a second way to get a prompt (`first_prompt`). The process
loaded the profile itself, so the CLI and the process each resolved the
config.

## Decision

- Whoever starts a process (`start`, `acp`) resolves everything first:
  profile, agent, cwd, role (editor or headless), strict mode (ADR 41),
  experimental actions and feature flags (ADR 4, 42), mode, model, config
  options, MCP servers, auth method, prompt and attachments, timeouts,
  `stop_when_idle`, what to log (ADR 22), bridges, the signal mask; the
  agent's and the bridges' commands as they are run (a bare name installed
  next to brnr is that path, ADR 38). It writes
  one JSON request on the process's stdin; file descriptors carry only the
  editor link and its signal link (fds 3 and 4, ADR 2) or the start channel
  (fd 3, ADR 7).
- The process reads no config and takes no flags. `brnr host` is no longer
  run by hand; `start --foreground` is the way to run a session in a
  terminal (ADR 9).
- The host log's `started` record holds the request, secrets redacted
  (ADR 25): every process records what it was asked to do.
- A config error under `acp` is reported on the editor's stderr as
  `brnr acp: …`, as the process's own failures already are.

## Consequences

- One way to get a prompt to a process.
- The agent's command no longer appears in the process's argv, so killing
  processes by the adapter's name (`pkill -f brnr-claude-adapter`) no longer
  takes the host with it (ADR 11).

## Considered

- One request that names the profile, with the process loading it: config
  would still be resolved in two places.
- Keeping `brnr host` by hand: a second interface, whose flags lagged
  `start`'s before (no `--file`, `--image`, `--model`, `--set`).
