# The image ClusterFuzzLite builds the fuzz targets (fuzz/) in: OSS-Fuzz's
# Rust builder, which has nightly and cargo-fuzz, pinned by digest so a new
# image arrives in a pull request that moves the pin (Dependabot's).
FROM gcr.io/oss-fuzz-base/base-builder-rust:v1@sha256:73be59e48262b7243f815f1c981a6e753e95e4b13e4d085cc9fc06bd79d42daa
COPY . $SRC/brnr
WORKDIR $SRC/brnr
COPY .clusterfuzzlite/build.sh $SRC/
