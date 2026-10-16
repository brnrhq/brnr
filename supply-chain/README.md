# Dependency audits

brnr keeps five direct dependencies. Every new dependency needs a reason
in its pull request, and every third-party version needs a cargo-vet audit
or an explicit exemption. `vet` is a required check on `main`; CI uses
`cargo vet --locked` so the imported evidence is the reviewed
`imports.lock`, rather than the latest remote audits.

## Remaining work

After three local review batches for [#29](https://github.com/brnrhq/brnr/issues/29),
the locked graph has 102 third-party versions: 43 fully audited, 1
partially audited (libc, a reviewed 0.2.189 to 0.2.190 delta on an
exemption) and 58 exempted, up from 25 audited and 77 exempted before the
first batch. These counts cover Cargo's
full dependency graph, including optional and platform-specific crates,
not just the dependencies compiled on Linux and macOS.

An exemption means the version is **not audited**. #29 remains open until
the remaining exemptions in `config.toml` have audit coverage. Prioritize
the runtime parsers and their supporting crates (`serde_json`, `toml`,
`toml_parser`, `winnow`, `agent-client-protocol-schema`, `serde_with`,
`memchr`, `hashbrown`), then the remaining build-time and
platform-specific crates. `cargo vet suggest` produces the current list
and the smallest reviewable changes from audited versions.

## Local evidence

`audits.toml` records 14 source-delta reviews against versions covered by
the configured upstream importers, one (libc) against an exemption, and
full source reviews of the small `darling_macro` and `jiff-tzdb-platform`
wrappers and of `itoa` and `zmij`. The records describe what
was checked, including unsafe pointer and FFI boundaries where affected. They identify
automated source reviews explicitly; they do not assert human review or
prove the absence of vulnerabilities. Each dependency needs its own
coverage; auditing a parent crate does not audit its dependencies.

The second batch covers the `powerfmt` buffer delta and those two wrappers.
Their dependencies still need separate coverage: `darling_core`, `syn` and
`jiff-tzdb` remain exempted.

The third batch reviews all of `itoa` 1.0.18 and `zmij` 1.0.23, the integer
and float formatters under `serde_json`, including the dedicated review of
`itoa`'s unsafe indexing and division arithmetic that the second batch left
open.

`toml` 1.1.6 was also read in full, and stays exempted: with default
features, a valid document can abort the process with a stack overflow.
`toml_parser` limits nested arrays and inline tables to 80, and each dotted
key to 80 segments, but not the table depth they combine to. This 12,801-byte
document nests about 6,300 tables, and `toml::from_str` overflows a default
2 MiB thread stack (it parses on a 64 MiB one):

```rust
let key = vec!["a"; 80].join(".");
let text = format!("{}a=0{}", format!("{key}={{").repeat(79), "}".repeat(79));
let value: Result<toml::Value, _> = toml::from_str(&text);
```

`safe-to-deploy` covers untrusted input, not only brnr's own configuration
files, so `toml` needs a version that limits the depth, or a review of an
equivalent fix, before it can be certified.

Only read changes are certified. Larger unread changes remain exemptions;
publisher identity alone is not used to grant coverage. `imports.lock`
retains the upstream evidence needed to connect the local deltas to their
audited baselines.

See [CONTRIBUTING.md](../CONTRIBUTING.md#dependencies) for the review routine.
After adding a delta, run `cargo vet prune` with network access to fetch
its upstream baseline and remove covered exemptions, then verify
`cargo vet --locked`. Pruning with `--locked` cannot fetch a baseline that
the previous minimized import file did not need.
