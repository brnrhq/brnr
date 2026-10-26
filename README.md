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


**A burner phone for your coding agents.**

brnr keeps a coding agent reachable outside the editor that started it, and
lets its session carry on after the editor closes. You can watch, steer and
approve many agents from one terminal, from your phone, or from another
agent.

- **The session outlives the window.** Your editor is one way in, not the
  only one: close it and carry its session on headless
  (`brnr session resume`), or start sessions with no editor at all.
  Watching an editor's session always works; acting on it is
  [experimental](https://brnrhq.github.io/brnr/docs/guide/editor.html#experimental-actions),
  enabled per action in a profile.
- **One place for many agents.** `brnr session list` and
  `brnr permission requests` show every session and every waiting approval,
  from editors and headless runs.
- **Agents that drive agents.** A CLI with `--json`, documented exit codes,
  events and a [bundled skill](https://brnrhq.github.io/brnr/docs/guide/agents.html):
  Claude Code or Codex can start workers, give them work, wait for them,
  and answer their approvals.
- **Reach it from anywhere.** [Bridges](https://brnrhq.github.io/brnr/docs/guide/profiles.html#bridges),
  programs you write or choose, forward events to Slack or push
  notifications, and send your answers back.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="https://raw.githubusercontent.com/brnrhq/brnr/main/site/brand/diagram-dark.svg">
  <img src="https://raw.githubusercontent.com/brnrhq/brnr/main/site/brand/diagram-light.svg" width="960" alt="The editor runs brnr acp, which relays bytes to the brnr process over socketpairs; the brnr process owns the agent and talks to it over pipes. You, from a terminal, and other agents reach the process over its control socket, and bridges a profile starts talk to it over pipes. A headless session has no editor and no brnr acp.">
</picture>

How the parts fit is in the
[introduction](https://brnrhq.github.io/brnr/docs/introduction.html#how-it-works).
brnr runs on macOS and Linux, and never phones home: it makes no network
requests of its own and sends no telemetry. Who can reach a session, and
what keeps others out, is in the [threat model](https://brnrhq.github.io/brnr/docs/threat-model.html).

**What brnr keeps.** brnr records each session in `~/.brnr` (or
`$BRNR_HOME`), readable only by you: its events and raw ACP under
`projects/`, which hold your prompts, the agent's replies, tool calls with
their input and output, and diffs of your code, and a log per process under
`hosts/`. By default it keeps them all, with no time limit, until you
delete them. `brnr session prune --older-than <s>` deletes what was last
active more than `<s>` seconds ago, and `retention = <s>` in the config has
every process do that as it starts; neither touches a session that is open.
A profile's `log = "events"` or `log = "off"` records less, or nothing.
[Transcripts](https://brnrhq.github.io/brnr/docs/guide/transcripts.html#how-long-brnr-keeps-them)
has the details.

## Install

```sh
brew install brnrhq/tap/brnr
brew install brnrhq/tap/brnr-claude-adapter   # for Claude Code, compiled on your machine
brew install brnrhq/tap/brnr-codex-adapter    # for Codex
brew install brnrhq/tap/brnr-muse-adapter     # for Muse Code, built from Rust source
```

Or brnr alone, from [crates.io](https://crates.io/crates/brnr), with the
adapters from Homebrew or npm:

```sh
cargo install brnr --locked
```

Muse uses your own `muse` CLI and login; its npm alternative is the scoped
`@brokkai/muse-acp`. [Adapters](https://brnrhq.github.io/brnr/docs/guide/adapters.html)
has the ACP v1 integration details.

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

## How light it is

`brnr acp` relays bytes to a process that reads ACP once. What that adds,
on an Apple M4 Pro (14 cores, macOS 26.4) busy with other work, brnr 0.10.0
(81ae7d0), against a fake agent that answers at once:

| | Direct pipe | Through `brnr acp` |
|---|---:|---:|
| A prompt's round trip (median) | 10 µs | 68 µs |
| A turn that streams 20 MB | 784 MB/s | 157 MB/s |

At rest, `brnr acp` takes 2.7 MiB of memory and 3 threads, and its process
3.9 MiB and 12 threads; each connection to the process adds 2 threads, and
each bridge 4. A real agent takes far longer than this to answer, so what
brnr adds is a small part of a turn. Every pull request measures these
numbers against budgets ([N3](https://brnrhq.github.io/brnr/docs/adr/0001-principles.html#n3-performance)).

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
  [agents with ACP of their own](https://brnrhq.github.io/brnr/docs/guide/native-agents.html)
  (Junie, Cursor),
  [doctor](https://brnrhq.github.io/brnr/docs/guide/doctor.html)
- [Interface reference](https://brnrhq.github.io/brnr/docs/reference/overview.html): commands, JSON, exit codes,
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
