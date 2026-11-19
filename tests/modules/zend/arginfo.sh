#!/bin/sh
set -eu
build=$(mktemp -d)
trap 'rm -rf "$build"' EXIT
config=${PHP_CONFIG:-php-config}
cc $("$config" --includes) "$(dirname "$0")/arginfo.c" -o "$build/arginfo"
"$build/arginfo"
