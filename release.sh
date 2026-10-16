#!/usr/bin/env bash
# Releases brnr in two steps, because main only changes through reviewed pull
# requests:
#
#   ./release.sh <version | major | minor | patch | rc>
#       From an up-to-date main: lints and tests, bumps the version and names
#       CHANGELOG.md's Unreleased section for it on a release-<version>
#       branch, and opens its pull request, which has the release notes, the
#       commits and the adapters' npm versions.
#
#   ./release.sh tag
#       Once that is merged: checks CHANGELOG.md has the version and CI passed
#       on main, tags v<version>, follows the release workflow (GitHub
#       release, source tarball, Homebrew formulae, crates.io), verifies the
#       tarball's attestation, checks the tap points at the tarball (but for
#       a release candidate) and crates.io has the version.
#
#   ./release.sh notes [<version | major | minor | patch | rc>]
#       Prints what the release pull request would say, changing nothing.
#
#   ./release.sh next <version | major | minor | patch | rc>
#       Prints the version that would be released, changing nothing.
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
# A version is X.Y.Z or, for a release candidate of X.Y.Z, X.Y.Z-rc.N, and
# must come after Cargo.toml's. From X.Y.Z-rc.N, rc is X.Y.Z-rc.<N+1>, and
# X.Y.Z releases it; major, minor and patch bump a release only. A release
# candidate is a GitHub pre-release and is on crates.io, but the Homebrew
# formulae stay at the last release. X.Y.Z's section of CHANGELOG.md takes in
# its release candidates', and its notes and commits start from the last
# release, so they say everything since.
#
# Needs git, cargo and gh (logged in, with push access). Generating release
# notes needs Git 2.13 or newer, for git describe --exclude.
set -euo pipefail
cd "$(dirname "$0")"

repo=brnrhq/brnr
tap=brnrhq/homebrew-tap

die() { echo "release.sh: $*" >&2; exit 1; }
say() { echo "==> $*"; }

current_version() { sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1; }

# The section of CHANGELOG.md (or <file>) headed `## [<name>]` (a version,
# or Unreleased), up to the next or the link definitions, without its heading
# or the blank lines around it. Each paragraph and list item is put on one
# line, because GitHub shows a release's and a pull request's line breaks as
# they are.
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
    ' "${2:-CHANGELOG.md}"
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
#
# A release takes in the sections of its release candidates, <version>-rc.N,
# and their links: its section has, under each change type, the **Breaking:**
# entries and then the others, newest first, and compares the tag its first
# release candidate compared from.
named_changelog() {
    awk -v version="$1" -v date="$2" -v url="https://github.com/$repo/compare/" '
        function rc(name) { return version !~ /-/ && index(name, version "-rc.") == 1 && substr(name, length(version) + 5) ~ /^[0-9]+$/ }
        # The name in a `## [name]` heading or a `[name]: ` link.
        function named(s) { s = substr(s, index(s, "[") + 1); return substr(s, 1, index(s, "]") - 1) }
        function close_entry() {
            if (entry == "") return
            k = ++count[kind]; text[kind, k] = entry; breaking[kind, k] = (index(entry, "- **Breaking:**") == 1)
            entry = ""; blanks = ""
        }
        function body(    i, j, b, n, prev, item) {
            n = 0
            for (i = 0; i <= kinds; i++) {
                if (!count[order[i]]) continue
                if (n++) print ""
                if (order[i] != "") { print "### " order[i]; print "" }
                prev = ""
                for (b = 1; b >= 0; b--)
                    for (j = 1; j <= count[order[i]]; j++) {
                        if (breaking[order[i], j] != b) continue
                        item = (substr(text[order[i], j], 1, 2) == "- ")
                        if (prev != "" && !(item && prev == "item")) print ""
                        print text[order[i], j]
                        prev = item ? "item" : "text"
                    }
            }
        }
        BEGIN { split("Added Changed Deprecated Removed Fixed Security", order, " "); order[0] = ""; kinds = 6; for (i = 0; i <= kinds; i++) known[order[i]] = 1 }

        # First reading: what Unreleased and the release candidates have,
        # each entry or paragraph by its change type.
        FNR == NR {
            if (/^## /) { close_entry(); n = named($0); from_section = (n == "Unreleased" || rc(n)); folds += rc(n); kind = ""; fence = 0; next }
            if (/^\[[^ ]+\]: /) {
                close_entry(); from_section = 0
                # The first release candidate compares a tag before them.
                if (rc(named($0)) && index($0, "]: " url) > 0) {
                    tag = substr($0, index($0, "]: " url) + length("]: " url)); tag = substr(tag, 1, index(tag, "...") - 1)
                    if (index(tag, "v" version "-rc.") != 1) first = tag
                }
                next
            }
            if (!from_section) next
            if (!fence && /^### /) { close_entry(); kind = substr($0, 5); if (!(kind in known)) { order[++kinds] = kind; known[kind] = 1 }; next }
            if (/^```/) fence = !fence
            if (!NF && !fence) { if (entry != "") blanks = blanks "\n"; next }
            if (entry != "" && (fence || /^ / || blanks == "") && !/^- /) { entry = entry "\n" blanks $0; blanks = ""; next }
            close_entry(); entry = $0
            next
        }

        !head && $0 == "## [Unreleased]" {
            print; print ""; print "## [" version "] - " date; head = 1
            if (folds) { print ""; body(); skip = 1 }
            next
        }
        skip && /^## / && rc(named($0)) { next }
        skip && (/^## / || /^\[[^ ]+\]: /) { skip = 0; print "" }
        skip { next }
        !link && index($0, "[Unreleased]: " url) == 1 && /\.\.\.HEAD$/ {
            from = substr($0, length("[Unreleased]: " url) + 1)
            from = substr(from, 1, length(from) - length("...HEAD"))
            if (folds && first != "") from = first
            print "[Unreleased]: " url "v" version "...HEAD"
            print "[" version "]: " url from "...v" version
            link = 1
            next
        }
        folds && /^\[[^ ]+\]: / && rc(named($0)) { next }
        { print }
        END { exit !(head && link) }
    ' CHANGELOG.md CHANGELOG.md
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

# X.Y.Z, or X.Y.Z-rc.N, without leading zeros (SemVer 2.0.0's form).
version_re='^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-rc\.([1-9][0-9]*))?$'

# <version> as a key that sorts as SemVer orders versions: by major, minor
# and patch, X.Y.Z after its release candidates, rc.10 after rc.9.
version_key() {
    [[ $1 =~ $version_re ]] || return 1
    printf '%012d.%012d.%012d.%012d\n' "${BASH_REMATCH[1]}" "${BASH_REMATCH[2]}" "${BASH_REMATCH[3]}" "${BASH_REMATCH[5]:-999999999999}"
}

next_version() {
    local current=$1 wanted=$2 major minor patch rc next
    [[ $current =~ $version_re ]] || die "Cargo.toml's version $current isn't X.Y.Z or X.Y.Z-rc.N"
    major=${BASH_REMATCH[1]} minor=${BASH_REMATCH[2]} patch=${BASH_REMATCH[3]} rc=${BASH_REMATCH[5]}
    case $wanted in
        major | minor | patch)
            [ -z "$rc" ] || die "$current is a release candidate: rc for $major.$minor.$patch-rc.$((rc + 1)), or $major.$minor.$patch to release it"
            case $wanted in
                major) next=$((major + 1)).0.0 ;;
                minor) next=$major.$((minor + 1)).0 ;;
                patch) next=$major.$minor.$((patch + 1)) ;;
            esac
            ;;
        rc)
            [ -n "$rc" ] || die "$current isn't a release candidate: name the first, X.Y.Z-rc.1"
            next=$major.$minor.$patch-rc.$((rc + 1))
            ;;
        *)
            [[ $wanted =~ $version_re ]] || die "a version (1.2.3, or 1.2.3-rc.1 for a release candidate) or major, minor, patch, rc; not $wanted"
            next=$wanted
            ;;
    esac
    [[ $(version_key "$next") > $(version_key "$current") ]] || die "$next doesn't come after $current"
    echo "$next"
}

prerelease() { [[ $1 == *-* ]]; }

# The tag <version>'s notes and commits start from: the last one's, but for
# a release, the last release's, so they cover its release candidates.
last_tag() {
    local git_version
    git_version=$(git --version) || die "couldn't read Git's version"
    [[ $git_version =~ ^git\ version\ ([0-9]+)\.([0-9]+)\. ]] || die "unrecognized Git version: $git_version"
    (( BASH_REMATCH[1] > 2 || (BASH_REMATCH[1] == 2 && BASH_REMATCH[2] >= 13) )) ||
        die "generating release notes needs Git 2.13 or newer; found $git_version"
    if prerelease "$1"; then
        git describe --tags --abbrev=0 --match 'v*' 2>/dev/null || true
    else
        git describe --tags --abbrev=0 --match 'v*' --exclude 'v*-*' 2>/dev/null || true
    fi
}

prepare() {
    on_clean_main
    local current version last branch today
    current=$(current_version)
    version=$(next_version "$current" "$1")
    branch=release-$version
    git rev-parse -q --verify "refs/tags/v$version" >/dev/null && die "v$version is tagged already"
    today=$(date -u +%Y-%m-%d)
    named_changelog "$version" "$today" >/dev/null || unnamed
    [ -n "$(named_changelog "$version" "$today" | changelog "$version" -)" ] ||
        die "CHANGELOG.md has nothing under Unreleased: say there what $version changes"
    last=$(last_tag "$version")

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
        --body "$(release_notes "$version" "$last" CHANGELOG.md)"
    echo
    echo "Once it is merged: ./release.sh tag"
}

# What the release pull request says: <version>'s section of <changelog>
# (CHANGELOG.md, or as it will be once named), the commits since <last>, and
# for a release the adapters, flagging an adapter build that changed without
# a new npm version (Homebrew wouldn't rebuild it; bump the formula's
# revision in $tap): anything in adapters/ but the pins, or with no pin
# moved, the pins' files too (bun.lock alone: a dependency of a pinned
# package moved).
release_notes() {
    local version=$1 last=$2 file=$3 range=HEAD
    [ -n "$last" ] && range="$last..HEAD"
    echo "Bumps the version to $version. Once this is merged, \`./release.sh tag\` tags \`v$version\`, which runs the release workflow."
    echo
    echo "## Release notes"
    echo
    echo "CHANGELOG.md's section for $version, which the GitHub release says; edit it here."
    echo
    changelog "$version" "$file"
    echo
    echo "## Commits since ${last:-the start}"
    git log --no-merges --format='- %s' "$range" | grep -v "^- Release " || echo "- (none)"
    echo
    if prerelease "$version"; then
        echo "## Adapters"
        echo "A release candidate is a GitHub pre-release and leaves the Homebrew formulae alone: the release after it updates them."
        return
    fi
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

    local pre
    pre=$(gh release view "v$version" -R "$repo" --json isPrerelease --jq .isPrerelease)
    if prerelease "$version"; then
        [ "$pre" = true ] || die "v$version's GitHub release isn't a pre-release"
        say "not checking the tap: a release candidate leaves it alone"
    else
        [ "$pre" = false ] || die "v$version's GitHub release is a pre-release"
        say "checking the tap"
        gh api "repos/$tap/contents/Formula/brnr.rb" --jq .content | base64 --decode |
            grep -qF "https://github.com/$repo/releases/download/v$version/$tarball" ||
            die "$tap's brnr formula doesn't point at v$version's $tarball"
    fi

    say "checking crates.io"
    curl -fsS -A "brnr release.sh (https://github.com/$repo)" "https://crates.io/api/v1/crates/brnr/$version" >/dev/null ||
        die "crates.io doesn't have brnr $version"
    gh release view "v$version" -R "$repo" --json url --jq .url
    if prerelease "$version"; then
        echo "brnr $version, a release candidate, is out: cargo install brnr --version $version --locked"
    else
        echo "brnr $version is out: brew upgrade brnr, or cargo install brnr --locked"
    fi
}

case ${1:-} in
    tag) tag ;;
    notes)
        current=$(current_version)
        bump="patch"
        prerelease "$current" && bump=rc
        version=$(next_version "$current" "${2:-$bump}")
        named_changelog "$version" "$(date -u +%Y-%m-%d)" >/dev/null || unnamed
        last=$(last_tag "$version")
        release_notes "$version" "$last" <(named_changelog "$version" "$(date -u +%Y-%m-%d)")
        ;;
    next)
        [ -n "${2:-}" ] || die "next <version | major | minor | patch | rc>"
        next_version "$(current_version)" "$2"
        ;;
    changelog)
        if [ "${2:-}" = --named ]; then
            [ -n "${3:-}" ] || die "changelog --named <version> [<date>]"
            named_changelog "$3" "${4:-$(date -u +%Y-%m-%d)}" || unnamed
        else
            release_section "${2:-$(current_version)}"
        fi
        ;;
    "" | -h | --help) sed -n '2,/^set -euo/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) prepare "$1" ;;
esac
