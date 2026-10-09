#!/usr/bin/env bash
# Releases brnr in two steps, because main only changes through reviewed pull
# requests:
#
#   ./release.sh <version | major | minor | patch>
#       From an up-to-date main: lints and tests, bumps the version and names
#       CHANGELOG.md's Unreleased section for it on a release-<version>
#       branch, and opens its pull request, which has the release notes, the
#       commits and the adapters' npm versions.
#
#   ./release.sh tag
#       Once that is merged: checks CHANGELOG.md has the version and CI passed
#       on main, tags v<version>, follows the release workflow (GitHub
#       release, source tarball, Homebrew formulae, crates.io), verifies the
#       tarball's attestation, checks the tap points at the tarball and
#       crates.io has the version.
#
#   ./release.sh notes [<version | major | minor | patch>]
#       Prints what the release pull request would say, changing nothing.
#
#   ./release.sh changelog [<version>]
#       Prints the version's section of CHANGELOG.md (Cargo.toml's version if
#       none is given), the GitHub release's notes; fails if it has none.
#
#   ./release.sh changelog --named <version> [<date>]
#       Prints CHANGELOG.md as <bump> writes it for <version>, changing
#       nothing.
#
# CHANGELOG.md is Keep a Changelog 1.1.0 (https://keepachangelog.com/en/1.1.0/).
#
# Needs git, cargo and gh (logged in, with push access).
set -euo pipefail
cd "$(dirname "$0")"

repo=brnrhq/brnr
tap=brnrhq/homebrew-tap

die() { echo "release.sh: $*" >&2; exit 1; }
say() { echo "==> $*"; }

current_version() { sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1; }

# The section of CHANGELOG.md headed `## [<name>]` (a version, or
# Unreleased), up to the next or the link definitions, without its heading or
# the blank lines around it. Each paragraph and list item is put on one line,
# because GitHub shows a release's and a pull request's line breaks as they
# are.
changelog() {
    awk -v name="$1" '
        function out(s) { printf "%s%s\n", gap, s; gap = ""; text = 1 }
        function flush() { if (line != "") out(line); line = "" }
        /^## / { if (on) exit; on = ($0 == "## [" name "]" || index($0, "## [" name "] - ") == 1); next }
        /^\[[^ ]+\]: / { if (on) exit; next }
        !on { next }
        /^```/ { flush(); fence = !fence; out($0); next }
        fence { out($0); next }
        !NF { flush(); if (text) gap = gap "\n"; next }
        line == "" || /^ *(#+|[-*>|]|[0-9]+\.) / { flush(); line = $0; next }
        { sub(/^ +/, ""); line = line " " $0 }
        END { flush() }
    ' CHANGELOG.md
}

# <version>'s release notes: its section of CHANGELOG.md, which it must have.
release_section() {
    local text
    text=$(changelog "$1")
    [ -n "$text" ] || die "CHANGELOG.md has no section for $1 (## [$1] - YYYY-MM-DD); './release.sh <bump>' makes it from Unreleased"
    echo "$text"
}

# CHANGELOG.md with what Unreleased has headed `## [<version>] - <date>`,
# under a new, empty Unreleased, and the links moved: <version> compares the
# tag Unreleased compared from with v<version>, and Unreleased v<version> with
# HEAD. Fails without the Unreleased heading or link.
named_changelog() {
    awk -v version="$1" -v date="$2" -v url="https://github.com/$repo/compare/" '
        !head && $0 == "## [Unreleased]" { print; print ""; print "## [" version "] - " date; head = 1; next }
        !link && index($0, "[Unreleased]: " url) == 1 && /\.\.\.HEAD$/ {
            from = substr($0, length("[Unreleased]: " url) + 1)
            print "[Unreleased]: " url "v" version "...HEAD"
            print "[" version "]: " url substr(from, 1, length(from) - length("...HEAD")) "...v" version
            link = 1
            next
        }
        { print }
        END { exit !(head && link) }
    ' CHANGELOG.md
}

unnamed() { die "CHANGELOG.md needs '## [Unreleased]' and '[Unreleased]: https://github.com/$repo/compare/<tag>...HEAD'"; }

# The npm version adapters/package.json pins for <package>, at <rev> (or in
# the working tree).
adapter_version() {
    local package=$1 rev=${2:-}
    if [ -n "$rev" ]; then git show "$rev:adapters/package.json"; else cat adapters/package.json; fi |
        python3 -c 'import json, sys; print(json.load(sys.stdin)["dependencies"].get(sys.argv[1], "-"))' "$package"
}

on_clean_main() {
    [ "$(git branch --show-current)" = main ] || die "not on main"
    [ -z "$(git status --porcelain)" ] || die "the working tree has changes"
    git fetch -q origin
    [ "$(git rev-parse HEAD)" = "$(git rev-parse origin/main)" ] || die "main isn't origin/main (git pull)"
}

next_version() {
    local current=$1 wanted=$2 major minor patch
    IFS=. read -r major minor patch <<<"$current"
    case $wanted in
        major) echo "$((major + 1)).0.0" ;;
        minor) echo "$major.$((minor + 1)).0" ;;
        patch) echo "$major.$minor.$((patch + 1))" ;;
        [0-9]*.[0-9]*.[0-9]*) echo "$wanted" ;;
        *) die "a version (1.2.3) or major, minor, patch; not $wanted" ;;
    esac
}

prepare() {
    on_clean_main
    local current version last branch today
    current=$(current_version)
    version=$(next_version "$current" "$1")
    branch=release-$version
    git rev-parse -q --verify "refs/tags/v$version" >/dev/null && die "v$version is tagged already"
    [ -n "$(changelog Unreleased)" ] || die "CHANGELOG.md has nothing under Unreleased: say there what $version changes"
    today=$(date -u +%Y-%m-%d)
    named_changelog "$version" "$today" >/dev/null || unnamed
    last=$(git describe --tags --abbrev=0 --match 'v*' 2>/dev/null || true)

    say "checking main"
    cargo clippy --release --all-targets -- -D warnings
    cargo test --release

    say "bumping $current -> $version on $branch"
    git checkout -q -b "$branch"
    sed -i.bak "s/^version = \"$current\"/version = \"$version\"/" Cargo.toml && rm Cargo.toml.bak
    cargo update -q --workspace
    [ "$(current_version)" = "$version" ] || die "couldn't bump Cargo.toml"
    named_changelog "$version" "$today" >CHANGELOG.md.new || unnamed
    mv CHANGELOG.md.new CHANGELOG.md
    release_section "$version" >/dev/null
    git commit -q -am "Release $version"
    git push -q -u origin "$branch"

    say "opening the pull request"
    gh pr create -R "$repo" --base main --head "$branch" --title "Release $version" \
        --body "$(release_notes "$version" "$last" "$version")"
    echo
    echo "Once it is merged: ./release.sh tag"
}

# What the release pull request says: CHANGELOG.md's <section> (the
# version's, or Unreleased before it is named), the commits since <last>, and
# the adapters, flagging an adapter build that changed without a new npm version
# (Homebrew wouldn't rebuild it; bump the formula's revision in $tap):
# anything in adapters/ but the pins, or with no pin moved, the pins' files
# too (bun.lock alone: a dependency of a pinned package moved).
release_notes() {
    local version=$1 last=$2 section=$3 range=HEAD
    [ -n "$last" ] && range="$last..HEAD"
    echo "Bumps the version to $version. Once this is merged, \`./release.sh tag\` tags \`v$version\`, which runs the release workflow."
    echo
    echo "## Release notes"
    echo
    echo "CHANGELOG.md's section for $version, which the GitHub release says; edit it here."
    echo
    changelog "$section"
    echo
    echo "## Commits since ${last:-the start}"
    git log --no-merges --format='- %s' "$range" | grep -v "^- Release " || echo "- (none)"
    echo
    echo "## Adapters (Homebrew formula versions)"
    local name package now before moved=
    for name in claude codex; do
        case $name in
            claude) package=@agentclientprotocol/claude-agent-acp ;;
            codex) package=@agentclientprotocol/codex-acp ;;
        esac
        now=$(adapter_version "$package")
        before=$([ -n "$last" ] && adapter_version "$package" "$last" || echo "-")
        if [ "$now" != "$before" ]; then
            moved=1
            echo "- brnr-$name-adapter: $before -> $now (\`brew upgrade\` rebuilds it)"
        else
            echo "- brnr-$name-adapter: $now, unchanged"
        fi
    done
    local built=(adapters ':!adapters/package.json' ':!adapters/bun.lock')
    [ -n "$moved" ] || built=(adapters)
    if [ -n "$last" ] && ! git diff --quiet "$last" HEAD -- "${built[@]}"; then
        echo
        echo "**adapters/ changed without a new npm version**, so Homebrew won't rebuild the adapters: bump \`revision\` in the formulae in $tap if users need the new build."
    fi
}

tag() {
    on_clean_main
    local version run
    version=$(current_version)
    git rev-parse -q --verify "refs/tags/v$version" >/dev/null && die "v$version is tagged already; bump the version first"
    release_section "$version" >/dev/null

    say "checking CI on main ($(git rev-parse --short HEAD))"
    local conclusion
    conclusion=$(gh run list -R "$repo" --workflow ci --commit "$(git rev-parse HEAD)" --json conclusion --jq '.[0].conclusion // "none"')
    [ "$conclusion" = success ] || die "CI on main is '$conclusion', not success"

    say "tagging v$version"
    git tag -a "v$version" -m "brnr $version"
    git push -q origin "v$version"

    say "following the release workflow"
    sleep 10
    run=$(gh run list -R "$repo" --workflow release --branch "v$version" --limit 1 --json databaseId --jq '.[0].databaseId')
    [ -n "$run" ] || die "no release run for v$version yet; see https://github.com/$repo/actions"
    gh run watch "$run" -R "$repo" --exit-status

    local tarball=brnr-$version.tar.gz dir
    say "verifying $tarball's attestation"
    dir=$(mktemp -d)
    gh release download "v$version" -R "$repo" -p "$tarball" -D "$dir"
    gh attestation verify "$dir/$tarball" -R "$repo" >/dev/null || die "$tarball's attestation doesn't verify"
    rm -r "$dir"

    say "checking the tap"
    gh api "repos/$tap/contents/Formula/brnr.rb" --jq .content | base64 --decode |
        grep -qF "https://github.com/$repo/releases/download/v$version/$tarball" ||
        die "$tap's brnr formula doesn't point at v$version's $tarball"

    say "checking crates.io"
    curl -fsS -A "brnr release.sh (https://github.com/$repo)" "https://crates.io/api/v1/crates/brnr/$version" >/dev/null ||
        die "crates.io doesn't have brnr $version"
    gh release view "v$version" -R "$repo" --json url --jq .url
    echo "brnr $version is out: brew upgrade brnr, or cargo install brnr --locked"
}

case ${1:-} in
    tag) tag ;;
    notes)
        last=$(git describe --tags --abbrev=0 --match 'v*' 2>/dev/null || true)
        release_notes "$(next_version "$(current_version)" "${2:-patch}")" "$last" Unreleased
        ;;
    changelog)
        if [ "${2:-}" = --named ]; then
            [ -n "${3:-}" ] || die "changelog --named <version> [<date>]"
            named_changelog "$3" "${4:-$(date -u +%Y-%m-%d)}" || unnamed
        else
            release_section "${2:-$(current_version)}"
        fi
        ;;
    "" | -h | --help) sed -n '2,31p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) prepare "$1" ;;
esac
