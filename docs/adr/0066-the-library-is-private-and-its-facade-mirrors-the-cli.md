# 66. The library is private, and its facade mirrors the command line

Accepted 2026-10-14. Implemented, but for the typed facade, which waits for
#150: `brnr::run` is the library's one public item. Resolves #147.
Amends 47: what the published crate's library offers, and that only that is
covered by compatibility.

## Context

Every module of the crate's library was `pub` (`src/lib.rs`), and `cargo
publish` uploads it to crates.io at every release (ADR 47). From 1.0.0
Cargo's semver rules apply to that library: renaming or removing any `pub`
item of any module, a frame constant, a helper of `render.rs`, a type
`schema.rs` re-exports, would take a new major version, though none of it
was meant for anyone but brnr. Holding them still would hold back changes
users never see; letting them move would break the promise P9 makes from
1.0.0 (compatibility within a major version) for whoever used them.

The binary was the library's only user but for the crate's own integration
tests (`tests/`) and fuzz targets (`fuzz/`), which are crates of their own
and used `brnr::json`, `brnr::frame`, `brnr::schema`, `brnr::log` and
`brnr::host::fuzz` from outside. The command dispatch (`ctl.rs`, `ctl/`)
was the binary's, reaching the rest through `brnr::`.

brnr's interface is its command line, and `--json` is its data for programs
(ADR 34). A library that other programs call is useful only if it is that
interface again, not a second one: P5's one interpretation of what an agent
did, reached two ways, with nothing the command line doesn't say.

## Decision

- The library is private. Its modules are `mod`, not `pub mod`, and an item
  is `pub(crate)` unless something outside reaches it (below).
- Its public API is a facade that mirrors the command line, and nothing
  else. From 1.0.0 the facade is covered by the compatibility promise (P9);
  what isn't in it is not.
- `brnr::run(args)` is the facade's coarsest piece, and today its only one:
  `brnr <args>` as a function, `args` being the words after `brnr`, run to
  the exit status the binary would exit with. The dispatch that was
  `main.rs`'s (`acp`, `host`, the rest) is `run`, and the command modules
  (`ctl.rs`, `ctl/`) are the library's; `main.rs` calls `run` with its
  arguments and does nothing else. `run` adds no surface the command line
  doesn't already have: its words and exit statuses are the command line's.
- The typed facade has this shape: one module per command group (`process`,
  `session`, `prompt`, `queue`, `permission`, `config`, `event`, ADR 63),
  and the tools at the top level (`doctor`, `skill`), with one function per
  verb (`brnr::session::list`, `brnr::prompt::send`), taking the command's
  arguments and flags, and returning the data its `--json` prints, with the
  same names and shapes; an error carries the stable code the command's
  error has (#205) once those land, and its message, like the command's, is
  not covered. It isn't built yet: its return types are the `--json`
  shapes, which #150 makes one name and one shape per concept. Built
  before, they would be built twice, or would freeze the shapes #150
  changes. It follows #150, by a record of its own if it departs from this
  shape.
- The crate's own tests and fuzz targets reach internals through
  `brnr::internals`, a `#[doc(hidden)]` module behind the `internals` Cargo
  feature, off by default. It re-exports what they use (`frame`'s reading
  and writing, `json`'s parsing and spans, `schema`'s types, `log::rfc3339`
  and `host::fuzz`'s harness), and nothing more. The integration tests turn
  it on with a dev-dependency of the crate on itself, so `cargo test` needs
  no flag; `fuzz/Cargo.toml` turns it on for the fuzz targets.
  `host::fuzz` is compiled only for unit tests and with the feature.
- `internals` is not covered by any compatibility promise, before or after
  1.0.0: its items change, move or go whenever the tests do. The crate's
  documentation, `Cargo.toml` and this record say so. With the feature on,
  `unreachable_pub` warns of a `pub` item that nothing outside reaches, so
  `pub` keeps meaning reachable through `run` or `internals`.

## Consequences

- `cargo doc` without the feature lists `run` alone. Anyone who used a
  module of the library (`brnr::render`, `brnr::schema`) loses it in this
  release, without a shim (P9); the command line, with `--json`, is the
  interface.
- A Cargo feature can't be hidden from crates.io: `internals` is in the
  published manifest and anyone can turn it on. What it reaches is no
  promise, and is said to be none.
- Cargo turns a feature on for the whole of one build: under `cargo test`
  the binary the tests drive is built with `internals` too, and so has the
  fuzz harness and its `StartChannel` variant, which nothing in the binary
  calls. `cargo build` and `cargo install`, what users run, build without.
- Until the typed facade is built, a program that wants brnr's data runs
  the binary, or `brnr::run`, with `--json`.

## Considered

- Keeping the library public, and treating every change to a `pub` item as
  a semver change from 1.0.0: the library would hold still for users it was
  never written for, and every refactor would be a major version or a
  break.
- `#[doc(hidden)]` on the public modules, with the README and ADR 47 saying
  the library isn't covered: the items would still be reachable by path,
  and a promise kept only in prose is one a user can miss and a tool like
  `cargo semver-checks` doesn't see. Private modules are the compiler's
  promise.
- Making everything private with the tests and fuzzers under a `cfg`
  instead of a feature: `cfg(test)` is set only for the crate's own unit
  tests, not for `tests/` or `fuzz/`, which are separate crates; a custom
  `--cfg` would have to be passed to every `cargo test`, by hand and in CI.
  A feature can be turned on in the manifests that need it.
- Moving the integration tests and fuzz targets into the crate as unit
  tests: the integration tests drive the real binary against
  `tests/fake_agent.py`, and cargo-fuzz builds its targets as a crate of its
  own.
- A second, unpublished crate holding the internals, which `brnr` and the
  tests both depend on: a published crate can't depend on an unpublished
  one, so it would be published too, with the same promise problem, and a
  workspace for it.
- Building the typed facade now: its return types would be today's
  `--json` shapes, which #150 changes.

## Tests

Run `cargo test --release adr_0066_`. Named claims and their assertions:

- [tests/cli.rs](../../tests/cli.rs)
  - `adr_0066_run_is_the_command_line`: `brnr::run`, called in the test's
    process, ends with the exit status the binary exits with for the same
    words.

That nothing else is public is the compiler's: a private module can't be
named from outside the crate, and `cargo doc` without the feature lists
`run` alone. The fuzz targets building against `internals` is checked by the
fuzz workflow's build (`.clusterfuzzlite/build.sh`).
