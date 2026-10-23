# Releasing

Generating release notes requires Git 2.13 or newer, so a final release's
comparison can exclude release candidates (ADR 11.4).

```sh
./release.sh minor     # or patch, major, 1.2.3: checks main, opens the release pull request
./release.sh tag       # once it's merged: tags, releases, updates the Homebrew tap
./release.sh notes     # what the release pull request would say
./release.sh 1.0.0-rc.1  # a release candidate; then rc for 1.0.0-rc.2, and 1.0.0 to release it
```

A release's notes are its section of [CHANGELOG.md](changelog.md), in
[Keep a Changelog 1.1.0](https://keepachangelog.com/en/1.1.0/) form, which
pull requests add to under `[Unreleased]`. `./release.sh minor` names that
section for the version on the release branch and moves its compare links,
`./release.sh tag` won't tag a version without one, and the release workflow
publishes it as the GitHub release's notes, or fails (ADR 11.4).

A release candidate, `X.Y.Z-rc.N`, is a GitHub pre-release and is on
crates.io (`cargo install brnr --version X.Y.Z-rc.N --locked`), but the
Homebrew formulae stay at the last release. `X.Y.Z`'s section of
CHANGELOG.md takes in its release candidates' sections, and its pull request
lists the commits since the last release, so its notes say everything since.
A version must come after the current one (ADR 11.4).

Dependabot opens a pull request when an adapter's npm package has a new
release; the next brnr release ships it.
