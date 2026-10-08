# Contributing

Bug reports, ideas and pull requests are welcome. For anything bigger than a
fix, open an issue first: brnr's behaviour is decided in
[ADRs](docs/adr/README.md), and a change to it starts there.

## Building and testing

```sh
cargo build --release
cargo fmt --check
cargo clippy --release --all-targets -- -D warnings
cargo test --release           # release, as CI: some tests stream tens of thousands of events
adapters/build.sh              # only if you change adapters/ (needs bun)
adapters/check.py --brnr target/release
```

CI runs the same on Linux and macOS, on every pull request; all of it has to
pass before a pull request can merge. The integration tests (`tests/`) drive
the real binary against `tests/fake_agent.py`, so they need `python3`. A
change in behaviour comes with a test that shows it.

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
