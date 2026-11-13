#!/bin/sh
# Run each tooling test file in its own process, in parallel. Most tests wait on
# compilers, git or ccache subprocesses, so files overlap well.
cd "$(dirname "$0")/../.." || exit 1
logs=$(mktemp -d)
pids=
for test in tests/dev/test_*.py; do
    name=${test##*/}
    python3 -m unittest discover -b -s tests/dev -p "$name" -q > "$logs/$name.log" 2>&1 &
    pids="$pids $!:$name"
done
# Module unit tests that need no build or pinned source, run from their own
# directory so they import their siblings.
for test in \
    tests/modules/integration/test_bridge_inventory.py \
    tests/modules/phpcollections/test_runtime.py \
    tests/modules/phphelpers/test_runtime.py \
    tests/modules/utilities/test_storage_summary.py \
    tests/modules/websocket/test_autobahn_verdict.py
do
    name=$(echo "$test" | tr / _)
    (cd "${test%/*}" && python3 -m unittest -b -q "$(basename "$test" .py)") > "$logs/$name.log" 2>&1 &
    pids="$pids $!:$name"
done
status=0
for entry in $pids; do
    if ! wait "${entry%%:*}"; then
        status=1
        echo "FAILED ${entry#*:}"
        cat "$logs/${entry#*:}.log"
    fi
done
rm -rf "$logs"
exit $status
