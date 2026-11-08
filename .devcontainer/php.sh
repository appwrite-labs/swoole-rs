#!/bin/sh
# PHP 8.3 built from source in $1 with the configure options that follow, for the builds
# Ubuntu doesn't ship: thread-safe (zts.yml, --enable-zts) and sanitized (tools/asan.py).
# It has the extensions the PHPTs call (sockets, pcntl, posix, curl, openssl) compiled
# in; install.sh has the libraries. CC and CFLAGS reach configure.
# Does nothing when the prefix already holds this release with these options (CI caches it).
set -e
VERSION=8.3.36
SHA256=c93f3441f87f6ab156bef4432db215d7e61ca2c0183bb3a2aa28d9886ab59f53
PREFIX=$(realpath -m "$1")
shift
STAMP="$VERSION $* CC=$CC CFLAGS=$CFLAGS"
if [ "$(cat "$PREFIX/.options" 2>/dev/null)" = "$STAMP" ]; then
    exit 0
fi
src=$(mktemp -d)
curl -fsSL --retry 5 -o "$src/php.tar.gz" "https://www.php.net/distributions/php-$VERSION.tar.gz"
echo "$SHA256  $src/php.tar.gz" | sha256sum -c -
tar -xzf "$src/php.tar.gz" -C "$src" --strip-components=1
(cd "$src" && ./configure --prefix="$PREFIX" --disable-cgi --disable-phpdbg \
    --without-libxml --disable-dom --disable-simplexml --disable-xml --disable-xmlreader --disable-xmlwriter \
    --without-sqlite3 --without-pdo-sqlite --enable-sockets --enable-pcntl --enable-mbstring --disable-mbregex \
    --with-curl --with-openssl --with-zlib "$@" >/dev/null && make -j"$(nproc)" >/dev/null && make install >/dev/null)
rm -rf "$src"
echo "$STAMP" > "$PREFIX/.options"
"$PREFIX/bin/php" -v
