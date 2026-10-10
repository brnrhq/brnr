# 39. Which adapter version is built, and how it shows

Accepted (former decision 30); reviewed 2026-10-07. Implemented.

Amended by 63: `brnr status` is `brnr session status`.

## Decision

- Each adapter formula's version is its npm package's
  (`brnr-claude-adapter 0.85.1`), written by the release workflow from
  `adapters/package.json`. A brnr release that doesn't move the pin changes
  the formula's source but not its version, so nobody rebuilds for nothing;
  `brew outdated` shows an adapter when its package does move. When
  `adapters/` changes without a new npm version, `release.sh` says so: bump
  the formula's `revision` by hand if users need the new build. That is any
  change in `adapters/` but the pins' files (`package.json`, `bun.lock`),
  and theirs too when no pin moved: `bun.lock` alone moving is a dependency
  of a pinned package moving, which changes the build as much. The release
  workflow drops the revision when the npm version next moves, since a new
  version starts the count over.
- The adapters say what they were built from (`--version`, compiled in), and
  `brnr doctor` shows it, and the version of npm-installed adapters (from the
  `package.json` their bin link leads to). `brnr status` shows what the agent
  says it is in `initialize` (`agentInfo`).
- Dependabot opens a pull request for new releases of the two packages,
  weekly; the next brnr release ships them. A nightly workflow
  (`canary.yml`) builds the newest releases first, without moving the pins,
  checks them with brnr and opens an issue when they fail, so a breaking
  release is known before Dependabot proposes it.

## Consequences

The pin matters more than it first seemed. The adapters behave differently
from each other, and from one version to the next, wherever ACP leaves room
(ADR 18); brnr's tests run against exactly the pinned builds.

## Considered

- A brnr release for every upstream release: batching them into brnr's
  releases keeps the pace a person's.

## Tests

Run `cargo test --release adr_0039_`. Named claims and their assertions:

- [tests/doctor.rs](../../tests/doctor.rs)
  - `adr_0039_adapter_versions`.
