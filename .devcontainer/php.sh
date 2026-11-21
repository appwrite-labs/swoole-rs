#!/bin/sh
# PHP built from source in $1 with the configure options that follow, for the builds
# Ubuntu doesn't ship: thread-safe (variant.yml's zts, --enable-zts), sanitized (tools/asan.py) and
# the releases other than 8.3 that upstream supports (php-versions.yml, PHP_SERIES=8.2 …).
# It has the extensions the PHPTs call (sockets, pcntl, posix, curl, openssl) compiled
# in; install.sh has the libraries. CC and CFLAGS reach configure.
# Does nothing when the prefix already holds this release with these options (CI caches it).
set -e
case "${PHP_SERIES:-8.3}" in
    8.2) VERSION=8.2.35 SHA256=9db4204f0a2d1911277ae832185e07bb4c3b3559772753a70d2968ca346d6910 ;;
    8.3) VERSION=8.3.36 SHA256=c93f3441f87f6ab156bef4432db215d7e61ca2c0183bb3a2aa28d9886ab59f53 ;;
    8.4) VERSION=8.4.27 SHA256=5bafed9994c1a49796b25b5f61c22c9183bfe046b6acc7b363842d03c77f972f ;;
    8.5) VERSION=8.5.12 SHA256=81a0f450349e84b08eb43faa85f8a4c835684f3cc1c50471775d1317a046c525 ;;
    *) echo "php.sh: no PHP $PHP_SERIES release pinned" >&2; exit 1 ;;
esac
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
