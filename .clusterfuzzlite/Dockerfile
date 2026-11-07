FROM gcr.io/oss-fuzz-base/base-builder-rust:v1
# crates/websocket's SHA-1 comes from OpenSSL (openssl-sys); build.sh links it statically.
RUN apt-get update && apt-get install -y --no-install-recommends libssl-dev pkg-config
COPY . $SRC/swoole-rs
WORKDIR $SRC/swoole-rs
COPY .clusterfuzzlite/build.sh $SRC/
