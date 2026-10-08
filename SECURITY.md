# Security

brnr runs coding agents and relays their approvals, so a flaw in it can let
someone act as the agent's user. Please report one privately.

## Reporting

Use [Report a vulnerability](https://github.com/brnrhq/brnr/security/advisories/new)
on the repository's Security tab. It opens a private advisory only the
maintainers can read. Please don't open a public issue, pull request or
discussion for it.

Say what an attacker can do, and how: the brnr version (`brnr --version`), the
OS, the agent and adapter, and steps or a proof of concept. `brnr doctor
--report` has most of that, but read it first: it names paths and sessions,
and has the end of a host log.

You'll get a reply within a week. Once a fix is released, the advisory is
published with credit to you, unless you'd rather not be named.

## Supported versions

Before 1.0, only the latest release gets fixes; upgrading is the fix
(`brew upgrade brnr`).

## In scope

[docs/threat-model.md](docs/threat-model.md) says who can reach what, how
brnr keeps it so, the test for each claim, and the known gaps.

- Reaching a session from outside: another user connecting to a control
  socket, reading or writing the runtime directory, metadata or session
  locks.
- Approvals: one answered, or a tool call run, without the user, a bridge or
  `permission_timeout` answering it as configured.
- Secrets: a value brnr knows is secret (ADR 25) written unredacted to a
  transcript, host log or event; a transcript readable by others.
- `brnr acp` changing what passes between the editor and the agent beyond
  what the README and ADRs name.
- The release: the workflows, the Homebrew formulae and the adapters
  brnr builds (`adapters/`).

## Out of scope

- The agents (Claude Code, Codex) and the upstream ACP adapters; report those
  to their projects. A flaw in how brnr builds or runs them is in scope.
- What an agent does with an approval its user gave.
- Anyone who can already run code as the user.
