# ClusterFuzzLite's build image for kynex-core's fuzz targets (see build.sh).
# Pinned by digest; Dependabot bumps it.
FROM gcr.io/oss-fuzz-base/base-builder-rust@sha256:d345218878c78feed410f0ad57268617a368912b224f844bbdb71c54c0b9955a
COPY . $SRC/kynex
WORKDIR $SRC/kynex
COPY .clusterfuzzlite/build.sh $SRC/
