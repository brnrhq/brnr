# Install

```sh
brew install brnrhq/tap/brnr
brew install brnrhq/tap/brnr-claude-adapter   # for Claude Code, compiled on your machine
brew install brnrhq/tap/brnr-codex-adapter    # for Codex
```

The adapter formulae compile the community's ACP adapters for Claude Code and
Codex into standalone executables that need no Node.js ([Adapters](guide/adapters.md#adapters)
says whose they are), and link them next to `brnr`, where brnr finds them
even when an editor's `PATH` doesn't include Homebrew. Each is versioned by
the npm package it builds, so `brew upgrade` rebuilds an adapter when that
package moves. `brnr doctor` shows which version each adapter was built from.

Or brnr alone, from [crates.io](https://crates.io/crates/brnr), with the
adapters from Homebrew or npm:

```sh
cargo install brnr --locked
```

The crate is the binary. Its library is the command line as functions:
`brnr::run`, and a typed facade, a function for each command that returns
what its `--json` prints
([ADR 8.5](adr/0008-the-command-line-and-compatibility.md#85-the-library-is-private-and-its-facade-mirrors-the-command-line),
[reference](reference/overview.md)).

Or from source, below.

The formulae build from the source tarball on each
[release](https://github.com/brnrhq/brnr/releases), `brnr-X.tar.gz` for
version X, which the release workflow makes from the tag and attests. To
check one you downloaded was made that way, with its version for X:

```sh
gh attestation verify brnr-X.tar.gz -R brnrhq/brnr
```

Each release also carries `brnr-X.cdx.json`, an attested CycloneDX SBOM of
brnr's Rust dependencies across all targets (not the adapters' npm packages).
CI checks that two clean release builds, in different directories and at
different times, have the same SHA-256 on each of Linux and macOS. This
checks reproducibility within one toolchain and runner, not across Rust or
OS versions. After publishing and updating the tap, a fresh macOS runner
installs the formula, runs its `brew test`, checks its version against the
tag, and runs `brnr doctor`
([ADR 11.7](adr/0011-building-releasing-and-support.md#117-reproducible-builds-and-an-sbom)).

More at
[brnrhq.github.io/brnr](https://brnrhq.github.io/brnr/).

# Build

```sh
cargo build --release          # target/release/brnr
adapters/build.sh              # optional: target/release/brnr-claude-adapter, brnr-codex-adapter (needs bun)
```

`brnr` is one binary. The adapters are separate, single-file builds of the ACP
adapters for Claude Code and Codex; see [Adapters](guide/adapters.md#adapters).
