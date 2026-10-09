# 40. `release.sh`

Accepted (former decision 31); reviewed 2026-10-07. Implemented.
Amended by 47: `release.sh tag` also checks crates.io.
Amended by 52: the workflow checks reproducibility, attaches an SBOM and tests Homebrew after publication.
Amended 2026-10-09 (#105): a release's notes are its section of
CHANGELOG.md, kept as Keep a Changelog 1.1.0 has it, where they were the
titles of the pull requests it merged.

## Decision

Two steps, because main only changes through reviewed pull requests:
`release.sh <bump>` lints and tests main and opens the release pull request,
with the changes and the adapters' versions; `release.sh tag`, after the
merge, checks that CI passed on main, tags, follows the release workflow and
checks the tap. `release.sh notes` prints what the release pull request
would say.

A release's notes are its section of `CHANGELOG.md`, which follows
[Keep a Changelog 1.1.0](https://keepachangelog.com/en/1.1.0/): what someone
upgrading notices, under its six change types (Added, Changed, Deprecated,
Removed, Fixed, Security, a fixed advisory by its id), each entry with the
ADR it follows (P9: release notes say what changed). Before 1.0 a breaking
change goes under Changed or Removed, starting with **Breaking:**.

- A pull request that changes what users see adds its entry under
  `## [Unreleased]`.
- `release.sh <bump>` refuses an empty Unreleased, and on the release branch
  heads what it has `## [X.Y.Z] - YYYY-MM-DD`, under a new, empty
  Unreleased. It moves the link definitions to match: `[X.Y.Z]` compares the
  last tag with `vX.Y.Z`, and `[Unreleased]` `vX.Y.Z` with HEAD. The release
  pull request shows the section, to be edited there; `release.sh changelog
  --named X.Y.Z` prints the file as it will be.
- `release.sh tag` refuses a version CHANGELOG.md has no section for. The
  release workflow publishes the section with `gh release create
  --notes-file`, and fails before the build if there is none.
- `release.sh changelog [<version>]` prints a version's section, the one
  reading of the file both use: `## [1.1]` isn't `## [1.1.0]`, the link
  definitions are left out, and each paragraph and list item is on a line,
  since GitHub shows line breaks as they are.

## Considered

- One command that pushes the bump to main and tags: it skips review.
- Doing it all in a workflow (`workflow_dispatch`): it works, but is harder
  to run and debug than a script you can read.
- `gh release create --generate-notes`: the pull requests' titles, which
  say what changed to whoever reviewed it, not what to do to whoever
  upgrades.
- Release notes written in the release pull request: once, by whoever
  releases, long after the change. An Unreleased entry is written with the
  change, by whoever made it.

## Tests

Run `cargo test --release adr_0040_`. Named claims and their assertions:

- [tests/release.rs](../../tests/release.rs)
  - `adr_0040_a_releases_notes_are_its_changelog_section`.
  - `adr_0040_a_version_without_a_section_has_no_notes`.
  - `adr_0040_a_release_names_unreleased_and_moves_its_links`, through
    `changelog --named`, which `<bump>` writes.
  - `adr_0040_the_version_being_built_has_its_notes`: so a release pull
    request without them fails CI, before `release.sh tag`; also that every
    section has only Keep a Changelog's change types, in its order.

Tagging, publishing and updating the tap mutate external repositories and
registries, so the suite can't run them; release.yml and release.sh's
checks after publication are the evidence. There is no sandboxed end-to-end
release.sh test.
