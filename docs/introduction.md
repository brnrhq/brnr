# brnr

[![License](https://img.shields.io/github/license/brnrhq/brnr)](https://github.com/brnrhq/brnr/blob/main/LICENSE)
![Platforms](https://img.shields.io/badge/platforms-macOS%20%7C%20Linux-lightgrey)
[![Rust](https://img.shields.io/badge/dynamic/toml?url=https%3A%2F%2Fraw.githubusercontent.com%2Fbrnrhq%2Fbrnr%2Fmain%2FCargo.toml&query=%24.package%5B%27rust-version%27%5D&label=rust&suffix=%2B)](https://github.com/brnrhq/brnr/blob/main/CONTRIBUTING.md#building-and-testing)
[![CI](https://github.com/brnrhq/brnr/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/brnrhq/brnr/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/brnrhq/brnr)](https://github.com/brnrhq/brnr/releases/latest)
[![Homebrew](https://img.shields.io/badge/brew-brnrhq%2Ftap%2Fbrnr-orange)](install.md#install)
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
editor's session is [experimental](guide/editor.md#experimental-actions).)

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
  a group's ([ADR 8.1](adr/0008-the-command-line-and-compatibility.md#81-commands-are-grouped-by-what-they-act-on-with-acps-verbs)).
  `--help` exits 0, and a command used wrongly exits 2 with its usage on
  stderr, having done nothing ([ADR 8.3](adr/0008-the-command-line-and-compatibility.md#83-waiting-and-exit-status)).

macOS and Linux (Unix sockets only). Why brnr does what it does, and what else
was considered, is in [docs/adr](adr/README.md).

brnr never phones home: it makes no network requests of its own and sends
no telemetry ([P15](adr/0001-principles.md)). Who can
reach a session, and what keeps others out, is in the
[threat model](threat-model.md).

## Interface reference

The [external interface reference](interface.md) describes command
syntax, inputs and outputs, JSON results, exit codes, configuration, and
the bridge/control-socket protocol. Use it when writing scripts or bridges;
the sections below provide examples and explain the workflows.
