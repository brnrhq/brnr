#!/bin/sh
# Builds the bundled adapters as single-file executables (bun --compile) into
# <out>, default ../target/release, next to brnr. They keep the command names
# their npm packages install: claude-agent-acp and codex-acp.
#
# Optional dependencies are omitted, so the native agent binaries the
# adapters' packages would pull in (Claude Code via the Claude Agent SDK, the
# Codex CLI via @openai/codex) are not bundled; the adapters run the user's.
set -eu
cd "$(dirname "$0")"
out=${1:-../target/release}

bun install --frozen-lockfile --omit=optional
mkdir -p "$out"
bun build --compile --minify claude.ts --outfile "$out/claude-agent-acp"
bun build --compile --minify codex.ts --outfile "$out/codex-acp"

# Third-party licenses of everything compiled in.
mkdir -p "$out/licenses"
for license in node_modules/*/LICENSE* node_modules/@*/*/LICENSE*; do
    [ -f "$license" ] || continue
    package=$(dirname "${license#node_modules/}")
    cp "$license" "$out/licenses/$(echo "$package" | tr / _).txt"
done
