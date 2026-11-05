# Storage layout measurements

Measured on 2026-11-05, Apple M4 Pro, macOS 26.6.2 arm64, rustc 1.99.0
(LLVM 23.1.1). Baseline: `4a71237c75c2c7948fca37c7ced5cfc644247ae4`.
[Raw trials, allocation probes, source hashes and executable hashes](../benchmarks/storage-results.json).

## Changes

- Buffer queues reuse up to 64 KiB / 64 payload blocks. Cache descriptors are
  admitted best-effort after a repeated successful append; popping never
  allocates. One-shot queues do not allocate a cache. Chunk boundaries, stable
  front pointers, typed generations, accounting and append rollback stay intact.
- HTTP directory names share one byte vector with offset/length descriptors.
  Sorting and duplicate suppression operate on descriptors; finish does not
  allocate. Clone copies the two buffers. Duplicate scan input may leave
  unreferenced bytes until the directory owner is dropped.
- Coroutine registry and generic heap nodes share a small internal stable slab.
  The first node remains an individual allocation; subsequent allocations can
  use 128 reusable slots, with individual allocations beyond that bound.
  Idle slab retention is 4 KiB per coroutine registry / 3 KiB per heap on this
  target, plus metadata and the heap's original single-spare fast path.
  Native cursors, node layouts, traversal order and tokens retain their contracts.
  The shared module avoids adding a dependency to standalone rustc gates.
- Reactor socket nodes use checked 32-bit internal links, shrinking from 40 to
  32 bytes. Public cursors and token widths stay intact. At most `u32::MAX`
  slots are representable; insertion checks the bound before publication.
  A cold end-of-iteration helper keeps the ordinary traversal branch hot.
- Timer heap repair moves 16-byte deadline/slot keys. Full timer records stay
  in reusable slots, with positions updated directly instead of hashing every
  moved ID. The public `TimerView` layout and native callback custody stay intact.

## Method

`benchmarks/storage.py` builds the same Rust public-ABI harness twice with
Cargo release optimization, against baseline and candidate sources. Each
workload verifies statuses and results; every timed checksum must match the
baseline. Allocation probes run separately with accounting disabled during
all timed trials. The allocation wrapper still has a disabled tracking branch.

There are seven trials per engine per workload, alternating engine order. A
preflight calibrates iteration counts toward at least 100 ms for the faster
engine. Times cover the whole workload including setup and teardown, excluding
process startup and a separate warmup. Builds, tests and other agent workloads
were stopped before measurement. CPU cores were not pinned. Small differences
remain uncertain; these results are not PHP/server throughput measurements.

The `cold` workloads construct a new owner each cycle in a warmed process.
Other buffer/registry/heap workloads reuse one owner across cycles. Registry
workloads insert, look up and clear; heap workloads push descending priorities
and pop in verified order; directory workloads scan, sort and look up every
name; reactor workloads traverse a populated registry. Timer workloads update
existing deadlines, or repeatedly add/remove 512 timers (`timer-churn-512`).

Allocation counts cover the entire calibrated workload, so compare the two
engines within a row. Peak bytes are simultaneously live sizes requested from
Rust's allocator, including fixture data. They exclude allocator headers,
fragmentation, native allocations and RSS.

| Workload | Baseline time / candidate time | Allocation calls, baseline → candidate | Peak requested bytes, baseline → candidate |
| --- | ---: | ---: | ---: |
| buffer-1k | 2.150× | 5,222,147 → 132 | 133,184 → 134,752 |
| buffer-16k | 1.175× | 583,991 → 12 | 131,264 → 131,392 |
| buffer-cold-1k | 0.984× | 2,475,398 → 2,475,398 | 133,184 → 133,216 |
| directory-1024 | 1.228× | 1,471,373 → 30,887 | 80,928 → 72,760 |
| directory-17 | 1.041× | 1,790,664 → 895,341 | 1,735 → 1,698 |
| registry-128 | 3.509× | 15,493,640 → 10 | 5,728 → 6,560 |
| registry-cold-1 | 0.983× | 4,201,044 → 4,201,044 | 104 → 128 |
| registry-cold-128 | 2.361× | 9,320,760 → 685,350 | 5,728 → 6,560 |
| registry-512 | 1.192× | 5,133,834 → 3,850,379 | 22,816 → 22,840 |
| heap-128 | 2.248× | 7,611,621 → 4 | 4,136 → 4,184 |
| heap-512 | 1.079× | 3,303,107 → 2,475,716 | 16,424 → 16,448 |
| heap-1 | 1.003× | 3 → 3 | 72 → 96 |
| reactor-4096 | 1.443× | 24 → 24 | 240,912 → 208,144 |
| reactor-65536 | 1.639× | 32 → 32 | 3,856,032 → 3,331,744 |
| timer-512 | 1.193× | 18 → 26 | 54,904 → 67,232 |
| timer-4096 | 1.199× | 24 → 35 | 438,392 → 536,736 |
| timer-churn-512 | 1.146× | 18 → 26 | 54,904 → 67,232 |

The largest gains occur when the bounded pools cover the working set. The
512-node workloads spill beyond the slab, so allocation reductions and speedups
are smaller. Timer throughput improves while total peak requested memory rises
about 22%: its frequently moved keys are smaller, but the separate slot storage
adds metadata. Cold buffer/one-record registry cases are approximately 2% slower;
the one-element heap is effectively tied. No universal speedup is claimed.

## Validation

- 499 workspace Rust tests pass; Clippy with `-D warnings`, rustfmt and migration
  checks pass.
- 331 tests across the six affected crates pass with Rust AddressSanitizer.
  The precompiled standard library is not instrumented. Changed memory code was
  rechecked after the final heap adjustment.
- Native coroutine registry ordering comparison passes; native socket source
  ordering and sanitizer gate passes.
- Sanitized timer C ABI gate passes all 13 topics; the built extension's
  sanitized native timer adapter passes all 14 scenarios.
- The immutable six-module extension builds with legacy directory ownership and
  socket registry ownership enabled. Static HTTP responses match upstream.
- Official original PHPT runner: upstream 34/34 pass; candidate 33/34 pass.
  `swoole_http_server/create_request.phpt` fails its random cookie-fragmentation
  sample. The existing deterministic sweep confirms exactly matching outcomes
  across both engines: 67 of 501 cuts fail the original expectations on each.
  The failed PHPT and diagnostic remain failures, not retried into a pass.
- Existing directory C++ fixture gate fails to compile missing `RewriteRulesField`
  / context declarations. Existing memory gate refuses the fixed-pool overlay
  because an original `slice->lock == 1` assertion is missing. Both gate failures
  reproduce using the saved baseline scripts and source overlays, before reaching
  the changed Rust. These gates are not reported as passed and their checks were
  not weakened. No upstream source or patch assertions were changed here.

Detailed local validation receipts remain under `target/reports/`:
`registry-rust.json`, `registry-order.json`, `sockets-native.json`,
`timer-kernel.json`, `timer-hybrid.json`, `static-server.json`,
`storage-phpt.json`, `request-splits.json`, `directory-native.json`,
`directory-baseline.json`, `memory-native.json`, and `memory-baseline.json`.
Workspace, Clippy and Rust ASan logs remain under `target/performance/`.

## Reproduce

Prepare an untouched baseline and run after all builds/tests finish:

```sh
mkdir -p target/storage-baseline
git archive 4a71237c75c2c7948fca37c7ced5cfc644247ae4 | tar -x -C target/storage-baseline
python3 benchmarks/storage.py --baseline target/storage-baseline \
  --output target/reports/storage.json
```

`--prepare-only` builds without measuring. `--repeat` controls the trial count
(minimum three); `--min-ms` controls calibration. Raw reports include the
per-workload iteration count, all trials, allocator probes, source fingerprints,
compiler/host identity and executable hashes. Source changes during measurement
are rejected.
