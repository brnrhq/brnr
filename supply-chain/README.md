# Dependency audits

brnr keeps five direct dependencies. Every new dependency needs a reason
in its pull request, and every third-party version needs a cargo-vet audit
or an explicit exemption. `vet` is a required check on `main`; CI uses
`cargo vet --locked` so the imported evidence is the reviewed
`imports.lock`, rather than the latest remote audits.

## Remaining work

The locked graph has 102 third-party versions: 25 fully audited (only by
imported audits from Mozilla, Google, the Bytecode Alliance, Embark, ISRG
and Zcash) and 77 exempted. An exemption means the version is **not
audited**. [#29](https://github.com/brnrhq/brnr/issues/29) remains open
until the exemptions in `config.toml` have person audit coverage (ADR 1
N1: an audit is a person's claim to have read the code). These counts
cover Cargo's full dependency graph, including optional and
platform-specific crates, not just the dependencies compiled on Linux and
macOS.

Nineteen earlier entries in `audits.toml` were automated source reviews
certified `safe-to-deploy`. They are now exemptions in `config.toml`, with
their review text kept in `notes` and marked as not a person's audit, so
anyone who imports brnr's audits does not inherit them as audits. Local
`audits.toml` has no certifications.

Prioritize the runtime parsers and their supporting crates (`serde_json`,
`toml`, `toml_parser`, `winnow`, `agent-client-protocol-schema`,
`serde_with`, `memchr`, `hashbrown`), then the remaining build-time and
platform-specific crates. `cargo vet suggest` produces the current list
and the smallest reviewable changes from audited versions.

## Notes kept on exemptions

Some exemptions carry notes from those automated reviews (what was read,
including unsafe pointer and FFI boundaries where mentioned). The notes
do not assert human review or prove the absence of vulnerabilities. Each
dependency needs its own coverage; auditing a parent crate does not audit
its dependencies.

`toml` 1.1.6 was read in full and stays exempted: with default features, a
valid document can abort the process with a stack overflow. `toml_parser`
limits nested arrays and inline tables to 80, and each dotted key to 80
segments, but not the table depth they combine to. This 12,801-byte
document nests about 6,300 tables, and `toml::from_str` overflows a
default 2 MiB thread stack (it parses on a 64 MiB one):

```rust
let key = vec!["a"; 80].join(".");
let text = format!("{}a=0{}", format!("{key}={{").repeat(79), "}".repeat(79));
let value: Result<toml::Value, _> = toml::from_str(&text);
```

`safe-to-deploy` covers untrusted input, not only brnr's own configuration
files, so `toml` needs a version that limits the depth, or a review of an
equivalent fix, before it can be certified.

Only changes a person has read are certified. Larger unread changes remain
exemptions; publisher identity alone is not used to grant coverage.
`imports.lock` retains the upstream evidence for imported audits. It still
holds the imported baselines of the 14 converted deltas (the rest of the
19 were full-version reviews or libc's exemption baseline) until the next
online `cargo vet prune`; `cargo vet` fetches them again when a person
later certifies one of those deltas.

See [CONTRIBUTING.md](../CONTRIBUTING.md#dependencies) for the review routine.
After adding a delta, run `cargo vet prune` with network access to fetch
its upstream baseline and remove covered exemptions, then verify
`cargo vet --locked`. Pruning with `--locked` cannot fetch a baseline that
the previous minimized import file did not need.
