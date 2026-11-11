#!/bin/sh
# System packages for building and testing swoole-rs and the upstream oracle. The devcontainer image and
# CI both run this, so local and CI environments cannot drift. Extra packages a job
# needs go as arguments. APT_CACHE (set by .github/actions/install) keeps the
# package files and indexes between CI runs, so most runs never touch the mirror.
set -e
[ "$(id -u)" = 0 ] || exec sudo env APT_CACHE="${APT_CACHE:-}" "$0" "$@"
export DEBIAN_FRONTEND=noninteractive
# The CI mirror sometimes throttles one connection for minutes; give up on a
# stalled download quickly and retry instead of waiting out apt's long default.
APT='-o Acquire::http::Timeout=15 -o Acquire::https::Timeout=15 -o Acquire::Retries=5'
# In apt.conf.d, so a job's later bare apt-get (variant.yml's valgrind) reads these indexes too (#2958).
if [ -n "$APT_CACHE" ]; then
    mkdir -p "$APT_CACHE/archives/partial" "$APT_CACHE/lists/partial"
    printf 'Dir::Cache::Archives "%s/archives";\nDir::State::Lists "%s/lists";\n' "$APT_CACHE" "$APT_CACHE" > /etc/apt/apt.conf.d/50apt-cache
fi
install() {
    apt-get $APT install -yq --no-install-recommends \
        php-dev libclang-dev php-sqlite3 php-curl php-xml php-mbstring python3 python3-pytest composer \
        libpcre2-dev libssl-dev libc-ares-dev zlib1g-dev pkg-config cmake g++ make autoconf git \
        ca-certificates curl libgtest-dev libnghttp2-dev libpq-dev libcurl4-openssl-dev libssh2-1-dev libbrotli-dev libzstd-dev unixodbc-dev libsqlite3-dev libffi-dev locales procps strace "$@"
}
# Cached indexes (refreshed weekly by the cache key) are enough when every package
# is cached too; if one isn't and the mirror has moved on, refresh and retry once.
# A refresh can fail on one repository mid-sync while the others are fine, so only
# the install decides: as the last command of each branch, its failure stops sh.
if [ -n "$APT_CACHE" ] && ls "$APT_CACHE"/lists/*_Packages >/dev/null 2>&1; then
    install "$@" || { apt-get $APT update -q || :; install "$@"; }
else
    apt-get $APT update -q || :
    install "$@"
fi
[ -z "$APT_CACHE" ] || chmod -R a+rX "$APT_CACHE"
# liburing for upstream's --enable-iouring build. Upstream needs >= 2.8
# and Ubuntu 24.04 ships 2.5, so build the release upstream's own CI installs
# (scripts/install-liburing.sh) into /usr/local, where pkg-config and ld find it.
if ! pkg-config --atleast-version=2.8 liburing 2>/dev/null; then
    liburing=$(mktemp -d)
    curl -fsSL --retry 5 -o "$liburing/src.tar.gz" https://github.com/axboe/liburing/archive/refs/tags/liburing-2.13.tar.gz
    echo "618e34dbea408fc9e33d7c4babd746036dbdedf7fce2496b1178ced0f9b5b357  $liburing/src.tar.gz" | sha256sum -c -
    tar -xzf "$liburing/src.tar.gz" -C "$liburing" --strip-components=1
    (cd "$liburing" && ./configure --prefix=/usr/local >/dev/null && make -j"$(nproc)" install >/dev/null)
    ldconfig
    rm -rf "$liburing"
fi
# Locales some original PHPTs switch to.
localedef -i en_US -f ISO-8859-1 en_US.ISO8859-1
localedef -i tr_TR -f ISO-8859-9 tr_TR.ISO8859-9
localedef -i tr_TR -f UTF-8 tr_TR.UTF-8
