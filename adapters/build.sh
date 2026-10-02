#!/bin/sh
# Builds the bundled adapters, brnr-claude and brnr-codex, as single-file
# executables (bun --compile) into <out>, default ../target/release, next to
# brnr.
#
# Optional dependencies are omitted, so the native agent binaries the
# adapters' packages would pull in (Claude Code via the Claude Agent SDK, the
# Codex CLI via @openai/codex) are not bundled; the adapters run the user's.
set -eu
cd "$(dirname "$0")"
out=${1:-../target/release}

bun install --frozen-lockfile --omit=optional
mkdir -p "$out"
for adapter in claude codex; do
    bun build --compile --minify "$adapter.ts" --outfile "$out/brnr-$adapter"
done

# Third-party licenses of everything compiled in.
mkdir -p "$out/licenses"
for license in node_modules/*/LICENSE* node_modules/@*/*/LICENSE*; do
    [ -f "$license" ] || continue
    package=$(dirname "${license#node_modules/}")
    cp "$license" "$out/licenses/$(echo "$package" | tr / _).txt"
done
