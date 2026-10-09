# Contributing

Bug reports, ideas and pull requests are welcome. For anything bigger than a
fix, open an issue first: brnr's behaviour is decided in
[ADRs](docs/adr/README.md), and a change to it starts there.

## Requirements for acceptable contributions

Before a pull request can be accepted:

- Follow the [coding standards](#coding-standards) below and the existing
  [design decisions](#decisions). Explain what changes and why in the pull
  request, including the ADR it follows or adds when behaviour changes.
- Add tests that demonstrate changed behaviour and catch regressions.
  Update the documentation and examples that describe that behaviour, as
  explained under [pull requests](#pull-requests).
- Pass the required [build, test and lint checks](#building-and-testing)
  in CI. Report which checks you ran locally and any you could not run.
- Justify new dependencies and meet the [dependency requirements](#dependencies).
  Never certify an audit without reviewing the code; record an exemption
  explicitly when it has not been audited.
- Contribute under the project's [Apache-2.0 license](#license).

## Coding standards

Match the surrounding code. Keep implementations compact, use comments to
explain why, and write doc comments that describe what an item does.
The same conventions are collected in [AGENTS.md](AGENTS.md#writing-code-here)
for coding agents; they apply to human contributions too.

- Rust follows the repository's rustfmt configuration and must pass
  `cargo fmt --check` and Clippy with warnings denied. Every `unsafe` block
  needs a `// SAFETY:` comment explaining why it is sound. Put system calls
  that are safe for all arguments behind safe functions in `src/sys.rs`.
- Python must pass Ruff's lint and formatting checks. Shell scripts must
  pass ShellCheck. GitHub Actions workflows must pass actionlint and zizmor.
  [ci.yml](.github/workflows/ci.yml) defines the commands and tool versions.
- Keep text and `--json` output consistent. Report errors or dropped work
  explicitly. A change to permissions, sockets, redaction or approvals
  must update the [threat model](docs/threat-model.md) and its tests.

## Building and testing

```sh
cargo build --release
cargo fmt --check
cargo clippy --release --all-targets -- -D warnings
cargo test --release           # release, as CI: some tests stream tens of thousands of events
adapters/build.sh              # only if you change adapters/ (needs bun)
adapters/check.py --brnr target/release
adapters/test_check.py         # only if you change adapters/check.py
```

CI runs the same on Linux and macOS, on every pull request; all of it has to
pass before a pull request can merge. The integration tests (`tests/`) drive
the real binary against `tests/fake_agent.py`, so they need `python3`. A
change in behaviour comes with a test that shows it.

`.github/workflows/coverage.yml` also runs the Rust unit and integration
tests on Linux with `cargo llvm-cov`. Its job summary lists each source
file's coverage, including files with no hits; its `coverage` artifact has
the same summary and an HTML report of uncovered lines, kept for 90 days.
The table reports line, region, and function coverage, with an overall
`TOTAL`; it does not measure branch coverage.
Compare successive `main` runs in Actions for the trend. Coverage is
informational: the job is allowed to fail, has no percentage threshold,
and does not replace the required tests above. There is no badge yet.

To produce the report locally:

```sh
rustup component add llvm-tools-preview
cargo install --locked cargo-llvm-cov --version 0.9.1
cargo llvm-cov --locked --release --all-targets --no-report
cargo llvm-cov report --release --ignore-filename-regex '/tests/'
cargo llvm-cov report --release --ignore-filename-regex '/tests/' --html
```

Open `target/llvm-cov/html/index.html`. The integration tests launch the
instrumented `CARGO_BIN_EXE_brnr` and preserve `LLVM_PROFILE_FILE` for its
children, so their execution contributes too. Tests are excluded from the
report, not from the run. Profiles are written on normal exit: work in
processes killed by a signal (including test cleanup's SIGKILL), or leaving
through `_exit`, can be missing. This is an execution baseline, not proof
that every behaviour is asserted; it also excludes doc tests and macOS-only
paths in CI.

The parsers that read bytes from outside brnr have fuzz targets in `fuzz/`
(frames, ACP lines, the host fed ACP from both sides), which CI fuzzes
with ClusterFuzzLite. To run one, on nightly with `cargo install cargo-fuzz`:
`cd fuzz && mkdir -p corpus/host && cargo +nightly fuzz run -O -a host
corpus/host seeds/host` (`cargo fuzz list` has the others). `-O -a` builds
it as CI does: optimized, with debug assertions and overflow checks on, so
a `debug_assert!` or an integer overflow an input reaches is a crash. (`-O`
alone turns them off; with neither, cargo-fuzz builds the same as `-O -a`.)
A crash it finds is fixed with a regression test in the main test suite.

Once a week, `.github/workflows/sanitizers.yml` runs the tests with brnr
built under AddressSanitizer and under MemorySanitizer, on Linux. To run
them yourself (nightly Rust, with `rust-src`), and see any
report, also the host's, in `/tmp/reports`:

```sh
mkdir -p /tmp/reports
RUSTFLAGS="-Zsanitizer=address --cfg sanitized" ASAN_OPTIONS=detect_leaks=1:log_path=/tmp/reports/asan \
  cargo +nightly test -Zbuild-std --target x86_64-unknown-linux-gnu --release
ls /tmp/reports                # empty: nothing was found
```

MemorySanitizer is `-Zsanitizer=memory`, with `MSAN_OPTIONS`. On a Mac: `--target aarch64-apple-darwin` and
`detect_leaks=0` (macOS has no leak detection, and no MemorySanitizer).

The minimum Rust version is `rust-version` in `Cargo.toml`: what the code
needs, not older. Raising it for a feature worth having is fine; say so in
the pull request.

## Dependencies

brnr has five direct dependencies, and few is the point. A new one needs:

- a reason, in the pull request: what it does that brnr shouldn't do itself;
- licenses and advisories that pass `cargo deny check` (`deny.toml`);
- a [cargo vet](https://mozilla.github.io/cargo-vet/) audit for it and each
  crate it brings, or an exemption saying it isn't audited yet
  (`supply-chain/`). CI runs `cargo vet --locked`.

Audits are imported from Mozilla, Google, the Bytecode Alliance, Embark,
ISRG and Zcash (`supply-chain/config.toml`); crates none of them has audited
at the version brnr uses are exemptions, to be replaced by audits.
The [audit status](supply-chain/README.md) describes the remaining work and
the scope of the local reviews. `vet` is a required check on `main`.
The workspace's `brnr` package is first-party (`audit-as-crates-io = false`),
even when its version is published; its dependencies still need coverage.

When a pull request moves crate versions (Dependabot's weekly `crates` one,
say), `vet` fails for each new version nobody has audited. On its branch:

```sh
cargo vet                       # fetches the importers' latest audits; often enough
cargo vet suggest               # what is left, smallest diff first
cargo vet diff serde 1.0.228 1.0.229   # read it, then:
cargo vet certify serde 1.0.228 1.0.229
cargo vet prune                 # drops exemptions and imports nothing needs now
```

and push the changes in `supply-chain/` to the branch. A version not worth
reading yet becomes an exemption instead (`cargo vet regenerate exemptions`),
said in the pull request.

## Decisions

[ADR 1](docs/adr/0001-principles.md) has the principles (P1 to P14, and P15
in [ADR 44](docs/adr/0044-brnr-never-phones-home.md)) every decision is
measured against. A pull request that changes what brnr does
either follows an ADR or adds one, as [docs/adr](docs/adr/README.md)
describes; one that breaks a principle says which, and why. Before 1.0 there
is no backwards compatibility: a change replaces what it changes, with no
aliases or shims.

## Pull requests

- Against `main`, which changes only through pull requests.
- Commits say what is now true, in a sentence (`notify, cut off, stops its
  command and says so`), with the why in the body. The ADR a commit
  implements goes in brackets: `(ADR 36)`.
- The README, the skill (`skills/brnr/`) and the ADRs change with the code
  they describe. Their examples, and the site's, run as tests
  (`tests/docs.rs`): a new one needs a line there saying how it runs, or why
  it can't.
- Releases are the maintainers' (`release.sh`, in the README).

## Security

Report vulnerabilities privately, as [SECURITY.md](SECURITY.md) says, not in
an issue.

## License

By contributing, you agree your contribution is licensed under the
[Apache License 2.0](LICENSE), as brnr is.
