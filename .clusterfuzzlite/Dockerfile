# ClusterFuzzLite's build image for kynex-core's fuzz targets (see build.sh).
# Pinned by digest; Dependabot bumps it.
FROM gcr.io/oss-fuzz-base/base-builder-rust@sha256:3428d0a90b1e7efbe2ba947bd1d0c9a70594986b4b6553804547f1fb24cfc92f
COPY . $SRC/kynex
WORKDIR $SRC/kynex
COPY .clusterfuzzlite/build.sh $SRC/
