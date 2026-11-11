FROM gcr.io/oss-fuzz-base/base-builder-rust:v1
# crates/websocket's SHA-1 comes from OpenSSL (openssl-sys); build.sh links it statically.
RUN apt-get update && apt-get install -y --no-install-recommends libssl-dev pkg-config
# The image's own nightly lags the workspace's toolchain and rejects APIs stabilised since
# (#3020); build with the nightly the Miri job pins (.github/workflows/miri.yml) instead.
ENV RUSTUP_TOOLCHAIN=nightly-2026-10-24
RUN rustup toolchain install $RUSTUP_TOOLCHAIN --profile minimal --component rust-src
COPY . $SRC/swoole-rs
WORKDIR $SRC/swoole-rs
COPY .clusterfuzzlite/build.sh $SRC/
