# 40. `release.sh`

Accepted (former decision 31); reviewed 2026-10-07. Implemented.
Amended by 47: `release.sh tag` also checks crates.io.
Amended by 52: the workflow checks reproducibility, attaches an SBOM and tests Homebrew after publication.
Amended 2026-10-09 (#105): a release's notes are its section of
CHANGELOG.md, kept as Keep a Changelog 1.1.0 has it, where they were the
titles of the pull requests it merged.
Amended 2026-10-15 (#156): release candidates, `X.Y.Z-rc.N`, are GitHub
pre-releases that leave the Homebrew formulae alone, and a release's notes
take in its release candidates'.

## Decision

Two steps, because main only changes through reviewed pull requests:
`release.sh <bump>` lints and tests main and opens the release pull request,
with the changes and the adapters' versions; `release.sh tag`, after the
merge, checks that CI passed on main, tags, follows the release workflow and
checks the tap. `release.sh notes` prints what the release pull request
would say. Generating release notes requires Git 2.13 or newer, checked
before looking up the last tag: older Git cannot exclude release candidates
from a final release's comparison, and must fail rather than list all commits.

A release's notes are its section of `CHANGELOG.md`, which follows
[Keep a Changelog 1.1.0](https://keepachangelog.com/en/1.1.0/): what someone
upgrading notices, under its six change types (Added, Changed, Deprecated,
Removed, Fixed, Security, a fixed advisory by its id), each entry with the
ADR it follows (P9: release notes say what changed). Before 1.0 a breaking
change goes under Changed or Removed, starting with **Breaking:**.

- A pull request that changes what users see adds its entry under
  `## [Unreleased]`.
- `release.sh <bump>` refuses an empty Unreleased unless a final release
  folds in its release candidates' entries. On the release branch it
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

### Release candidates

Amended 2026-10-15 (#156), before 1.0.0-rc.1. A release candidate lets
users try a version before Homebrew users get it with `brew upgrade`.

- A version is `X.Y.Z` or `X.Y.Z-rc.N` (N from 1), as SemVer 2.0.0 writes
  them, and comes after the current one in SemVer's order: `X.Y.Z` after
  its release candidates. `release.sh` refuses anything else (`1.2.3.4`,
  `1.0.0rc1`, an older version), rather than release it.
- `release.sh 1.0.0-rc.1` names the first. From a release candidate, `rc`
  is the next one, and `1.0.0` the release; `major`, `minor` and `patch`
  fail there, since they could mean the release or a version past it (P4).
  `release.sh next <bump>` prints the version a bump makes.
- A tag with a `-` makes a GitHub pre-release (`gh release create
  --prerelease`), so the latest release stays the last `X.Y.Z`. The release
  workflow publishes it to crates.io, where `cargo install brnr` only takes
  it with `--version`, but doesn't update the formulae in the tap, and skips
  the Homebrew smoke test, which installs from there. `release.sh tag`
  checks the GitHub release is a pre-release, doesn't check the tap, and
  says how to install it.
- A release candidate's CHANGELOG.md section and pull request cover what
  changed since the last tag, a release candidate or not: they are its
  GitHub release's notes. The release's cover what changed since the last
  release: `release.sh X.Y.Z` folds the sections of `X.Y.Z-rc.N` into
  `X.Y.Z`'s, under each change type the **Breaking:** entries first, newest
  first, removes their links, and compares `X.Y.Z` with the tag the first
  release candidate compared from. Its pull request lists the commits and
  adapter changes since the last release. With nothing under Unreleased, the
  release candidates' entries are its notes; it refuses an empty section.

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
- Release candidates in the tap, as a `brnr@rc` formula or a `--HEAD`
  build: a second formula to keep, for a few weeks before each major
  version. crates.io and the release's tarball reach those who want one.
- Keeping each release candidate's section under the release's: the
  release's notes would say only what changed since the last release
  candidate, and someone upgrading from the last release reads them all.
- `major`, `minor` and `patch` from a release candidate as npm's `semver
  inc` does (`1.0.0-rc.1` `major` is `1.0.0`): it picks for the maintainer
  whether the bump releases the candidate or passes it.

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
  - `adr_0040_a_version_comes_after_cargo_tomls`: 0.8.0 to 1.0.0-rc.1 to
    rc.2 to 1.0.0 through `release.sh next`, and what it refuses.
  - `adr_0040_a_release_takes_in_its_release_candidates`.
  - `adr_0040_a_releases_commits_start_from_the_last_release`, through
    `release.sh notes` in a scratch repository with tags, including refusal
    of Git older than 2.13 before it can produce notes from the wrong tag.

Tagging, publishing and updating the tap mutate external repositories and
registries, so the suite can't run them; release.yml and release.sh's
checks after publication are the evidence. There is no sandboxed end-to-end
release.sh test. That a `-rc.N` tag makes a pre-release and leaves the tap
alone rests on release.yml's `if:`s and `release.sh tag`'s check that the
GitHub release is a pre-release.
