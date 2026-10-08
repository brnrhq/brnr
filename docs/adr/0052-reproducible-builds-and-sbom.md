# 52. Release builds are checked for reproducibility and carry an SBOM

Accepted 2026-10-08. Implemented.
Amends 40 and 47: checks before publication and after the tap update.

## Context

Build provenance says where the source archive came from. It does not say
that rebuilding it gives the same binary, list its dependencies in a
machine-readable form, or prove that the published Homebrew formula works.
Issue #37 asks for those checks. A failed check must fail the workflow
(P3, P7), before publication wherever possible.

## Decision

- On pull requests, main, and before a tag is published, Linux and macOS
  each build the committed source twice with `cargo build --release
  --locked`. Each build has its own source and target directories, without
  a target cache, and runs at a different wall-clock time. Both use the
  commit timestamp as `SOURCE_DATE_EPOCH` and remap the source directory to
  `/brnr` with `--remap-path-prefix`. Their SHA-256 hashes are printed and
  must match. The tag calls the same workflow that checks pull requests.
- Before publication, pinned `cargo-cyclonedx` generates a CycloneDX 1.5
  JSON SBOM, including transitive Rust dependencies across all targets.
  It must leave `Cargo.lock` unchanged. The release attaches it as
  `brnr-X.cdx.json`, with build provenance alongside the source archive.
- After the release and tap update, a separate, fresh macOS runner runs
  `brew install brnrhq/tap/brnr`, checks `brnr --version` against the tag,
  and runs `brnr doctor`. It uses the public formula and source download,
  with no checkout or local build to fall back to.

## Consequences

- Reproducibility is checked within each runner's toolchain and OS. This
  does not promise identical binaries across Rust versions, platforms or
  linkers; stable Rust and runner images still move. Releases still ship
  source, not prebuilt binaries.
- The SBOM describes brnr's Rust crate. It does not inventory the adapters'
  npm dependencies or claim that an inventoried dependency is safe.
- The smoke test fails the release workflow if installation, version
  checking or doctor fails. Publication and the tap push have already
  happened and cannot be rolled back by this check. Missing optional
  adapters are informational to doctor and need no credentials.

## Considered

- Reusing one target directory: Cargo may reuse the first binary, which
  proves nothing about an independent rebuild.
- Checking only on main: a tag can point at a commit without a successful
  main run. Publication must depend on the check itself.
- Running the smoke test from the release checkout: it can accidentally
  test a local binary, bypassing what the user downloads and installs.
