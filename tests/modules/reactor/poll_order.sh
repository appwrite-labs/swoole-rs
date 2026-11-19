#!/bin/sh
# Run the reference with the same libstdc++ that builds the pinned Swoole.
set -eu
build=$(mktemp -d)
trap 'rm -rf "$build"' EXIT
g++ -std=c++17 -O2 "$(dirname "$0")/poll_order.cc" -o "$build/poll-order"
"$build/poll-order" "$@"
