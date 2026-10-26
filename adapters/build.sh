#!/bin/sh
# Builds the adapters as single-file executables (bun --compile or Cargo) into
# <out>, default ../target/release, next to brnr: brnr-claude-adapter
# (claude-agent-acp) and brnr-codex-adapter (codex-acp), or only those named
# after <out> (claude, codex, muse). Muse is built with Cargo from its source
# tag; its npm package supplies only the version. Their own names don't clash
# with the npm packages' commands when both are installed.
#
#   build.sh [<out> [claude] [codex] [muse]]
#
# Optional dependencies are omitted, so the native agent binaries the
# adapters' packages would pull in (Claude Code via the Claude Agent SDK, the
# Codex CLI via @openai/codex) are not bundled; the adapters run the user's.
set -eu
cd "$(dirname "$0")"
out=${1:-../target/release}
[ $# -gt 0 ] && shift
[ $# -gt 0 ] || set -- claude codex muse
node=
for adapter; do
    case $adapter in
        claude | codex) node=1 ;;
        muse) ;;
        *) echo "build.sh: no adapter $adapter (claude, codex, muse)" >&2; exit 2 ;;
    esac
done

[ -z "$node" ] || bun install --frozen-lockfile --production --omit=optional
mkdir -p "$out"
# Only this build's licenses, followed by each selected adapter's own.
rm -rf "$out/licenses"
mkdir -p "$out/licenses"
for adapter; do
    if [ "$adapter" = muse ]; then
        python3 muse.py "$out"
    else
        bun build --compile --minify "$adapter.ts" --outfile "$out/brnr-$adapter-adapter"
    fi
done

# Third-party licenses of everything compiled in: each package's own
# (LICENSE, license, License.txt, LICENCE, ...), nested packages too, as
# <package>.txt; a second file of a package's as <package>.<file>.txt.
[ -n "$node" ] || exit 0
find node_modules -type f \( -iname 'licen[cs]e' -o -iname 'licen[cs]e[.-]*' \) | sort |
while read -r license; do
    package=${license%/*}
    parent=${package%/*}
    case ${parent##*/} in
        node_modules) ;;
        @*) grandparent=${parent%/*}; [ "${grandparent##*/}" = node_modules ] || continue ;;
        *) continue ;; # Somewhere inside a package, not its own.
    esac
    name=$(echo "${package#node_modules/}" | tr / _)
    dest="$out/licenses/$name.txt"
    [ -e "$dest" ] && dest="$out/licenses/$name.${license##*/}.txt"
    cp "$license" "$dest"
done
