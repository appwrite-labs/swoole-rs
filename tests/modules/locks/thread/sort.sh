#!/bin/sh
set -eu
build=$(mktemp -d)
trap 'rm -rf "$build"' EXIT
config=${PHP_CONFIG:-php-config}
cc -shared -fPIC $("$config" --includes) "$(dirname "$0")/sort.c" -o "$build/sort.so"
"$("$config" --php-binary)" -n -d "extension=$build/sort.so" -r '
foreach (array_slice($argv, 1) as $row) {
    $keys = array_map("intval", explode(",", $row));
    foreach ([false, true] as $cyclic) {
        echo implode(" ", reference_sort($keys, $cyclic)), PHP_EOL;
    }
}' -- "$@"
