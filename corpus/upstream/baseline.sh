#!/usr/bin/env bash
# A variant workflow's upstream report from main's latest merge-tier run of it, while nothing it
# depends on has changed since and it ran on this PHP (tools/ci.py phpt-baseline), into
# target/phpt/VARIANT/upstream.json, so a PR runs the variant's families on cargo alone.
# With no baseline, nothing is copied and both engines run.
#   baseline.sh VARIANT [PHP]
set -eo pipefail
variant=$1 php=${2:-php}
read -r run _ <<< "$(python3 tools/ci.py phpt-baseline --variant "$variant" --php "$("$php" -r 'echo PHP_VERSION;')" || true)"
if [ -n "$run" ] && gh run download "$run" -n "phpt-$variant" -D target/baseline; then
  mkdir -p "target/phpt/$variant"
  cp "target/baseline/target/phpt/$variant/upstream.json" "target/phpt/$variant/upstream.json"
  echo "Upstream's report: merge-tier run $run"
else
  echo "No baseline: upstream runs here"
fi
