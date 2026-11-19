#!/usr/bin/env bash
# A variant workflow's upstream report from main's latest merge-tier run of it, while nothing it
# depends on has changed since and it ran on this PHP (tools/ci.py phpt-baseline), into
# target/phpt/VARIANT/results/upstream.json, so a PR runs the variant's families on cargo alone.
# With no baseline, nothing is copied and both engines run.
#   baseline.sh VARIANT [PHP]
set -eo pipefail
variant=$1 php=${2:-php}
# phpt-baseline exits 2 when no baseline stands; any other failure is a failed lookup.
run=
if found=$(python3 tools/ci.py phpt-baseline --variant "$variant" --php "$("$php" -r 'echo PHP_VERSION;')"); then
    read -r run _ <<< "$found"
elif [ $? -ne 2 ]; then
    exit 1
fi
if [ -n "$run" ] && gh run download "$run" -n "phpt-$variant" -D target/baseline; then
  mkdir -p "target/phpt/$variant/results"
  cp target/baseline/results/upstream.json "target/phpt/$variant/results/upstream.json"
  echo "Upstream's report: merge-tier run $run"
else
  echo "No baseline: upstream runs here"
fi
