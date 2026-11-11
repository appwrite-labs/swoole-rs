#!/bin/bash -eu
# Every cargo-fuzz target in fuzz/ (see fuzz/README.md), with its seeds and dictionary.
cd "$SRC/swoole-rs/fuzz"
# OSS-Fuzz compiles C++ against libc++; link the oracle the same way.
export CXXSTDLIB=c++
# libcrypto.a, so the fuzzers run without OpenSSL in the runner image.
export OPENSSL_STATIC=1
# libz-sys's bundled zlib, built with the sanitizer CFLAGS so ASan sees what the
# inflate targets make zlib do.
export LIBZ_SYS_STATIC=1
# pkg-config --static, so libbrotlidec.a and libbrotlienc.a bring libbrotlicommon.a.
export PKG_CONFIG_ALL_STATIC=1
cargo fuzz build -O --debug-assertions
for target in $(cargo fuzz list); do
    cp "target/x86_64-unknown-linux-gnu/release/$target" "$OUT/"
    if [ -d "seeds/$target" ]; then
        zip -jq "$OUT/${target}_seed_corpus.zip" "seeds/$target"/*
    fi
    if [ -f "${target%%_*}.dict" ]; then
        cp "${target%%_*}.dict" "$OUT/$target.dict"
    fi
done
