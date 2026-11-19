#!/usr/bin/env bash
# download.sh RUN DIRECTORY [gh run download options]: a run's artifacts into DIRECTORY,
# retried, because artifact storage throttles with HTTP 503 (#2378).
set -u
run=$1 directory=$2
shift 2
for attempt in 1 2 3 4 5; do
    rm -rf "$directory"
    gh run download "$run" "$@" -D "$directory" && exit 0
    [ "$attempt" != 5 ] || break
    echo "::warning::Retrying artifact download for run $run (attempt $((attempt + 1)) of 5)"
    sleep "${DOWNLOAD_BACKOFF:-$((attempt * 20))}"
done
exit 1
