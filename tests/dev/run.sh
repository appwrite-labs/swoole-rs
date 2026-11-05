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
    tests/modules/clients/test_gate_output.py \
    tests/modules/integration/test_bridge_inventory.py \
    tests/modules/memory/test_benchmark.py \
    tests/modules/phpcollections/test_prepare_benchmark.py \
    tests/modules/phpcollections/test_runtime.py \
    tests/modules/phphelpers/test_entrypoint.py \
    tests/modules/phphelpers/test_facade_identity.py \
    tests/modules/phphelpers/test_php_benchmark.py \
    tests/modules/phphelpers/test_runtime.py \
    tests/modules/phphelpers/test_sdk.py \
    tests/modules/ports/test_configured_flags.py \
    tests/modules/ports/test_count_checkpoints.py \
    tests/modules/reactor/test_buffer_typed_identity.py \
    tests/modules/tls/test_gate.py \
    tests/modules/tls/test_php.py \
    tests/modules/utilities/test_overlay.py \
    tests/modules/utilities/test_prepare_benchmark.py \
    tests/modules/utilities/test_storage_summary.py \
    tests/modules/wrappers/test_model_checkpoints.py
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
