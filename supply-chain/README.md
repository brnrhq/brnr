# Dependency audits

brnr keeps five direct dependencies. Every new dependency needs a reason
in its pull request, and every third-party version needs a cargo-vet audit
or an explicit exemption. `vet` is a required check on `main`; CI uses
`cargo vet --locked` so the imported evidence is the reviewed
`imports.lock`, rather than the latest remote audits.

## Remaining work

After two local review batches for [#29](https://github.com/brnrhq/brnr/issues/29),
the locked graph has 102 third-party versions: 41 fully audited and 61
exempted, up from 25 audited and 77 exempted. These counts cover Cargo's
full dependency graph, including optional and platform-specific crates,
not just the dependencies compiled on Linux and macOS.

An exemption means the version is **not audited**. #29 remains open until
the remaining exemptions in `config.toml` have audit coverage. Prioritize
the runtime parsers and their supporting crates (`serde_json`, `toml`,
`toml_parser`, `winnow`, `agent-client-protocol-schema`, `serde_with`,
`memchr`, `itoa`, `zmij`, `hashbrown`), then the remaining build-time and
platform-specific crates. `cargo vet suggest` produces the current list
and the smallest reviewable changes from audited versions.

## Local evidence

`audits.toml` records 14 source-delta reviews against versions covered by
the configured upstream importers, plus full source reviews of the small
`darling_macro` and `jiff-tzdb-platform` wrappers. The records describe what
was checked, including unsafe pointer and FFI boundaries where affected. They identify
automated source reviews explicitly; they do not assert human review or
prove the absence of vulnerabilities. Each dependency needs its own
coverage; auditing a parent crate does not audit its dependencies.

The second batch covers the `powerfmt` buffer delta and those two wrappers.
Their dependencies still need separate coverage: `darling_core`, `syn` and
`jiff-tzdb` remain exempted. `itoa` was considered, but its formatter rewrite
needs a dedicated review of the unsafe indexing and division arithmetic;
its exemption remains.

Only read changes are certified. Larger unread changes remain exemptions;
publisher identity alone is not used to grant coverage. `imports.lock`
retains the upstream evidence needed to connect the local deltas to their
audited baselines.

See [CONTRIBUTING.md](../CONTRIBUTING.md#dependencies) for the review routine.
After adding a delta, run `cargo vet prune` with network access to fetch
its upstream baseline and remove covered exemptions, then verify
`cargo vet --locked`. Pruning with `--locked` cannot fetch a baseline that
the previous minimized import file did not need.
