# AGENTS.md

For coding agents working on brnr itself. (Agents that *use* brnr to run
other agents get a skill instead: [skills/brnr](skills/brnr/SKILL.md),
ADR 46.) People: [CONTRIBUTING.md](CONTRIBUTING.md) says the same at more
length.

## What brnr is

One Rust binary that sits between an editor and an ACP agent, and owns the
agent in a process of its own that other commands can reach over a Unix
socket. The diagram is at the top of [src/lib.rs](src/lib.rs) and the
[README](README.md). macOS and Linux only.

## Decisions come first

- [ADR 1](docs/adr/0001-principles.md) has the principles (P1 to P14, and
  P15 in [ADR 44](docs/adr/0044-brnr-never-phones-home.md)) every decision
  is measured against; [docs/adr](docs/adr/README.md) has the
  decisions. Read the ADRs a change touches before changing it. Modules say
  which ADR they implement in their first doc line.
- A change in what brnr does follows an ADR or adds one (the format is in
  docs/adr/README.md). One that breaks a principle says which, and why.
- In a design discussion, settle the principle at stake first, and show the
  current sequence of events, traced from the code, as a numbered diagram
  before asking questions about a flow. Recommend, citing principles.
- No backwards compatibility before 1.0: a rename or change replaces the old
  form, with no aliases or shims. Say so in the pull request, and in
  CHANGELOG.md's `[Unreleased]`, the next release's notes, under Changed or
  Removed, starting with **Breaking:**.
- From 1.0.0, what [ADR 64](docs/adr/0064-what-the-compatibility-promise-covers.md)
  covers is kept within a major version: commands, flags and arguments,
  `--json`, exit codes, events, a started bridge's protocol, transcripts,
  config TOML, ACP and what `brnr acp` passes through. Add freely; a
  removal, rename, or change of a field's type or meaning or of an exit code
  waits for the next major, the old form deprecated and working until then.
  Where files are under `$BRNR_HOME`, and how the runtime directory is
  found, are covered; text output, help, error wording, what host logs and
  lock files contain, the control socket (private, not an interface) and
  what is in the runtime directory aren't.

## Where things are

- `src/lib.rs`: `brnr::run`, the dispatch, which `src/main.rs` calls. `acp`
  → `proxy.rs` (the editor's child, relays bytes), `host` → `host/` (owns
  the agent), the rest → `ctl.rs` and `ctl/`. `run` is the library's one
  public item; the modules are private (ADR 66), and the tests and fuzz
  targets reach them through `brnr::internals` (the `internals` feature).
- `src/host/`: `mod.rs` the process and its event loop, `acp.rs` the ACP
  stream, `control.rs` the socket and bridges, `state.rs` what the agent has
  said, plus one file per ADR-sized concern (start, death, flow, display,
  strict, experimental).
- `src/render.rs`: events as text, the one way sessions are shown (P5).
- `src/sys.rs`: the system calls that are safe whatever their arguments.
- `tests/`: integration tests that drive the real binary against
  `tests/fake_agent.py`, whose behaviour is chosen with environment variables
  (its docstring lists them). `tests/common/mod.rs` has the helpers (`Env`);
  `tests/security.rs` checks the claims of the threat model.
  `tests/docs.rs` runs the examples in the README, the site and the skill
  (`skills/brnr/`): an example added or changed needs its line in
  `EXAMPLES` there (or `SKIPPED`, with why).
- `fuzz/`: cargo-fuzz targets for the parsers that read bytes from outside
  (frame.rs, json.rs, the host through `host::fuzz::Harness`), a workspace
  of its own on nightly; seeds in `fuzz/seeds/`. CI fuzzes them with
  ClusterFuzzLite (`.clusterfuzzlite/`, `.github/workflows/fuzz.yml`).
- `docs/threat-model.md`: who can reach what, each claim with what enforces
  it and its test, and the gaps. A change to a boundary (permissions, the
  socket, redaction, approvals, what `brnr acp` changes) updates it.
- `adapters/`: the Claude Code and Codex ACP adapters, built with bun.
- `docs/adr/`, `README.md`, `site/`: what brnr does, for people.

## Checks

All of these must pass; CI runs them on Linux and macOS:

```sh
cargo fmt --check
cargo clippy --release --all-targets -- -D warnings
cargo test --release           # release: some tests stream tens of thousands of events
ruff check && ruff format --check
shellcheck release.sh adapters/build.sh .clusterfuzzlite/build.sh
```

Change `adapters/`? Also `adapters/build.sh` and `adapters/check.py --brnr
target/release` (`--brnr`: brnr, built there, is checked with them); change
`check.py`? `adapters/test_check.py`. CI also
runs `cargo deny check`, `cargo vet --locked`, the minimum Rust version
(`rust-version` in Cargo.toml) and actionlint and zizmor on the workflows.

## Writing code here

- Match the code around you: compact, comments that say why, doc comments
  that say what in a sentence. rustfmt's config keeps short things on one
  line.
- Every `unsafe` block has a `// SAFETY:` comment saying why it is sound
  (clippy denies one without). A call that is sound whatever its arguments
  goes in `sys.rs` as a safe function instead.
- A new crate needs a reason: there are five, `cargo deny` checks their
  licenses and advisories, `cargo vet` that each crate is audited or exempted
  (`supply-chain/`; CONTRIBUTING.md has the routine), and few is the point.
  Never certify an audit you didn't do: an unread crate is an exemption.
- What brnr prints, it prints the same in text and `--json` (P5). It never
  guesses or picks for the user (P4), and never drops something silently
  (P3): an error, an exit status or an event says so.
- A behaviour change comes with a test that shows it, and the README and the
  ADRs say what the code now does. What users notice gets its line under
  `[Unreleased]` in CHANGELOG.md, with its ADR, under its
  [Keep a Changelog 1.1.0](https://keepachangelog.com/en/1.1.0/) change type.

## Git

- Work on a branch and open a pull request against `main`; `main` only
  changes through pull requests that pass CI. Pull requests are merged, not
  squashed (`.git-blame-ignore-revs` lists commits by SHA).
- A commit's subject says what is now true, in a sentence (`event notify, cut
  off, stops its command and says so`), with the ADR it implements in
  brackets: `(ADR 36)`. The body says why.
- No "generated by" or co-author lines for agents in commits or pull
  requests.
- Releases, tags and the Homebrew tap are the maintainer's (`release.sh`).
  Don't tag, release or push to `brnrhq/homebrew-tap`.
