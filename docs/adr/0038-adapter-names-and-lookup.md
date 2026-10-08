# 38. The adapters' names, and finding them next to brnr

Accepted (former decisions 27 and 28); reviewed 2026-10-07. Implemented.

## Decision

- The compiled adapters are called `brnr-claude-adapter` and
  `brnr-codex-adapter`. `brnr acp -- claude-agent-acp` still runs the npm
  package; `brnr doctor` reports both kinds.
- A bare agent name (no `/`) is looked for next to brnr first, then on PATH,
  so an editor's PATH needn't include Homebrew's. "Next to brnr" is the
  running binary's directory, and the directory of the path brnr was started
  by (`argv[0]`, when it is a path): Homebrew links `bin/brnr` and the
  adapters into its prefix's `bin`. On macOS the running binary's path is
  the link's, so an adapter next to the link is found; on Linux it is the
  link's target in the Cellar, where the adapters aren't. `brnr start` and
  `brnr acp`, started by that path, look, as whoever starts a process
  resolves everything (ADR 8), and pass the path found to the process,
  which is the one that starts the agent. A fixed rule, documented (P4). A
  started bridge's bare command (`~` expanded) is looked for the same way,
  in the same place, so `command = ["brnr", "notify", …]` works from an
  editor whose PATH lacks brnr, and a bridge linked into Homebrew's `bin` is
  found on Linux too (ADR 35, 36).

## Considered

- The npm packages' command names (`claude-agent-acp`, `codex-acp`), so
  docs and configs read the same either way: installed through Homebrew next
  to a global npm install, two commands of one name would be on PATH, and
  which one ran would depend on PATH order.
- Looking only next to the running binary: on Linux, through Homebrew's
  symlink, the adapters weren't found.
