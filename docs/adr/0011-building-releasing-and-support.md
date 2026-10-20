# 11. Building, releasing and support

Draft 2026-10-19. Implemented. Factor: N1 (ADR 1). Brings together former
ADRs 37, 38, 39, 40, 45, 47 and 52 (docs/adr/README.md maps them).

How brnr and its adapters are built, found, versioned, released and
published, and how a user reports a bug. The supply chain serves N1: pinned
builds compiled where they run, reproducible release builds with an SBOM,
publication without a stored token, and bug reports that send nothing the
user didn't choose to paste.

## 11.1 Adapters through Homebrew

- `brnr-claude-adapter` and `brnr-codex-adapter`, formulae in brnr's tap,
  compile the adapters on the user's machine from brnr's `adapters/` at the
  same tag, pinned by `bun.lock`, with `bun build --compile`. bun, in
  homebrew-core, is a build-only dependency. Nothing prebuilt is
  distributed: the user's machine fetches and compiles the packages, as
  `adapters/build.sh` does for anyone who builds from source. The npm
  packages are mentioned as the alternative.
- One formula per adapter. You install what you use (each is about 60 MB
  and its own build). Each has the license that fits: only the Claude
  adapter compiles in the SDK, `license :cannot_represent` with a caveat;
  the Codex adapter is Apache-2.0. One adapter's upstream breaking doesn't
  stop the other from installing. A third agent's adapter is one more
  formula.
- `adapters/` packages the adapters unchanged. brnr adds only finding the
  user's own agent, and `--version` (`adapters/claude.ts`, `codex.ts`).

Not covered by a Rust test: Homebrew formulae live in the separate tap
repository; this checkout cannot prove their installation or license
declarations. The adapters CI job builds and exercises both pinned
adapters; the release workflow tests the public brnr formula. Adapter
formula installation still needs a tap-side check.

**Why.** brnr runs the community's ACP adapters for Claude Code and Codex.
They are Node packages. claude-agent-acp compiles in Anthropic's Claude
Agent SDK, under Anthropic's Commercial Terms, so brnr's tap can't ship
prebuilt adapters.

**Considered.**

- Telling Homebrew users to install the npm packages: that needs Node 22
  on the editor's PATH, and the packages likely bring their own copies of
  the agents through optional dependencies (`build.sh` leaves those out).
- One formula for both (`brnr-adapters`): it loses each reason for one
  formula per adapter, above.
- A tap per adapter: more `brew tap`s for users and more release plumbing,
  for nothing a formula per adapter doesn't give.
- deno as the compiler (also in homebrew-core): `build.sh` uses `bun build
  --compile` with a frozen lockfile, its output is what brnr's tests run
  against, and both adapters are written for Node. deno's npm compatibility
  would be one more thing to verify, for code that spawns and talks to
  subprocesses.

## 11.2 The adapters' names, and finding them next to brnr

- The compiled adapters are `brnr-claude-adapter` and
  `brnr-codex-adapter`. `brnr acp -- claude-agent-acp` runs the npm
  package. `brnr doctor` reports both kinds.
- A bare agent name (no `/`) is looked for next to brnr first, then on
  PATH, so an editor's PATH needn't include Homebrew's. "Next to brnr" is
  the running binary's directory, and the directory of the path brnr was
  started by (`argv[0]`, when it is a path). Homebrew links `bin/brnr` and
  the adapters into its prefix's `bin`. On macOS the running binary's path
  is the link's, so an adapter next to the link is found. On Linux it is
  the link's target in the Cellar, where the adapters aren't.
- `brnr session new`, `brnr session resume` and `brnr acp`, started by that
  path, do the lookup, since whoever starts a process resolves everything
  (ADR 3.3). They pass the path found to the process, which starts the
  agent. The rule is fixed and documented (P4).
- A started bridge's bare command (`~` expanded) is looked for the same
  way, in the same place. So `command = "brnr"`, `args = ["event",
  "notify", …]` works from an editor whose PATH lacks brnr, and a bridge linked into
  Homebrew's `bin` is found on Linux too (ADR 5.7, ADR 5.8).

**Why.** An editor's PATH often lacks Homebrew's `bin`, and on Linux the
running binary's own directory is in the Cellar, away from the adapters.

**Considered.**

- The npm packages' command names (`claude-agent-acp`, `codex-acp`), so
  docs and configs read the same either way: with a Homebrew install next
  to a global npm install, two commands of one name would be on PATH, and
  which one ran would depend on PATH order.
- Looking only next to the running binary: on Linux, through Homebrew's
  symlink, the adapters aren't found.

## 11.3 Which adapter version is built

- Each adapter formula's version is its npm package's (`brnr-claude-adapter
  0.85.1`), written by the release workflow from `adapters/package.json`. A
  brnr release that doesn't move the pin changes the formula's source but
  not its version, so nobody rebuilds for nothing. `brew outdated` shows an
  adapter when its package moves.
- When `adapters/` changes without a new npm version, `release.sh` says so:
  bump the formula's `revision` by hand if users need the new build. That
  is any change in `adapters/` but the pins' files (`package.json`,
  `bun.lock`), and theirs too when no pin moved: `bun.lock` alone moving is
  a dependency of a pinned package moving, which changes the build as much.
  The release workflow drops the revision when the npm version next moves,
  since a new version starts the count over.
- The adapters say what they were built from (`--version`, compiled in),
  and `brnr doctor` shows it. For npm-installed adapters doctor shows the
  version from the `package.json` their bin link leads to. `brnr session
  status` shows what the agent says it is in `initialize`: ACP's
  `agentInfo`, as its `agent`'s `name` and `version`, beside `program`
  (ADR 8.2).
- Dependabot opens a pull request for new releases of the two packages,
  weekly; the next brnr release ships them. A nightly workflow
  (`canary.yml`) builds the newest releases first, without moving the pins,
  checks them with brnr and opens an issue when they fail, so a breaking
  release is known before Dependabot proposes it. Every pull request, a
  Dependabot one included, builds the pinned adapters and checks them with
  brnr the same way (`adapters/check.py --brnr`).

**Consequences.** The pin matters. The adapters behave differently from
each other, and from one version to the next, wherever ACP leaves room
(ADR 4.2). brnr's tests run against exactly the pinned builds.

**Considered.**

- A brnr release for every upstream release: batching them into brnr's
  releases keeps the pace a person's.

## 11.4 `release.sh`

Two steps, because main only changes through reviewed pull requests:

- `release.sh <bump>` lints and tests main and opens the release pull
  request, with the changes and the adapters' versions.
- `release.sh tag`, after the merge, checks that CI passed on main, tags,
  follows the release workflow, and checks the tap and crates.io
  (ADR 11.6).
- `release.sh notes` prints what the release pull request would say.
  Generating release notes requires Git 2.13 or newer, checked before
  looking up the last tag: older Git cannot exclude release candidates from
  a final release's comparison, and must fail rather than list all commits.

### Release notes

A release's notes are its section of `CHANGELOG.md`, which follows
[Keep a Changelog 1.1.0](https://keepachangelog.com/en/1.1.0/): what
someone upgrading notices, under its six change types (Added, Changed,
Deprecated, Removed, Fixed, Security, a fixed advisory by its id), each
entry with the ADR it follows (P9: release notes say what changed). Before
1.0 a breaking change goes under Changed or Removed, starting with
**Breaking:**.

- A pull request that changes what users see adds its entry under
  `## [Unreleased]`.
- `release.sh <bump>` refuses an empty Unreleased unless a final release
  folds in its release candidates' entries. On the release branch it heads
  what it has `## [X.Y.Z] - YYYY-MM-DD`, under a new, empty Unreleased. It
  moves the link definitions to match: `[X.Y.Z]` compares the last tag with
  `vX.Y.Z`, and `[Unreleased]` `vX.Y.Z` with HEAD. The release pull request
  shows the section, to be edited there. `release.sh changelog --named
  X.Y.Z` prints the file as it will be.
- `release.sh tag` refuses a version `CHANGELOG.md` has no section for. The
  release workflow publishes the section with `gh release create
  --notes-file`, and fails before the build if there is none.
- `release.sh changelog [<version>]` prints a version's section, the one
  reading of the file both use: `## [1.1]` isn't `## [1.1.0]`, the link
  definitions are left out, and each paragraph and list item is on one
  line, since GitHub shows line breaks as they are.

### Release candidates

A release candidate lets users try a version before Homebrew users get it
with `brew upgrade`.

- A version is `X.Y.Z` or `X.Y.Z-rc.N` (N from 1), as SemVer 2.0.0 writes
  them, and comes after the current one in SemVer's order: `X.Y.Z` after
  its release candidates. `release.sh` refuses anything else (`1.2.3.4`,
  `1.0.0rc1`, an older version), rather than release it.
- `release.sh 1.0.0-rc.1` names the first. From a release candidate, `rc`
  is the next one, and `1.0.0` the release. `major`, `minor` and `patch`
  fail there, since they could mean the release or a version past it (P4).
  `release.sh next <bump>` prints the version a bump makes.
- A tag with a `-` makes a GitHub pre-release (`gh release create
  --prerelease`), so the latest release stays the last `X.Y.Z`. The release
  workflow publishes it to crates.io, where `cargo install brnr` only takes
  it with `--version`. It doesn't update the formulae in the tap, and skips
  the Homebrew smoke test, which installs from there (ADR 11.7).
  `release.sh tag` checks that the GitHub release is a pre-release, doesn't
  check the tap, and says how to install it.
- A release candidate's `CHANGELOG.md` section and pull request cover what
  changed since the last tag, a release candidate or not: they are its
  GitHub release's notes. The release's cover what changed since the last
  release: `release.sh X.Y.Z` folds the sections of `X.Y.Z-rc.N` into
  `X.Y.Z`'s, under each change type the **Breaking:** entries first, newest
  first, removes their links, and compares `X.Y.Z` with the tag the first
  release candidate compared from. Its pull request lists the commits and
  adapter changes since the last release. With nothing under Unreleased,
  the release candidates' entries are its notes; it refuses an empty
  section.

Not covered by a Rust test: tagging, publishing and updating the tap
change external repositories and registries, so the suite can't run them;
release.yml and release.sh's checks after publication are the evidence.
There is no sandboxed end-to-end release.sh test. That a `-rc.N` tag makes
a pre-release and leaves the tap alone rests on release.yml's `if:`s and
`release.sh tag`'s check that the GitHub release is a pre-release.

**Why.** main changes only through reviewed pull requests, so a release is
a pull request and then a tag. Release notes are for whoever upgrades, and
are best written with the change, by whoever made it (P9).

**Considered.**

- One command that pushes the bump to main and tags: it skips review.
- Doing it all in a workflow (`workflow_dispatch`): it works, but is harder
  to run and debug than a script you can read.
- `gh release create --generate-notes`: the pull requests' titles, which
  say what changed to whoever reviewed it, not what to do to whoever
  upgrades.
- Release notes written in the release pull request: once, by whoever
  releases, long after the change.
- Release candidates in the tap, as a `brnr@rc` formula or a `--HEAD`
  build: a second formula to keep, for a few weeks before each major
  version. crates.io and the release's tarball reach those who want one.
- Keeping each release candidate's section under the release's: the
  release's notes would say only what changed since the last release
  candidate, and someone upgrading from the last release reads them all.
- `major`, `minor` and `patch` from a release candidate as npm's `semver
  inc` does (`1.0.0-rc.1` `major` is `1.0.0`): it picks for the maintainer
  whether the bump releases the candidate or passes it.

## 11.5 Bug reports

- `brnr doctor --report` runs the checks and prints, instead of them, a bug
  report to read, then paste into an issue. It is Markdown, and starts with
  a comment that says to read it first. Then:
  - brnr's version and the OS (`macOS 26.4 (Darwin 25.4.0, arm64)`: the
    product's name and version from SystemVersion.plist or os-release, and
    `uname`'s);
  - the adapters, as doctor's lines have them (where each is, and the
    version, ADR 11.3);
  - the checks that aren't ok (`fail`, `warn` and `info`);
  - the last 20 lines of the latest host log, and of the latest log of a
    process that panicked or died without recording it (ADR 3.7), when
    that is another one, each with how it ended (`exited`, `brnr panicked`,
    `running`, `died without recording it`). A line longer than 4000
    characters is cut, saying how much more there was. Each line that is a
    record, read as transcripts are (ADR 2.5: nested up to 256), has what
    ADR 9.1 redacts redacted again, for a log an older brnr wrote; it is
    otherwise as recorded. A line nested deeper is not shown, and says
    so.
  - Everywhere, the home directory is `~`, as a path and as the start of a
    project folder's name.
- `doctor --report --json` is one object with the same data (P5, ADR 8.2):
  `brnr`, `os`, `adapters`, `checks` (doctor's `{level, check, message}`),
  and `host_logs` (`run`, `ended`, `lines`, the lines as the text has
  them). `--report` exits as doctor does. `--fix` doesn't go with it.
- A panic prints, after Rust's message, a link to a pre-filled bug form
  (`issues/new?template=bug.yml`), URL-encoded, with the title (`Panic at
  <file:line:col>`), `brnr --version`, the OS and the panic's message (at
  most 1000 characters of it). It never has the command's arguments, only
  which command it was. Nothing is sent; the user opens the link or
  doesn't. It goes where the user of the panicking process looks:
  - the CLI (every command but `host`), and `brnr acp`: on stderr, which is
    the editor's log for `acp`;
  - a host in the foreground (ADR 3.5): on its stderr, from its hook;
  - a detached host has no terminal (its stderr is its host log): when its
    start fails with the panic, `brnr session new`, `brnr session resume`
    and `brnr acp` print the link after the reason. A detached host's panic
    later on is the `process_exited` error that watchers and `event wait` see, and
    the host log `doctor --report` shows. It has no link, which would be
    noise in every event a panic ends.
- The bug form asks for `brnr doctor --report`, not `doctor --json`.

**Why.** A bug report needs the brnr version, the OS, the adapters, what
doctor finds and what the host log says. Gathered by hand, that is
`--version`, `doctor --json` (which names paths and sessions) and a host
log, trimmed. A panic said only what Rust says, and a detached host's panic
is recorded (ADR 3.7) where nobody reads it as it happens. brnr listens on
no network (P13) and never phones home (P15): what reaches an issue is what
the user chose to paste (#23, #24).

**Consequences.**

- The report shows the host log's lines as they are, prompts included (the
  `process_started` record has the `start_request`, ADR 3.3): only what brnr knows is
  secret is redacted. The comment at its top, the README and the form say
  to read it first.
- A panic that keeps the hook from running (an abort, ADR 3.7) prints no
  link.

**Considered.**

- Sending reports (telemetry, a crash reporter): brnr reaches nothing
  beyond the machine (P13, P15), and the user decides what to share.
- The session transcripts in the report: they hold prompts and code; `brnr
  event log` is there for what shows the bug.
- Every host log of a process that died: the latest is what a report is
  about, and doctor's check already counts the rest.
- The command line in the link: arguments carry prompts and paths.
- Writing the link into the host log too: the log is the record, and the
  panic is in it.

## 11.6 brnr on crates.io

- brnr is published to crates.io as `brnr`, at every release and release
  candidate, by the release workflow. A `crates` job runs after the GitHub
  release and the tap, once the reproducibility check and, for a release,
  the Homebrew smoke test (ADR 11.7) have passed. A release candidate
  skips the smoke test (ADR 11.4).
- It uses crates.io's trusted publishing, with no stored token. The job
  exchanges GitHub's OIDC token for a short-lived crates.io token
  (`rust-lang/crates-io-auth-action`, which revokes it when the job ends).
  The crates.io side trusts only `release.yml` in `brnrhq/brnr`, in the
  `crates-io` environment.
- Before anything is made public, the release job runs `cargo publish
  --dry-run`. A version that won't package stops before the GitHub release,
  and a version already published can't be replaced.
- The package `include`s only what the build needs: `src/`, `skills/`
  (which `brnr skill` embeds), the README, and the license and NOTICE,
  which Apache-2.0 asks a redistribution to carry. The tests, docs, site
  and workflows stay in the repository.
- The published library is private but for `brnr::run`; only its facade is
  covered by compatibility (ADR 8.5).
- `cargo install brnr --locked` is documented next to Homebrew. It installs
  brnr only. The adapters come from Homebrew or npm (ADR 11.1), because a
  crate can't build them.
- `release.sh tag` checks that crates.io has the new version, as it checks
  the tap (ADR 11.4).

Not covered by a Rust test: publishing requires the crates.io
trusted-publisher environment and creates an immutable external version.
release.yml runs `cargo publish --dry-run` before publication and uses the
`crates` job for the real publish; release.sh checks the published version.
The runtime suite cannot prove registry permissions or publication.

**Why.** `cargo install brnr` is what a Rust user tries first, and anyone
could publish a crate by that name. A tool that runs coding agents with the
user's credentials makes a good lure for someone who impersonates it (N1).
With Homebrew as the only channel, Linux users without Homebrew had to
build from a clone.

**Consequences.**

- A crates.io version can be yanked but not deleted. A bad release is fixed
  by the next one, as with the tap.
- The first version (0.6.0) was published by hand, with a token scoped to
  `brnr` and `publish-new`, then revoked: trusted publishing can only be
  set up for a crate that exists. After that, no token is needed.

**Considered.**

- An empty placeholder crate to hold the name: crates.io's policy
  discourages it, and the real crate costs no more.
- A `CARGO_REGISTRY_TOKEN` secret: a long-lived credential that can
  publish, kept in the repository's secrets. Trusted publishing needs none.
- Publishing from `release.sh` on the maintainer's machine: the same
  long-lived token, kept locally, and a publish that doesn't come from the
  tested, tagged build.
- Prebuilt binaries through `cargo binstall`: binstall can already fall
  back to building from source. Release binaries are a separate decision.

## 11.7 Reproducible builds and an SBOM

- On pull requests, main, and before a tag is published, Linux and macOS
  each build the committed source twice with `cargo build --release
  --locked`. Each build has its own source and target directories, without
  a target cache, and runs at a different wall-clock time. Both use the
  commit timestamp as `SOURCE_DATE_EPOCH` and remap the source directory to
  `/brnr` with `--remap-path-prefix`. Their SHA-256 hashes are printed and
  must match. The tag calls the same workflow that checks pull requests.
- Before publication, pinned `cargo-cyclonedx` generates a CycloneDX 1.5
  JSON SBOM, including transitive Rust dependencies across all targets. It
  must leave `Cargo.lock` unchanged. The release attaches it as
  `brnr-X.cdx.json`, with build provenance, next to the source archive.
- After the release and the tap update, a separate, fresh macOS runner runs
  `brew install brnrhq/tap/brnr` and `brew test brnrhq/tap/brnr`, checks
  `brnr --version` against the tag, and runs `brnr doctor`. It uses the
  public formula and source download, with no checkout or local build to
  fall back to. A release candidate leaves the tap alone, so it has no
  smoke test (ADR 11.4).

Not covered by a Rust test: these claims are exercised by workflow jobs
rather than Rust tests. reproducible.yml independently rebuilds and
compares SHA-256 hashes on Linux and macOS; release.yml generates the SBOM
and runs the fresh Homebrew smoke job. The runtime suite cannot prove a
public installation or a release attachment.

**Why.** Build provenance says where the source archive came from. It does
not say that rebuilding it gives the same binary, list its dependencies in
a machine-readable form, or prove that the published Homebrew formula
works (#37). A failed check must fail the workflow (P3, P7), before
publication wherever possible.

**Consequences.**

- Reproducibility is checked within each runner's toolchain and OS. This
  does not promise identical binaries across Rust versions, platforms or
  linkers; stable Rust and runner images still move. Releases ship source,
  not prebuilt binaries.
- The SBOM describes brnr's Rust crate. It does not inventory the adapters'
  npm dependencies or claim that an inventoried dependency is safe.
- The smoke test fails the release workflow if installation, the formula's
  test, the version check or doctor fails. The GitHub release and the tap
  push have already happened and this check cannot roll them back.
  Publishing to crates.io, where a version can't be replaced, waits for it
  (ADR 11.6): a failed smoke test leaves the version off crates.io. Missing
  optional adapters are informational to doctor and need no credentials.

**Considered.**

- Reusing one target directory: Cargo may reuse the first binary, which
  proves nothing about an independent rebuild.
- Checking only on main: a tag can point at a commit without a successful
  main run. Publication must depend on the check itself.
- Running the smoke test from the release checkout: it can accidentally
  test a local binary, and bypass what the user downloads and installs.

## Tests

Run `cargo test --release adr_0011_`. Named claims, by section:

- 11.1: none; see the section.
- 11.2: [tests/headless.rs](../../tests/headless.rs):
  `adr_0011_adapters_next_to_a_symlinked_brnr`,
  `adr_0011_bridges_next_to_a_symlinked_brnr`.
- 11.3: [tests/doctor.rs](../../tests/doctor.rs):
  `adr_0011_adapter_versions`.
- 11.4: [tests/release.rs](../../tests/release.rs):
  - `adr_0011_a_releases_notes_are_its_changelog_section`.
  - `adr_0011_a_version_without_a_section_has_no_notes`.
  - `adr_0011_a_release_names_unreleased_and_moves_its_links`, through
    `changelog --named`, which `<bump>` writes.
  - `adr_0011_the_version_being_built_has_its_notes`: so a release pull
    request without them fails CI, before `release.sh tag`; also that every
    section has only Keep a Changelog's change types, in its order.
  - `adr_0011_a_version_comes_after_cargo_tomls`: 0.8.0 to 1.0.0-rc.1 to
    rc.2 to 1.0.0 through `release.sh next`, and what it refuses.
  - `adr_0011_a_release_takes_in_its_release_candidates`.
  - `adr_0011_a_releases_commits_start_from_the_last_release`, through
    `release.sh notes` in a scratch repository with tags, including the
    refusal of Git older than 2.13 before it can produce notes from the
    wrong tag.
- 11.5: [tests/headless.rs](../../tests/headless.rs):
  `adr_0011_a_panic_prints_a_link_to_report_it`;
  [tests/doctor.rs](../../tests/doctor.rs):
  `adr_0011_the_report_is_what_to_paste`,
  `adr_0011_the_report_redacts_a_deep_line`,
  `adr_0011_the_report_shows_a_death_without_a_record`.
- 11.6, 11.7: none; see the sections.
