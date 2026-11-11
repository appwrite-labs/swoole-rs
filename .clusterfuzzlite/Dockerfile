FROM gcr.io/oss-fuzz-base/base-builder-rust:v1
# OpenSSL reaches the fuzz build through swoole-rs-tls, libzstd and libbrotli through
# crates/inflate. Without the -dev packages' unversioned .so links, -lzstd and -lbrotli*
# pick their .a, so the fuzzers run without them in the runner image.
RUN apt-get update && apt-get install -y --no-install-recommends libssl-dev pkg-config \
        libzstd-dev libbrotli-dev \
    && cd /usr/lib/x86_64-linux-gnu \
    && rm libzstd.so libbrotlicommon.so libbrotlidec.so libbrotlienc.so
# The image's own nightly lags the workspace's toolchain and rejects APIs stabilised since
# (#3020); build with the nightly the Miri job pins (.github/workflows/miri.yml) instead.
ENV RUSTUP_TOOLCHAIN=nightly-2026-10-24
RUN rustup toolchain install $RUSTUP_TOOLCHAIN --profile minimal --component rust-src
COPY . $SRC/swoole-rs
WORKDIR $SRC/swoole-rs
COPY .clusterfuzzlite/build.sh $SRC/
