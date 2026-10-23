# brnr

[![License](https://img.shields.io/github/license/brnrhq/brnr)](LICENSE)
![Platforms](https://img.shields.io/badge/platforms-macOS%20%7C%20Linux-lightgrey)
[![Rust](https://img.shields.io/badge/dynamic/toml?url=https%3A%2F%2Fraw.githubusercontent.com%2Fbrnrhq%2Fbrnr%2Fmain%2FCargo.toml&query=%24.package%5B%27rust-version%27%5D&label=rust&suffix=%2B)](CONTRIBUTING.md#building-and-testing)
[![CI](https://github.com/brnrhq/brnr/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/brnrhq/brnr/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/brnrhq/brnr)](https://github.com/brnrhq/brnr/releases/latest)
[![Homebrew](https://img.shields.io/badge/brew-brnrhq%2Ftap%2Fbrnr-orange)](#install)
[![crates.io](https://img.shields.io/crates/v/brnr)](https://crates.io/crates/brnr)

[![OpenSSF Scorecard](https://api.scorecard.dev/projects/github.com/brnrhq/brnr/badge)](https://scorecard.dev/viewer/?uri=github.com/brnrhq/brnr)
[![OpenSSF Best Practices](https://www.bestpractices.dev/projects/15315/badge)](https://www.bestpractices.dev/projects/15315)


A burner phone for your coding agents.

brnr sits between your editor and an [ACP](https://agentclientprotocol.com/)
agent (Claude Code, Codex, …). The editor talks to the agent exactly as before,
but the agent now lives in a process of its own that you can also reach from
outside the editor: watch everything its sessions do, and carry them on once
the editor has gone. Or run sessions with no editor at all, and send them
messages and answer their approvals from somewhere else. (Doing that to an
editor's session is [experimental](https://brnrhq.github.io/brnr/docs/guide/editor.html#experimental-actions).)

```text
editor ──stdio── brnr acp ──socketpairs── brnr process ──pipes── agent
                                             │
                                  control socket ── brnr prompt send / event watch / …
                                             │
                                          bridges (Slack, push, …)
```

- **brnr acp** is what the editor runs as its agent. It only relays bytes.
- **The brnr process** owns the agent for its whole life, out of the editor's
  process group and process tree. When the editor goes, the agent goes, as if
  the editor had run it. `brnr process list` lists these processes.
- **The other commands** (`brnr prompt send`, `brnr event watch`, …) talk to
  a session through its process, over a Unix socket that only you can open.
  Most are a group, what they act on, and a verb: `brnr <group> --help` lists
  a group's ([ADR 8.1](docs/adr/0008-the-command-line-and-compatibility.md#81-commands-are-grouped-by-what-they-act-on-with-acps-verbs)).
  `--help` exits 0, and a command used wrongly exits 2 with its usage on
  stderr, having done nothing ([ADR 8.3](docs/adr/0008-the-command-line-and-compatibility.md#83-waiting-and-exit-status)).

macOS and Linux (Unix sockets only). Why brnr does what it does, and what else
was considered, is in [docs/adr](docs/adr/README.md).

brnr never phones home: it makes no network requests of its own and sends
no telemetry ([P15](docs/adr/0001-principles.md)). Who can
reach a session, and what keeps others out, is in the
[threat model](docs/threat-model.md).

## Install

```sh
brew install brnrhq/tap/brnr
brew install brnrhq/tap/brnr-claude-adapter   # for Claude Code, compiled on your machine
brew install brnrhq/tap/brnr-codex-adapter    # for Codex
```

Or brnr alone, from [crates.io](https://crates.io/crates/brnr), with the
adapters from Homebrew or npm:

```sh
cargo install brnr --locked
```

[Install](https://brnrhq.github.io/brnr/docs/install.html) has the rest:
what the adapters are, building from source, and checking a release.

## Quick start

Configure your editor's ACP agent command as the first line, or start a
session with no editor; then reach it from outside. `$s` is a session's
id, as `brnr session list` shows it.

```sh
brnr acp -- brnr-claude-adapter  # or claude-agent-acp, from npm
brnr session new --cwd ~/work/project --prompt "fix the failing tests" -- brnr-claude-adapter
brnr session list                  # brnr's sessions: open ones and ended ones
brnr event watch $s                # live: messages, tools, plan, approvals
brnr prompt send $s --prompt "also update the changelog"               # held while a turn runs
brnr permission requests           # approvals waiting, in every session
brnr permission allow $s p1        # with its allow_once option; --always: allow_always
brnr doctor                        # checks what brnr depends on
```

## Documentation

The documentation is a book at
[brnrhq.github.io/brnr/docs](https://brnrhq.github.io/brnr/docs/):

- [Introduction](https://brnrhq.github.io/brnr/docs/introduction.html)
- [Install](https://brnrhq.github.io/brnr/docs/install.html) and build
- Guide: [use it from an editor](https://brnrhq.github.io/brnr/docs/guide/editor.html),
  [talk to it from outside](https://brnrhq.github.io/brnr/docs/guide/outside.html),
  [approvals and elicitations](https://brnrhq.github.io/brnr/docs/guide/approvals.html),
  [events and notifications](https://brnrhq.github.io/brnr/docs/guide/events.html),
  [headless sessions](https://brnrhq.github.io/brnr/docs/guide/headless.html),
  [for agents](https://brnrhq.github.io/brnr/docs/guide/agents.html),
  [profiles, bridges and environment](https://brnrhq.github.io/brnr/docs/guide/profiles.html),
  [transcripts](https://brnrhq.github.io/brnr/docs/guide/transcripts.html),
  [adapters](https://brnrhq.github.io/brnr/docs/guide/adapters.html),
  [doctor](https://brnrhq.github.io/brnr/docs/guide/doctor.html)
- [Interface reference](https://brnrhq.github.io/brnr/docs/interface.html): commands, JSON, exit codes,
  configuration and the bridge protocol, for scripts and bridges
- Design: [principles](https://brnrhq.github.io/brnr/docs/adr/0001-principles.html),
  [decision records](https://brnrhq.github.io/brnr/docs/adr/README.html),
  [threat model](https://brnrhq.github.io/brnr/docs/threat-model.html)
- Project: [releasing](https://brnrhq.github.io/brnr/docs/project/releasing.html),
  [changelog](https://github.com/brnrhq/brnr/blob/main/CHANGELOG.md)

## Contributing

[CONTRIBUTING.md](CONTRIBUTING.md) says how to build, test and propose a
change, including the [requirements for acceptable contributions](CONTRIBUTING.md#requirements-for-acceptable-contributions)
and [coding standards](CONTRIBUTING.md#coding-standards).
brnr has five direct dependencies; CI checks their licenses and
advisories with cargo deny, and with cargo vet that every crate is audited
(by Mozilla, Google and others whose audits brnr imports) or listed as an
exemption still to audit. The [audit status](supply-chain/README.md) records
the remaining work; an exemption is not an audit. Report vulnerabilities privately, as
[SECURITY.md](SECURITY.md) says.

## License

Apache-2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE).
