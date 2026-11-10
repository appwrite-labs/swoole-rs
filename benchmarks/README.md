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
Cargo build/upstream ratio is stable on shared runners where timings are not.
The merge tier's `callgrind` job runs it for `plaintext`, `echo-1k`, `json` and
`service-health` on the Cargo build (`tools/build_ext.py --profile release-lto`) and fails if a ratio rises more than
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

Two measure compression against `benchmarks/compression` (#2649): `gzip` posts
4 KiB of JSON with `Accept-Encoding: gzip` and checks the gzip-encoded echo, and
`websocket-deflate` echoes 1 KiB of JSON with permessage-deflate negotiated, so
both directions are compressed. Their records started 5% above an arm64 Linux
run (0.99 each); the ratchet lowers them once two merge runs beat them.

Three measure `Swoole\Server` without HTTP (#2650), each echoing 1 KiB on one
connection: `tcp-length` (length-prefixed frames, `07-tcp-framing`), `tcp-eof`
(`open_eof_split` on `\r\n`) and `udp` (a datagram listener), both against
`benchmarks/framing`. Their records started 5% above an arm64 Linux run
(0.97, 0.52 and 1.04); the ratchet lowers them once two merge runs beat them.

Five send larger or concurrent traffic to `benchmarks/traffic` (#2652), a server
of its own so its static handler's per-request file check stays out of the other
workloads: `http2-concurrent` (16 HTTP/2 streams at once, each posting 4000 bytes
that come back), `websocket-64k` (one 64 KiB frame echoed), `websocket-fragmented`
(a 4 KiB message sent as four fragments, echoed as one frame), `upload` (a
multipart POST with a 64 KiB file) and `static` (a 16 KiB file from
`enable_static_handler`). Their records started 5% above an arm64 Linux run
(1.26, 0.57, 0.73, 1.00 and 1.05).

`mysql` runs a prepared `SELECT` per request through hooked PDO mysql against
`benchmarks/mysql`, on one connection per worker. It needs MySQL on `MYSQL_HOST`
(default `127.0.0.1`, root/root, database `test`) and `--php-extension mysqlnd
--php-extension pdo --php-extension pdo_mysql`. Its record started at a
provisional 1.10; the ratchet lowers it once two merge runs beat it.

```sh
python3 tools/build_ext.py --profile release-lto
python3 benchmarks/callgrind.py --requests 1000 --workload plaintext --workload echo-1k --limits benchmarks/callgrind_limits.json \
  --engine cargo=target/ext/swoole.so \
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
(cd target/worktrees/base && python3 tools/build_ext.py --profile release-lto)
python3 tools/build_ext.py --profile release-lto
python3 benchmarks/callgrind.py --requests 1000 --workload plaintext --profiles \
  --engine base=target/worktrees/base/target/ext/swoole.so \
  --engine branch=target/ext/swoole.so
git worktree remove --force target/worktrees/base
```

## Wall clock and perf counters (nightly)

`benchmarks/wall.py` drives each engine at fixed concurrency (wrk for HTTP/1, h2load
for HTTP/2, a WebSocket echo loop) with engines interleaved over several repetitions,
and records requests/s, p50/p99 latency and `perf stat` task-clock, cycles,
instructions, branch and cache misses per request of the server's processes, each as
a median with its spread over the repetitions that counted it (one lost `perf stat`
sample is left out, not a failure). Wall time on shared runners is noise, so `--max-ratio`
judges only a counter ratio (`--counter`, default task-clock: GitHub-hosted runners
expose no hardware counters, so cycles and the rest are recorded only where counted).
The nightly `bench.yml` runs it with results as an artifact; never run it on a loaded
machine.

## Memory (nightly)

`benchmarks/memory.py` starts each engine's server for a workload and reads
`/proc/<pid>/smaps_rollup` of every server process at three points: idle after the
warmup, after `--requests` more requests, and while `--connections` clients each also
hold a connection open. It reports PSS (the footprint, with pages the forked processes
share split between them), USS, RSS and private dirty pages. PSS and USS also split
file pages with unrelated processes mapping the same files (any other PHP), so compare
engines on RSS and dirty pages. The
per-connection and per-request figures use dirty pages: heap, stacks and written data
are what connections and leaks grow, and the kernel can't drop them the way it drops
clean code pages. The nightly `bench.yml` runs it after `wall.py` and puts the table in
the job summary. It needs no quiet machine: memory doesn't depend on timing.

```sh
python3 benchmarks/memory.py --workload plaintext --workload task \
  --engine cargo=target/ext/swoole.so \
  --engine upstream=target/upstream/src/modules/swoole.so
```

To see which mappings make up a difference, sum `Pss` and `Private_Dirty` per pathname
in `/proc/<pid>/smaps` of each engine's idle server.

## Module microbenchmarks

`dev.py candidate --benchmark` runs the timer, buffer and WebSocket
microbenchmarks of the modules it selects. For native buffer comparisons, retain both pop-only and `--peek-front` reports
from `tests/modules/buffer/benchmark.py`. The latter reads one byte per DATA
front before pop, exercising repeated cached-front calls and verifying a checksum
outside timing. They are different workloads, not a before/after speedup ratio.

## Rust storage microbenchmarks

`storage.py --baseline PATH --output target/storage-results.json` builds the
same release harness against an untouched baseline and this checkout. It covers
buffer reuse, heap node pooling and timer heap keys, with
seven alternating trials calibrated toward at least 100 ms for the faster engine.
Allocation probes run separately from timing; all timed checksums must match.
Use `--prepare-only` to build without measuring. Stop builds and tests before
measurement. These Rust public-ABI workloads do not measure PHP/server throughput.
Keep raw reports under `target/` and put findings on the PR or issue.
