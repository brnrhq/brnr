#!/usr/bin/env bash
# Releases brnr in two steps, because main only changes through reviewed pull
# requests:
#
#   ./release.sh <version | major | minor | patch>
#       From an up-to-date main: lints and tests, bumps the version on a
#       release-<version> branch and opens its pull request, which lists what
#       changed and the adapters' npm versions.
#
#   ./release.sh tag
#       Once that is merged: checks CI passed on main, tags v<version>, follows
#       the release workflow (GitHub release, Homebrew formulae) and checks the
#       tap has the new version.
#
#   ./release.sh notes [<version | major | minor | patch>]
#       Prints what the release pull request would say, changing nothing.
#
# Needs git, cargo and gh (logged in, with push access).
set -euo pipefail
cd "$(dirname "$0")"

repo=brnrhq/brnr
tap=brnrhq/homebrew-tap

die() { echo "release.sh: $*" >&2; exit 1; }
say() { echo "==> $*"; }

current_version() { sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1; }

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
    local current version last branch
    current=$(current_version)
    version=$(next_version "$current" "$1")
    branch=release-$version
    git rev-parse -q --verify "refs/tags/v$version" >/dev/null && die "v$version is tagged already"
    last=$(git describe --tags --abbrev=0 --match 'v*' 2>/dev/null || true)

    say "checking main"
    cargo clippy --release --all-targets -- -D warnings
    cargo test --release

    say "bumping $current -> $version on $branch"
    git checkout -q -b "$branch"
    sed -i.bak "s/^version = \"$current\"/version = \"$version\"/" Cargo.toml && rm Cargo.toml.bak
    cargo update -q --workspace
    [ "$(current_version)" = "$version" ] || die "couldn't bump Cargo.toml"
    git commit -q -am "Release $version"
    git push -q -u origin "$branch"

    say "opening the pull request"
    gh pr create -R "$repo" --base main --head "$branch" --title "Release $version" \
        --body "$(release_notes "$version" "$last")"
    echo
    echo "Once it is merged: ./release.sh tag"
}

# What the release pull request says: the changes since <last>, and the
# adapters, flagging an adapter build that changed without a new npm version
# (Homebrew wouldn't rebuild it; bump the formula's revision in $tap).
release_notes() {
    local version=$1 last=$2 range=HEAD
    [ -n "$last" ] && range="$last..HEAD"
    echo "Bumps the version to $version. Once this is merged, \`./release.sh tag\` tags \`v$version\`, which runs the release workflow."
    echo
    echo "## Changes since ${last:-the start}"
    git log --no-merges --format='- %s' "$range" | grep -v "^- Release " || echo "- (none)"
    echo
    echo "## Adapters (Homebrew formula versions)"
    local name package now before
    for name in claude codex; do
        case $name in
            claude) package=@agentclientprotocol/claude-agent-acp ;;
            codex) package=@agentclientprotocol/codex-acp ;;
        esac
        now=$(adapter_version "$package")
        before=$([ -n "$last" ] && adapter_version "$package" "$last" || echo "-")
        if [ "$now" != "$before" ]; then
            echo "- brnr-$name-adapter: $before -> $now (\`brew upgrade\` rebuilds it)"
        else
            echo "- brnr-$name-adapter: $now, unchanged"
        fi
    done
    if [ -n "$last" ] && ! git diff --quiet "$last" HEAD -- adapters ':!adapters/package.json' ':!adapters/bun.lock'; then
        echo
        echo "**adapters/ changed without a new npm version**, so Homebrew won't rebuild the adapters: bump \`revision\` in the formulae in $tap if users need the new build."
    fi
}

tag() {
    on_clean_main
    local version run
    version=$(current_version)
    git rev-parse -q --verify "refs/tags/v$version" >/dev/null && die "v$version is tagged already; bump the version first"

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

    say "checking the tap"
    gh api "repos/$tap/contents/Formula/brnr.rb" --jq .content | base64 --decode | grep -q "v$version.tar.gz" ||
        die "$tap's brnr formula doesn't point at v$version"
    gh release view "v$version" -R "$repo" --json url --jq .url
    echo "brnr $version is out: brew upgrade brnr"
}

case ${1:-} in
    tag) tag ;;
    notes)
        last=$(git describe --tags --abbrev=0 --match 'v*' 2>/dev/null || true)
        release_notes "$(next_version "$(current_version)" "${2:-patch}")" "$last"
        ;;
    "" | -h | --help) sed -n '2,19p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) prepare "$1" ;;
esac
