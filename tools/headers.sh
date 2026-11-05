#!/bin/sh
# Regenerate native/include/swoole_rs_<crate>.h for every crate with a cbindgen.toml.
# --check leaves files alone and fails, with a diff, if any committed header is stale,
# or if a hand-written swr_v1_* prototype under native/ disagrees with cbindgen
# (tools/headers.py).
set -e
cd "$(dirname "$0")/.."
status=0
for config in crates/*/cbindgen.toml; do
    crate=$(dirname "$config")
    header=native/include/swoole_rs_$(basename "$crate").h
    generated=$(mktemp)
    cbindgen --quiet --config "$config" --output "$generated" "$crate"
    if [ "$1" = --check ]; then
        if ! cmp -s "$generated" "$header"; then
            echo "$header is stale; run tools/headers.sh" >&2
            diff -u "$header" "$generated" >&2 || true
            status=1
        fi
        rm -f "$generated"
    else
        mv "$generated" "$header"
    fi
done
if [ "$1" = --check ]; then
    python3 tools/process_records.py --check || status=1
    python3 tools/headers.py || status=1
else
    python3 tools/process_records.py
fi
exit $status
