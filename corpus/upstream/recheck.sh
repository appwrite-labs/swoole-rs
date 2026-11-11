#!/usr/bin/env bash
# Re-run the PHPT gate's flagged tests up to three times per engine, upstream first. Upstream
# stops at a test's first failed re-run; cargo then re-runs every flagged test until its
# verdict is settled (a pass, and also a failure unless upstream failed), once only when its
# first failure timed out (unless upstream timed out too), up to nine times when upstream
# failed one or until cargo passes one (gate.py --print-pending).
# An inconclusive re-run (the #200 runner abort) is replaced, up to twice the budget in all.
# A test cargo both passed and failed then gets more upstream re-runs (up to 24 in all),
# so a test racy on both engines isn't counted as a cargo regression; so does one whose
# re-runs both engines failed (up to 9).
#   recheck.sh RESULTS_DIR ENGINES_DIR RECHECK_DIR [run.py arguments...]
# ENGINES_DIR holds upstream.so and each other engine as <engine>.so; RECHECK_DIR receives <engine>-<attempt>.json.
set -eo pipefail
results=$1 engines=$2 recheck=$3
shift 3
mkdir -p "$recheck"
pending() {
  local engine=$1
  set -- "$results"/*.json
  if ls "$recheck"/*.json >/dev/null 2>&1; then set -- "$@" --recheck "$recheck"/*.json; fi
  # It prints an empty line when nothing is pending; that must not become a bare --case.
  python3 corpus/upstream/gate.py "$@" --print-pending "$engine" | sed -n 's/^\(..*\)$/--case \1/p' | tr '\n' ' '
}
# Upstream first, then every other engine in ENGINES_DIR.
for engine in upstream $(cd "$engines" && ls -- *.so | sed 's/\.so$//' | grep -vx upstream); do
  for attempt in $(seq 18); do
    cases=$(pending "$engine")
    [ -n "$cases" ] || break
    # shellcheck disable=SC2086 # one --case per word
    python3 corpus/upstream/run.py "$@" $cases --engine "$engine=$engines/$engine.so" \
      --output "$recheck/$engine-$attempt.json" || [ $? -eq 1 ]
  done
done
for attempt in $(seq 7 54); do
  cases=$(pending upstream-extended)
  [ -n "$cases" ] || break
  # shellcheck disable=SC2086 # one --case per word
  python3 corpus/upstream/run.py "$@" $cases --engine "upstream=$engines/upstream.so" \
    --output "$recheck/upstream-$attempt.json" || [ $? -eq 1 ]
done
