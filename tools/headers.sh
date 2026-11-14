#!/bin/sh
# Regenerate native/include/swoole_rs_<crate>.h for every crate with a cbindgen.toml.
# --check leaves files alone and fails, with a diff, if any committed header is stale,
# or if a hand-written swr_v<N>_* prototype under native/ disagrees with cbindgen
# (tools/headers.py).
set -e
cd "$(dirname "$0")/.."
status=0
for config in crates/*/cbindgen*.toml; do
    crate=$(dirname "$config")
    suffix=$(basename "$config" .toml)
    suffix=${suffix#cbindgen}
    # An -ffi crate exports its safe crate: crates/error-ffi writes swoole_rs_error.h.
    name=$(basename "$crate")
    header=native/include/swoole_rs_${name%-ffi}$suffix.h
    generated=$(mktemp)
    input=$crate
    if [ -n "$suffix" ] && [ -f "$crate/src/${suffix#_}.rs" ]; then
        input=$crate/src/${suffix#_}.rs
    fi
    cbindgen --quiet --config "$config" --output "$generated" "$input"
    if [ -n "$suffix" ] && [ -f "$crate/src/${suffix#_}.rs" ]; then
        python3 tools/header_strings.py "$crate/src/${suffix#_}.rs" "$generated"
    fi
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
# The typed SDK hook inventory also owns the C call-site dispatch aliases.
generated=$(mktemp)
header=native/hooks/php_swoole_ssh2_hook.h
cargo run --quiet --locked -p swoole-rs-ssh2 --bin ssh2-headers > "$generated"
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
generated=$(mktemp)
header=native/corebase/switches.cc
cargo run --quiet --locked -p swoole-rs-corebase --bin switches > "$generated"
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
if [ "$1" = --check ]; then
    python3 tools/process_records.py --check || status=1
    python3 tools/headers.py || status=1
else
    python3 tools/process_records.py
fi
exit $status
