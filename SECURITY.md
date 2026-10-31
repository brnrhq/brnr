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

From 1.0.0, security fixes are made on the latest major version, in a new
release of it. Upgrading within that major is the fix, and it breaks nothing
the compatibility promise covers
([ADR 8.4](docs/adr/0008-the-command-line-and-compatibility.md#84-what-the-compatibility-promise-covers)). There is
one major version so far, so no support period for older majors is set yet;
it will be once a second major exists.

## In scope

[docs/threat-model.md](docs/threat-model.md) says who can reach what, how
brnr keeps it so, the test for each claim, and the known gaps.

- Reaching a session from outside: another user connecting to a control
  socket, reading or writing the runtime directory, metadata or session
  locks.
- Approvals: one answered, or a tool call run, without the user, a bridge or
  `response_timeout` answering it as configured.
- Secrets: a value brnr knows is secret (ADR 9.1) written unredacted to a
  transcript, host log or event; a transcript readable by others.
- `brnr acp` changing what passes between the editor and the agent beyond
  what the documentation and ADRs name.
- The release: the workflows, the Homebrew formulae and the adapters
  brnr builds (`adapters/`).

## Out of scope

- The agents (Claude Code, Codex, Muse Code) and the upstream ACP adapters; report those
  to their projects. A flaw in how brnr builds or runs them is in scope.
- What an agent does with an approval its user gave.
- Anyone who can already run code as the user. That includes an agent
  allowed to run `brnr`: with any arguments, it can run any command as the
  user, start an agent that asks nothing, and answer the approvals of one it
  started. brnr is not a permission boundary between an agent and the
  agents it starts; [brnr from a restricted agent](docs/guide/agents.md#brnr-from-a-restricted-agent)
  says which boundaries hold, and how to set them. Even without `--fix`,
  `doctor` runs adapter `--version` probes resolved beside brnr or on PATH;
  `remote check` starts system SSH and the destination's doctor, including
  its probes. Diagnostic arguments do not guarantee side-effect-free
  executables. Deny both commands whole when the policy forbids program
  starts, or explicitly scope an exception to trusted executables and
  destinations.
