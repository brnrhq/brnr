# The image ClusterFuzzLite builds the fuzz targets (fuzz/) in: OSS-Fuzz's
# Rust builder, which has nightly and cargo-fuzz, pinned by digest so a new
# image arrives in a pull request that moves the pin (Dependabot's).
FROM gcr.io/oss-fuzz-base/base-builder-rust:v1@sha256:3428d0a90b1e7efbe2ba947bd1d0c9a70594986b4b6553804547f1fb24cfc92f
COPY . $SRC/brnr
WORKDIR $SRC/brnr
COPY .clusterfuzzlite/build.sh $SRC/
