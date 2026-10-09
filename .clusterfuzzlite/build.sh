#!/bin/bash -eu
# Builds every fuzz target into $OUT, as ClusterFuzzLite runs them, each with
# its seeds (fuzz/seeds/<target>) as its seed corpus.
cd "$SRC/brnr"
cargo fuzz build -O
for target in $(cargo fuzz list); do
  cp "fuzz/target/x86_64-unknown-linux-gnu/release/$target" "$OUT/"
  (cd "fuzz/seeds/$target" && zip -q "$OUT/${target}_seed_corpus.zip" ./*)
done
