#!/usr/bin/env bash
# ADR 11.8: build the documentation, docs/, with mdBook into target/book.
# mdBook is the release's x86_64 Linux binary, checked against its pinned
# SHA-256 before it runs; a new version is a new pin here.
set -euo pipefail

version=0.5.4
sha256=3f28de05dafca9d0f2eab99c662116b0e37b89b1d96a08f8f430b9eeae958cd7
tarball="mdbook-v$version-x86_64-unknown-linux-gnu.tar.gz"
bin="${RUNNER_TEMP:-$(mktemp -d)}/mdbook-$version"

cd "$(git rev-parse --show-toplevel)"
mkdir -p "$bin"
curl -fsSL --retry 3 -o "$bin/$tarball" \
  "https://github.com/rust-lang/mdBook/releases/download/v$version/$tarball"
echo "$sha256  $bin/$tarball" | sha256sum -c -
tar -xzf "$bin/$tarball" -C "$bin"
"$bin/mdbook" build
