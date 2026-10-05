#!/bin/sh
# Builds the bundled adapters as single-file executables (bun --compile) into
# <out>, default ../target/release, next to brnr: brnr-claude-adapter
# (claude-agent-acp) and brnr-codex-adapter (codex-acp), or only those named
# after <out> (claude, codex). Their own names, so they don't clash with the
# npm packages' commands when both are installed.
#
#   build.sh [<out> [claude] [codex]]
#
# Optional dependencies are omitted, so the native agent binaries the
# adapters' packages would pull in (Claude Code via the Claude Agent SDK, the
# Codex CLI via @openai/codex) are not bundled; the adapters run the user's.
set -eu
cd "$(dirname "$0")"
out=${1:-../target/release}
[ $# -gt 0 ] && shift
[ $# -gt 0 ] || set -- claude codex
for adapter; do
    case $adapter in
        claude | codex) ;;
        *) echo "build.sh: no adapter $adapter (claude, codex)" >&2; exit 2 ;;
    esac
done

bun install --frozen-lockfile --omit=optional
mkdir -p "$out"
for adapter; do
    bun build --compile --minify "$adapter.ts" --outfile "$out/brnr-$adapter-adapter"
done

# Third-party licenses of everything compiled in.
mkdir -p "$out/licenses"
for license in node_modules/*/LICENSE* node_modules/@*/*/LICENSE*; do
    [ -f "$license" ] || continue
    package=$(dirname "${license#node_modules/}")
    cp "$license" "$out/licenses/$(echo "$package" | tr / _).txt"
done
