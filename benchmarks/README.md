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
2% above its record in `callgrind_limits/<workload>` (the ratchet; see
[Speed](../docs/migration.md#speed)). On distributions with dynamic PDO modules, pass `--php-extension pdo
--php-extension pdo_sqlite` (the servers run under `php -n`). It uses `echo-1k`
rather than `echo-64k`: how a 64 KiB request is split across reads differs between
runners, and that changes how much of it the Expect-header scan walks (#338).

A new or reset record comes from the merge tier's x86_64 numbers (its `callgrind`
artifact), set 5% above them; a local arm64 run doesn't predict them. Older records
noted below as set from arm64 runs keep those values until the ratchet lowers them. On x86_64, glibc copies and fills
large buffers with `rep movsb`/`rep stosb`, which callgrind counts once per byte, so a
64 KiB `memcpy` is about 65,000 instructions there and a few thousand on arm64. Bulk
copies both engines make weigh far more on x86_64 and pull its ratio towards 1.0
(`websocket-64k` is 0.56 on arm64 and 0.98 on x86_64), and a copy or fill only one
engine makes weighs far more against it (#2778).

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

Every other HTTP workload reuses one kept-alive connection. `connect` (#2651) is
`plaintext` on a new connection per request, sent with `Connection: close`, and the
client waits for the server's close, so accepting the connection, setting up its
session and tearing it down count in every request. Its record started 5% above an
arm64 Linux run (0.99), but the x86 merge tier measures about 1.085, so it was reset to
1.09. Most of that was ~29 op dispatches a connection through `master_ops::answer`;
#2748 answers them inline (8% fewer instructions on arm64), and the ratchet lowers the record.

Three measure outbound coroutine clients against `benchmarks/clients` (#2647),
each on a connection the worker keeps: `http-client` (a `Coroutine\Http\Client`
GET), `client` and `socket` (a 1 KiB send/recv through `Coroutine\Client` and
`Coroutine\Socket`). `callgrind.py` serves their peers, an HTTP server and a TCP
echo, from threads of its own, so callgrind counts only the client side. The echo
replies after 2 ms, so the client waits on the reactor once per request instead of
whenever the reply happens to be late. Their records started 5% above an
arm64 Linux run (0.92, 0.91 and 0.94); the ratchet lowers them once two merge runs
beat them.

Two measure compression against `benchmarks/compression` (#2649): `gzip` posts
4 KiB of JSON with `Accept-Encoding: gzip` and checks the gzip-encoded echo, and
`websocket-deflate` echoes 1 KiB of JSON with permessage-deflate negotiated, so
both directions are compressed. Their records started 5% above an arm64 Linux
run (0.99 each); the ratchet lowers them once two merge runs beat them.

Three measure `Swoole\Server` without HTTP (#2650), each echoing 1 KiB on one
connection: `tcp-length` (length-prefixed frames, `07-tcp-framing`), `tcp-eof`
(`open_eof_split` on `\r\n`) and `udp` (a datagram listener), both against
`benchmarks/framing`. The `udp` client waits 2 ms after each reply, so the server's
read loop always ends on EAGAIN and waits on the reactor once per datagram; otherwise
whether the next datagram beat that last read moved the count by up to 10% (#2899). Their records started 5% above an arm64 Linux run
(0.97, 0.52 and 1.04); the ratchet lowers them once two merge runs beat them.

Two measure TLS against `benchmarks/tls` (#2648), which serves the pin's test
certificate (`tests/include/ssl_certs/server.crt`, RSA 4096) so it needs
`target/upstream/src`: `https` (GETs on one kept-alive connection: the record
layer) and `wss` (the WebSocket echo over TLS). Their records started 5% above an
arm64 Linux run (0.99 and 0.99); the ratchet lowers them once two merge runs beat
them. A full handshake per request is not measured: OpenSSL's RSA signature is
about 56 million instructions of it on both engines, so the ratio reads 1.0000
whatever Swoole does, and the run takes minutes. `connect` covers the
per-connection cost Swoole itself adds.

`atomic` (#2654) adds 64 values to a `Swoole\Atomic`, each under a `Swoole\Lock`
mutex, against `benchmarks/primitives`, and `process-mode` is `plaintext` against
`benchmarks/process`: `SWOOLE_PROCESS` with two workers, so the master's reactor
reads each request and dispatches it to a worker, which every other workload
skips. Their records started 5% above an arm64 Linux run (0.97 and 0.90).
`Swoole\Thread` and io_uring file reads need a ZTS PHP and the
`--enable-swoole-thread` / `--enable-iouring` builds of both engines, which this
job doesn't make.
Five send larger or concurrent traffic to `benchmarks/traffic` (#2652), a server
of its own so its static handler's per-request file check stays out of the other
workloads: `http2-concurrent` (16 HTTP/2 streams at once, each posting 4000 bytes
that come back), `websocket-64k` (one 64 KiB frame echoed), `websocket-fragmented`
(a 4 KiB message sent as four fragments in one write, echoed as one frame; #2954),
`upload` (a multipart POST with a 64 KiB file) and `static` (a 16 KiB file from
`enable_static_handler`). Their records started 5% above an arm64 Linux run
(1.26, 0.57, 0.73, 1.00 and 1.05), which the x86_64 merge tier never met (#2778).
`http2-concurrent` measured 3.56 there until #2712 and 1.09 after it; 66,000 of its
444,000 instructions a request were the static handler zero-filling a 4 KiB filename
buffer for each of the 16 streams, which it no longer does. `websocket-64k` was two
64 KiB copies on both engines, so its ratio stayed near 1.0 on x86_64; the Cargo build now
makes one, sending the pushed payload from the PHP string behind the frame's header in one
`writev`. The ratchet has
lowered all five since.

`mysql` runs a prepared `SELECT` per request through hooked PDO mysql against
`benchmarks/mysql`, on one connection per worker. It needs MySQL on `MYSQL_HOST`
(default `127.0.0.1`, root/root, database `test`) and `--php-extension mysqlnd
--php-extension pdo --php-extension pdo_mysql`. Its record started at a
provisional 1.10; the ratchet lowers it once two merge runs beat it.

Four measure database and network hooks against `benchmarks/hooks` (#2653), one
round trip per request: `pgsql` and `sqlite` run a prepared `SELECT` through
hooked PDO (PostgreSQL on `PGSQL_HOST`, default `127.0.0.1`, root/root, database
`test`; SQLite in a file under the server's data directory), `curl` is a GET to
the HTTP peer with `SWOOLE_HOOK_NATIVE_CURL`, and `dns` a
`Coroutine\System::dnsLookup` through c-ares, with the DNS cache off, that a
resolver `callgrind.py` serves answers. They need both engines built with
`--configure=--enable-swoole-pgsql --configure=--enable-swoole-sqlite
--configure=--enable-swoole-curl --configure=--enable-cares` and `--php-extension pdo
--php-extension pdo_sqlite --php-extension curl`, but not PHP's `pdo_pgsql`: that build
registers its own PDO pgsql driver. Building the main pair that way would shift every
other workload's record, so the merge tier runs them on a second pair, as a fourth
`callgrind` shard whose result the ratchet reads with the other three.

Two run only on a build variant (#2683), against `benchmarks/variants`. `uring-file`
reads a file with `Coroutine\System::readFile`, which the `--enable-iouring` build
does through io_uring, and `thread` starts and joins a `Swoole\Thread`, which needs
the ZTS PHP and `--enable-swoole-thread`. The merge tier's `variant.yml` counts each
one on the pair its variant already builds (release profile, not `release-lto`), so
their records compare only with themselves. Their results go in the variant's
`phpt-<variant>` artifact, which the ratchet doesn't read: lower those records by hand.
`thread` reads at parity by construction, like a full TLS handshake: starting a
thread boots a ZTS PHP request in it, and its profile (`profiles-zts-callgrind`)
puts 92.7% of the instructions in `php` and 6.9% in libc on both engines, against
0.05% in the extension (691,776 for cargo, 743,664 plus 20,411 in libstdc++ for
upstream, over 1.37 billion). It stays as a check that a thread still starts and
joins; `uring-file`, at 59.5% (cargo) and 41.7% (upstream) in the extension, does
measure Swoole's work.

```sh
python3 tools/build_ext.py --profile release-lto
python3 benchmarks/callgrind.py --requests 1000 --workload plaintext --workload echo-1k --limits benchmarks/callgrind_limits \
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

WebSocket uses one connection because its fixture broadcasts to every connected client.
`--websocket-pipeline` (default 32) sends a bounded batch of messages before waiting for
their echoes; `--connections` applies to HTTP. Every echo is validated, and each message's
latency runs from its batch's send to the read completing that echo. Set the pipeline to 1
to reproduce the serial round-trip workload used before #3409. Compare results only at
the same pipeline size (recorded in the JSON); batching changes latency and server work
per read. `client_cpu` records load-generator CPU seconds per wall second, so a client near
one core (two for wrk) can bound the measured throughput even when server CPU is lower.

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
buffer reuse and the timer queue (crates/timer's `Timer`), with
seven alternating trials calibrated toward at least 100 ms for the faster engine.
Allocation probes run separately from timing; all timed checksums must match.
Use `--prepare-only` to build without measuring. Stop builds and tests before
measurement. These Rust workloads do not measure PHP/server throughput.
Keep raw reports under `target/` and put findings on the PR or issue.
