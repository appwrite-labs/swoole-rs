# Basic benchmarks

Four response-validated HTTP workloads reuse the corpus applications: plaintext,
small JSON, 64 KiB binary echo, and the health endpoint of the multi-file SQLite
service. The last measures HTTP dispatch, not database throughput. All fixtures
use one PHP worker. The benchmark never loads both extensions into one process.

```sh
python3 tools/build_hybrid.py --module buffer --module websocket
python3 benchmarks/run.py \
  --engine hybrid=target/hybrid/rust-buffer-websocket/modules/swoole.so \
  --engine upstream=target/upstream/src/modules/swoole.so \
  --requests 2000 --concurrency 1 16 --warmup 20 --repeat 3
```

On distributions with dynamic PDO modules, also pass
`--php-extension pdo --php-extension pdo_sqlite` (the runner uses `php -n`).

Defaults run three trials per workload at concurrency 1 and 16. Each connection
performs its own warmup before a shared timed start. Trials use fresh server
processes; engine order alternates between trials. Startup, warmup, and teardown
are excluded from timing. Each client keeps one request outstanding and reuses
its connection. Total requests are distributed across connections, not multiplied
by concurrency. Responses must match status and full body; failures are counted,
shown, and make the run exit nonzero. Warmup failures abort the run.

JSON output (`target/reports/benchmarks.json`) contains every trial, successful
requests/sec, body bytes, error samples, p50/p95/p99/max successful-response latency,
and median throughput over error-free trials. Percentiles use nearest rank.
Metadata records PHP/Swoole versions, extension SHA-256, host/architecture/CPU count,
Python version, repository revision and dirty state, and benchmark parameters.
At least one `--engine` is required. Explicit paths allow any build, so verify
that comparable optimized builds were selected. Upstream's configure build uses
its default optimization flags; its full build log is retained locally.

Use `--workload plaintext`, `--requests 100 --warmup 2 --repeat 1`, or
`--output PATH` for a smoke run. CI only runs smoke validation; it does not enforce
performance thresholds on shared runners.

These are **closed-loop, localhost measurements with a Python threaded client**.
The client, Python scheduling/GIL, OS, and the same-host server can limit results.
Latency excludes failures and does not correct for coordinated omission. These
numbers are useful for repeatable smoke checks and large regressions; they do not
establish maximum server capacity or production latency. Use a separate load host
and a dedicated generator such as wrk/wrk2 for serious performance decisions.
Keep CPU allocation, PHP version/build, extension features, concurrency, request
mix, and machine load fixed, and retain raw reports. Module-specific native buffer and PHP WebSocket codec microbenchmarks are
documented in [the hybrid handoff](../docs/hybrid-build.md). No CPU, RSS, TLS,
coroutine, or database-throughput benchmarks are implemented yet.

For native buffer comparisons, retain both pop-only and `--peek-front` reports
from `tests/modules/buffer/benchmark.py`. The latter reads one byte per DATA
front before pop, exercising repeated cached-front calls and verifying a checksum
outside timing. They are different workloads, not a before/after speedup ratio.

## Rust storage layouts

`storage.py` compares an untouched baseline checkout with current Rust storage
implementations using a common optimized public-ABI harness. It covers buffers,
directory names, registry/heap nodes, reactor traversal and timer queues, with
separate allocation probes and calibrated alternating trials. See
[results, trade-offs, validation and reproduction](../docs/storage-performance.md).
These microbenchmarks do not establish PHP/server throughput.
