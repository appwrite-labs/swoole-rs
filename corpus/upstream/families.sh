#!/usr/bin/env bash
# Run PHPT families on upstream and the Cargo build, re-run flagged tests and gate them, as the
# merge tier's phpt jobs do for every family (dev.py candidate, from migration/modules.json).
#   families.sh OUT UPSTREAM_SO CARGO_SO [--module FAMILY...] [--shard I/N] [run.py arguments: --php-extension...]
# The manifest holds the families' cases, or with --shard every N-th of them from the I-th, so
# runners can split them. OUT receives the manifest, the results, the re-runs and gate.json.
set -eo pipefail
out=$1 upstream=$2 cargo=$3
shift 3
selection=() run=()
while [ $# -gt 0 ]; do
  case $1 in
    --module|--shard) selection+=("$1" "$2"); shift 2 ;;
    *) run+=("$1"); shift ;;
  esac
done
mkdir -p "$out/engines" "$out/results"
ln -sf "$upstream" "$out/engines/upstream.so"
ln -sf "$cargo" "$out/engines/cargo.so"
python3 corpus/upstream/inventory.py "${selection[@]}" --output "$out/manifest.json"
python3 -c 'import json, sys; sys.exit(not json.load(open(sys.argv[1]))["tests"])' "$out/manifest.json" ||
  { echo "No case in this shard"; exit 0; }
for engine in upstream cargo; do
  # Exit 1 means some tests failed, which the gate judges.
  python3 corpus/upstream/run.py --manifest "$out/manifest.json" --timeout 30 "${run[@]}" \
    --engine "$engine=$out/engines/$engine.so" --output "$out/results/$engine.json" || [ $? -eq 1 ]
done
corpus/upstream/recheck.sh "$out/results" "$out/engines" "$out/recheck" --manifest "$out/manifest.json" --timeout 30 "${run[@]}"
set -- "$out"/results/*.json
if ls "$out"/recheck/*.json >/dev/null 2>&1; then set -- "$@" --recheck "$out"/recheck/*.json; fi
python3 corpus/upstream/gate.py "$@" --output "$out/gate.json"
