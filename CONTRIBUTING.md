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

The minimum Rust version is `rust-version` in `Cargo.toml`: what the code
needs, not older. Raising it for a feature worth having is fine; say so in
the pull request.

## Decisions

[ADR 1](docs/adr/0001-principles.md) has the principles (P1 to P14) every
decision is measured against. A pull request that changes what brnr does
either follows an ADR or adds one, as [docs/adr](docs/adr/README.md)
describes; one that breaks a principle says which, and why. Before 1.0 there
is no backwards compatibility: a change replaces what it changes, with no
aliases or shims.

## Pull requests

- Against `main`, which changes only through pull requests.
- Commits say what is now true, in a sentence (`notify, cut off, stops its
  command and says so`), with the why in the body. The ADR a commit
  implements goes in brackets: `(ADR 36)`.
- The README and the ADRs change with the code they describe.
- Releases are the maintainers' (`release.sh`, in the README).

## Security

Report vulnerabilities privately, as [SECURITY.md](SECURITY.md) says, not in
an issue.

## License

By contributing, you agree your contribution is licensed under the
[Apache License 2.0](LICENSE), as brnr is.
