#!/usr/bin/env bash
# ADR 52: compare independent release builds of HEAD, not a cached binary.
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
export SOURCE_DATE_EPOCH
SOURCE_DATE_EPOCH=$(git show -s --format=%ct HEAD)
export CARGO_INCREMENTAL=0
git archive HEAD > "$work/source.tar"

for build in first second; do
    mkdir "$work/$build"
    tar -xf "$work/source.tar" -C "$work/$build"
    (
        cd "$work/$build"
        # pwd -P also resolves macOS's /var -> /private/var symlink.
        export CARGO_TARGET_DIR="$PWD/target"
        export RUSTFLAGS
        RUSTFLAGS="--remap-path-prefix=$(pwd -P)=/brnr"
        cargo build --release --locked --bin brnr
    )
    # Even a tiny build must start in a different wall-clock second.
    if [ "$build" = first ]; then sleep 2; fi
done

first=$(shasum -a 256 "$work/first/target/release/brnr" | cut -d' ' -f1)
second=$(shasum -a 256 "$work/second/target/release/brnr" | cut -d' ' -f1)
printf 'first:  %s\nsecond: %s\n' "$first" "$second"
if [ "$first" != "$second" ]; then
    echo "release builds differ (same toolchain, different directories and times)" >&2
    exit 1
fi
