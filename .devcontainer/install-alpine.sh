#!/bin/sh
# Alpine (musl) counterpart of install.sh for the php:8.5-alpine merge-tier job,
# which matches the libc and PHP of Appwrite's base image.
set -e
apk add --no-cache $PHPIZE_DEPS git python3 py3-pytest bash curl openssl-dev curl-dev pcre2-dev \
    zlib-dev brotli-dev zstd-dev linux-headers clang-dev nghttp2-dev postgresql-dev sqlite-dev
# ftp (with ftp_ssl_connect) for the swoole_ftp PHPTs the job runs on PHP 8.5.
docker-php-ext-configure ftp --with-ftp-ssl
docker-php-ext-install -j"$(nproc)" sockets pcntl ftp
curl -sSf https://getcomposer.org/installer | php -- --install-dir=/usr/local/bin --filename=composer --quiet
# rust-toolchain.toml selects the version on first use.
curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain none
