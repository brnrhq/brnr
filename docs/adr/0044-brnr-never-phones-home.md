# 44. brnr never phones home

Proposed 2026-10-08. Amends ADR 1: adds P15. Nothing to implement: it holds
of brnr as it is (see Context).

## Context

brnr sits between people and their agents. It sees their prompts, their
code, the agent's answers and their approvals, and keeps them in transcripts.
Whatever it sent anywhere, even a harmless counter, would ask the user to
trust that it sends nothing else. Saying it never sends anything is worth
more than what it could learn.

P13 already says brnr listens on no network. Nothing says it doesn't call
out. What reaches the network today, checked against the code in October
2026:

- brnr's own code (`src/`) opens Unix sockets only: the control socket
  (`UnixListener`, `UnixStream`) and the socketpairs between `brnr acp` and
  its process. It has no HTTP client, resolves no names and checks for no
  updates; none of its five crates does networking
  (`agent-client-protocol-schema`, `libc`, `serde`, `serde_json`, `toml`).
- What it runs, the user chose: the agent and its adapter (the profile's
  `agent`), bridges and `notify` commands (ADR 35, ADR 36). The agent talks
  to its provider; MCP servers given by `url` (ADR 31) are reached by the
  agent, not by brnr. `brnr doctor` runs the adapters only with `--version`.
- The adapters (`adapters/`) are the upstream npm packages, unchanged; what
  brnr adds is a launcher that finds the user's `claude` or `codex` on disk
  (ADR 37). They make no requests of their own beyond the agent's, to its
  provider.
- Installing does reach the network, because the user runs it: Homebrew
  downloads brnr, and `bun install` fetches the adapters' packages (ADR 37).

## Decision

### P15. brnr never phones home

brnr makes no network requests of its own. It collects and sends no
telemetry, usage data, error reports or update checks. What reaches the
network does so because the user configured it: the agent's (and its
adapter's) traffic, a bridge's, a `notify` command's, an MCP server's.

- A feature that needs the network is a command the user runs or configures,
  never a default and never in the background.
- What brnr knows about a session it keeps on the machine, readable only by
  the user (P13).
- The principle is about brnr, the program. The project's website and
  repository are outside it; what they count is said where they say it
  (#40).

## Consequences

- What the project would learn from telemetry it learns otherwise, from what
  people choose to share or from counts that are public anyway (#40):
  release download counts, GitHub's traffic numbers, and GitHub Discussions.
- Error reports are the user's to make. `brnr doctor --report` (#24) is to
  print a report to read before pasting, with secrets redacted (ADR 25), and
  a panic a pre-filled issue link. Nothing is sent unless the user opens
  it.
- brnr doesn't tell the user a new release is out; Homebrew and the release
  page do.
- An adapter brnr bundles is checked against P15 when its version moves: it
  may reach the agent's provider, not anyone of brnr's.
- The README says it, in one line.

## Considered

- Opt-in telemetry in the style of Go's transparent telemetry: counters kept
  locally, uploaded only once the user turns it on, to a public dataset, with
  the list of counters published. It is the best form of telemetry, and
  rejected for now: it needs a server and someone to run it, its counters
  would be one more thing to trust in a tool whose point is that it is
  trusted, and the questions it would answer (which agents, which editors,
  what breaks) are answered well enough by what users share (#24, #40). A
  later ADR could revisit it; it would amend this one.
- Opt-out telemetry: the user's data would leave before they knew to stop
  it.
- Widening P13 instead of adding a principle: P13 is about who can reach
  brnr on the machine; this is about what brnr reaches. Kept apart, each can
  be cited for what it says.
- A badge in the README: the line says it with the link; a badge adds
  nothing a reader can check.
