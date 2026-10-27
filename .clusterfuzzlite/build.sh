#!/bin/bash -eu
# Builds kynex-core's fuzz targets (crates/kynex-core/fuzz) into $OUT, each
# with its seed corpus from crates/kynex-core/fuzz/seeds/<target>/.

cd "$SRC/kynex/crates/kynex-core"
cargo fuzz build -O
release=fuzz/target/x86_64-unknown-linux-gnu/release
for target in $(cargo fuzz list); do
    cp "$release/$target" "$OUT/"
    if [ -d "fuzz/seeds/$target" ]; then
        (cd "fuzz/seeds/$target" && zip -q -r "$OUT/${target}_seed_corpus.zip" .)
    fi
done
