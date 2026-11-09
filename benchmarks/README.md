# Benchmarks

Five response-validated HTTP workloads reuse the corpus applications: plaintext,
small JSON, 1 KiB and 64 KiB binary echo, and the health endpoint of the
multi-file SQLite service. The last measures HTTP dispatch, not database
throughput. All fixtures use one PHP worker, and each engine runs in its own
process. Wall-clock load against a deployed stack is `tools/appwrite_bench.py`.

## Instruction counts (CI)

`benchmarks/callgrind.py` counts instructions per request under callgrind
(Linux, `valgrind` installed), with instrumentation on only while requests are
served. Counts barely vary between runs of the same binaries, so the
hybrid/upstream ratio is stable on shared runners where timings are not.
The merge tier's `callgrind` job runs it for `plaintext`, `echo-1k`, `json` and
`service-health` on the all-modules hybrid and fails if a ratio rises more than
1% above its record in `callgrind_limits.json` (the ratchet; see
[Speed](../docs/migration.md#speed)). On distributions with dynamic PDO modules, pass `--php-extension pdo
--php-extension pdo_sqlite` (the servers run under `php -n`). It uses `echo-1k`
rather than `echo-64k`: how a 64 KiB request is split across reads differs between
runners, and that changes how much of it the Expect-header scan walks (#338).

The same job also runs `websocket` (a WebSocket echo against `06-websocket`) and
`timer` (an endpoint in `08-timers/server.php` that adds and clears a batch of
far-future timers per request), on the same ratchet, and `app`: `benchmarks/app`,
an application-shaped order endpoint (routing, JSON parsing and validation, three
coroutines per request doing pooled Redis `SET`/`GET` over hooked streams and a file
read). It needs Redis on `REDIS_HOST:REDIS_PORT` (default `127.0.0.1:6379`); the job
runs it as a service.

Four more cover runtime primitives (#1909): `http2` (a GET over HTTP/2 with
prior knowledge), `table` (64 `Swoole\Table` sets and gets) and `channel` (64
values through a `Coroutine\Channel` from a second coroutine), all against
`benchmarks/primitives`, and `task`, a `Server::taskwait` round trip against
`10-task-workers`.

```sh
python3 benchmarks/callgrind.py --requests 1000 --workload plaintext --workload echo-1k --limits benchmarks/callgrind_limits.json \
  --engine hybrid=target/hybrid/rust/modules/swoole.so \
  --engine upstream=target/upstream/src/modules/swoole.so
```

Results go to `target/callgrind/<timestamp>/callgrind.json` unless `--output` says
otherwise; a bare `--profiles` keeps the profiles next to them, for
`callgrind_annotate --inclusive=no`.

### Before/after

Build the other commit in a git worktree under `target/`, not a clone or a
scratch directory, so every build and profile stays inside this checkout where
`dev.py prune` can find it:

```sh
git worktree add target/worktrees/base origin/main
(cd target/worktrees/base && python3 tools/build_upstream.py && python3 tools/build_hybrid.py)
python3 tools/build_hybrid.py
python3 benchmarks/callgrind.py --requests 1000 --workload plaintext --profiles \
  --engine base=target/worktrees/base/target/hybrid/rust/modules/swoole.so \
  --engine branch=target/hybrid/rust/modules/swoole.so
git worktree remove --force target/worktrees/base
```

## Wall clock and perf counters (nightly)

`benchmarks/wall.py` drives each engine at fixed concurrency (wrk for HTTP/1, h2load
for HTTP/2, a WebSocket echo loop) with engines interleaved over several repetitions,
and records requests/s, p50/p99 latency and `perf stat` task-clock, cycles,
instructions, branch and cache misses per request of the server's processes, each as
a median with its spread. Wall time on shared runners is noise, so `--max-ratio`
judges only a counter ratio (`--counter`, default cycles). The nightly `bench.yml`
runs it with results as an artifact; never run it on a loaded machine.

## Module microbenchmarks

`dev.py candidate --benchmark` runs the timer, buffer and WebSocket
microbenchmarks of the modules it selects. For native buffer comparisons, retain both pop-only and `--peek-front` reports
from `tests/modules/buffer/benchmark.py`. The latter reads one byte per DATA
front before pop, exercising repeated cached-front calls and verifying a checksum
outside timing. They are different workloads, not a before/after speedup ratio.

## Rust storage microbenchmarks

`storage.py --baseline PATH --output target/storage-results.json` builds the
same release harness against an untouched baseline and this checkout. It covers
buffer reuse, flat directory names, heap node pooling and timer heap keys, with
seven alternating trials calibrated toward at least 100 ms for the faster engine.
Allocation probes run separately from timing; all timed checksums must match.
Use `--prepare-only` to build without measuring. Stop builds and tests before
measurement. These Rust public-ABI workloads do not measure PHP/server throughput.
Keep raw reports under `target/` and put findings on the PR or issue.
