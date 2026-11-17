#!/bin/sh
# One release binary, built inside the official php image it targets (release.yml:
# `docker run php:<series>-<cli|zts>-<bookworm|alpine> tools/release.sh <version>`), so it
# links that image's libc, OpenSSL, Brotli and zstd. Writes
# dist/swoole-rs-<version>-php<series>-<nts|zts>-linux-<gnu|musl>-<machine>.so.
# The default build profile, which loads in a PHP with no other extension: no sockets
# integration and no service hooks. A thread-safe PHP gets Swoole\Thread.
set -e
if command -v apk >/dev/null; then
    libc=musl
    # shellcheck disable=SC2086 # the image's word list
    apk add --no-cache $PHPIZE_DEPS git python3 openssl-dev pcre2-dev zlib-dev brotli-dev zstd-dev linux-headers clang-dev
    # Rust's std links the unwinder as -lgcc_s, and the php image on arm64 has no libgcc_s.so.1.
    # A directory that offers that name only as a linker script for gcc's static unwinder
    # (and libgcc, whose atomics it calls on arm64) wins the linker's search; --exclude-libs
    # keeps its _Unwind_* symbols out of the dynamic symbol table.
    mkdir -p /tmp/unwind
    echo "GROUP ( $(cc -print-file-name=libgcc_eh.a) $(cc -print-file-name=libgcc.a) )" > /tmp/unwind/libgcc_s.a
    # musl build scripts link statically and cannot dlopen libclang (zend-sys's bindgen).
    export RUSTFLAGS='-C target-feature=-crt-static -L native=/tmp/unwind -C link-arg=-Wl,--exclude-libs,libgcc_eh.a'
else
    libc=gnu
    apt-get update -q
    apt-get install -yq --no-install-recommends git python3 libclang-dev libssl-dev libpcre2-dev zlib1g-dev libbrotli-dev libzstd-dev
fi
# rust-toolchain.toml selects the version on first use.
curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain none
export PATH="$HOME/.cargo/bin:$PATH"
# The checkout is the runner's, not root's; build_ext.py records its commit.
git config --global --add safe.directory "$PWD"
if [ "$(php -r 'echo PHP_ZTS;')" = 1 ]; then
    threads=zts
    set -- "$@" --configure=--enable-swoole-thread
else
    threads=nts
fi
version=$1
shift
if [ "$libc" = gnu ] && [ "$(uname -m)" = aarch64 ]; then
    rustup component add llvm-tools-preview
    set -- "$@" --pgo
fi
python3 tools/build_ext.py --profile release-lto "$@"
mkdir -p dist
cp target/ext/swoole.so "dist/swoole-rs-$version-php$(php -r 'echo PHP_MAJOR_VERSION, ".", PHP_MINOR_VERSION;')-$threads-linux-$libc-$(uname -m).so"
# The shared libraries a PHP must have beside it, for the log.
readelf -d dist/*.so | grep NEEDED
