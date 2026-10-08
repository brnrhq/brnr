# 47. brnr on crates.io, published by the release workflow

Proposed 2026-10-08. Implemented, once the first version is published by
hand (trusted publishing can only be set up for a crate that exists).
Amends 40: `release.sh tag` also checks crates.io.

## Context

Nobody held the name `brnr` on crates.io. `cargo install brnr` is what a
Rust user tries first, and anyone could publish a crate by that name. A
tool that runs coding agents with the user's credentials makes a good lure
for someone impersonating it. Homebrew was the only install channel, which
leaves Linux users without Homebrew to build from a clone.

## Decision

- brnr is published to crates.io as `brnr`, at every release, by the
  release workflow: a `crates` job after the GitHub release and the tap,
  using crates.io's trusted publishing. It has no stored token. The job
  exchanges GitHub's OIDC token for a short-lived crates.io token
  (`rust-lang/crates-io-auth-action`, which revokes it when the job ends).
  The crates.io side trusts only `release.yml` in `brnrhq/brnr`, in the
  `crates-io` environment.
- Before anything is made public, the release job runs `cargo publish
  --dry-run`. A version that won't package stops before the GitHub release,
  and a version already published can't be replaced.
- The package `include`s only what the build needs: `src/`, `skills/` (which
  `brnr skill` embeds), the README and the license. The tests, docs, site
  and workflows stay in the repository.
- `cargo install brnr --locked` is documented next to Homebrew. It installs
  brnr only. The adapters still come from Homebrew or npm (ADR 37), because
  a crate can't build them.
- `release.sh tag` checks that crates.io has the new version, as it checks
  the tap.

## Consequences

- A crates.io version can be yanked but not deleted. A bad release is fixed
  by the next one, as with the tap.
- The first version is published by hand, with a token scoped to `brnr` and
  `publish-new`, which is then revoked. After that, a token isn't needed
  again.

## Considered

- An empty placeholder crate to hold the name: crates.io's policy
  discourages it, and the real crate costs no more.
- A `CARGO_REGISTRY_TOKEN` secret: a long-lived credential that can publish,
  sitting in the repository's secrets. Trusted publishing needs none.
- Publishing from `release.sh` on the maintainer's machine: the same
  long-lived token, kept locally, and a publish that doesn't come from the
  tested, tagged build.
- Prebuilt binaries through `cargo binstall`: binstall can already fall back
  to building from source. Release binaries are a separate decision.
