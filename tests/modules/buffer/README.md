# Buffer and String gates

Gates for the Rust `swoole::Buffer` queue (`crates/buffer`) and `swoole::String`
(`crates/buffer/src/string.rs`, gates in [string/](string/)).
Native runners compile the upstream side from pinned upstream
`c7e514509451beafd4d70ce5b78943d51e76381c`, extracted into a disposable tree, and
the Rust side from the patched tree and headers of the hybrid build the supplied
Rust archive comes from (`tools.build_hybrid.hybrid_tree`). They never rebuild the
archive. The `test_*.py` gates run under pytest (`python3 -m pytest tests/modules/buffer/<file>`) with the
shared options in `tests/conftest.py` and read sources from `target/upstream/src`. `--sanitize` adds C++ ASan/UBSan; the Rust archive is not instrumented
unless built that way and declared with `--rust-asan` (see below).

## Native queue gates

| Command | Checks |
| --- | --- |
| `baseline.py --sanitize` | Pinned upstream only: original `core-tests/src/memory/buffer.cpp` assertions (64 seeded runs) plus a scatter/gather matrix that records upstream defects. |
| `test_native.py --manifest M --sanitize` (or `--archive A`) | Same originals against the patched facade and Rust queue; the matrix must have zero byte/accounting mismatches. Also C ABI validation, native-token ownership, cached front identity, DATA callbacks on live bytes and quiescent fork. |
| `test_model.py --manifest M --sanitize` (or `--archive A`) | Six seeded 64-step sequences against an independent C++ reference queue, six facade sequences, and a fixture that replays each sequence once per allocation ordinal with that allocation failing. Failed operations must leave queue state unchanged. Details: [model.md](model.md). |
| `test_iovec.py --archive A [--sanitize]` | Twelve positive POSIX iovec append cases; native and selected layout and observation counts must match. |
| `test_callbacks.py --archive A` | A throwing destroy callback must reach `std::terminate` (exit 77) for DATA pop, native-token pop and teardown in both engines. |
| `test_typed.py --archive A [--sanitize]` | Typed queue facade compiled and run under C++14/17/20. |
| `test_typed_layout.py --archive A` | `Buffer`/`BufferChunk` sizes and offsets under typed ON/omitted/zero; omitted and zero must preprocess to the same declaration as the control header. |

Run as `python3 tests/modules/buffer/<script>`; `S` is the pinned upstream
checkout (`target/upstream/src`).

## PHP gates

- `test_network.py --engine NAME=SO ... [--php PHP]`: ordered 4 MiB streaming, server
  reported queued bytes, sendfile offset/length, keep-alive and health after a
  cancelled response.
- `lifecycle.py --engine NAME=SO ... [--php-extension EXT]`: slow readers,
  stream/sendfile cancellation, half-close, churn and pending-write shutdown
  (corpus project `11-buffer-lifecycle`).
- Original PHPTs (`originals.json`, seven Swoole v6.2.4 tests plus
  `examples/test.jpg`):
  `python3 corpus/upstream/run.py --manifest tests/modules/buffer/originals.json --source S --engine upstream=SO --engine hybrid=SO --require-all --isolate-cases`.
  On Linux PHP 8.2 add `--php-extension curl --php-extension sockets` for both engines.

## Benchmark

Run only on an idle host. `benchmark.py --build-manifest M` compares optimized
upstream and Rust queue consumers (append/pop, contiguous and vectored, 50,000
cycles, five alternating repeats). To split preparation from timing:
`--prepare-dir D --output P.json` compiles and runs untimed checks, then
`--prepared-report P.json [--peek-front] --repeat 7 --iterations 10000 --output R`
times the prepared binaries after checking their hashes. `--previous-build-manifest`
adds a previous hybrid build to the comparison.

## Rust ASan

`test_native.py` accepts `--rust-asan` with
`--sanitize` for an archive built with Rust ASan. They refuse archives without
`__asan_report_` imports. Build such an archive on Linux with
`RUSTC_BOOTSTRAP=1 RUSTFLAGS='-Zsanitizer=address -Zexternal-clangrt=yes' cargo build --locked --offline -p swoole-rs-buffer --release --target=aarch64-unknown-linux-gnu`;
the explicit target keeps host build scripts uninstrumented. On macOS the
instrumented archive fails to link because Apple's runtime lacks
`___asan_version_mismatch_check_v8`. Precompiled Rust std stays uninstrumented.

## Known failures and intentional differences

- Upstream `Buffer::append(iovec)` defects, reproduced by `baseline.py`: chunk
  size 4 at offset 3 into a six-byte vector leaves length `4294967295` after pop
  (sixteen matrix cases have accounting mismatches); offset exactly at a vector
  boundary copies the wrong vector (four cases). Empty vectors and offsets at or
  beyond the total length are not executed against upstream because it can loop
  or read out of bounds. Rust rejects or handles these explicitly.
- Upstream frees DATA storage before its destroy callback; the Rust facade runs
  DATA callbacks on live bytes (`docs/buffer-abi-v1.md`).
- `swoole_server/sendfile_02.phpt` fails in both engines: `Swoole\Client::getSocket()`
  is missing because the build lacks `SWOOLE_SOCKETS_SUPPORT`.
- The lifecycle half-close case fails on upstream (truncated response) and on any
  hybrid built without the separate half-close fix.

## Ownership

| Responsibility | Owner |
| --- | --- |
| Queue chunks, bytes, count, offset validation, failure rollback | Rust |
| Native sendfile/close tokens and callback invocation | C++, after Rust returns; DATA pop callbacks are noexcept |
| `BufferChunk` public view and mutable send offset | C++ caches the public view; callers keep its identity |
| POSIX iovec adaptation | C++ copies descriptors (8 on stack, heap fallback); Rust traverses and chunks |
| `swoole::String`'s methods (growth, edits, copy/move, search, split, random bytes) | Rust, under the upstream symbols, calling the String's own allocator |
| `split`'s `std::function` handler, `format_impl`'s printf varargs, inline accessors | C++ |

The ltrim/rtrim/strnpos/strnstr original String tests exercise native helpers in
`include/swoole_util.h` and `include/swoole.h`, not Rust code.

## Gate audit (#473)

Kept, each with what nothing else reaches:

- `buffer-native` and `string-originals` run upstream's own
  `core-tests/src/memory/buffer.cpp` and `core-tests/src/core/string.cpp`, which
  no other phase builds.
- `buffer-model`: allocation-failure replay at every allocation ordinal, plus the
  legacy facade sequences (DATA callbacks on live bytes), under ASan/UBSan.
- `buffer-typed-layout`: ABI layout of `Buffer`/`BufferChunk` under each macro value.
- `buffer-typed`: the typed facade's only C++14/17/20 builds, under sanitizers.
- `buffer-callbacks`: a throwing destroy callback must reach `std::terminate`
  across the Rust boundary; no PHPT throws from a native destructor.
- `buffer-posix-iovec`: the C++ iovec adapter (stack/heap descriptor copy) against
  upstream under sanitizers; the Rust unit tests start after that adapter.
- `buffer-string-compatibility`: positive String edits and C++14/17/20 whole-TU builds.
- `buffer-network`: `Http\Response::sendfile` with an offset and a length behind
  4 MiB of queued output; no original PHPT passes a length.

Deleted:

- `string-{split,integer,ownership,allocation,format,random,copy,transitions}`:
  they drove the String step-machine exports, which whole Rust methods under the
  upstream symbols replaced; `string-originals` runs upstream's assertions on them.
- `buffer-facade`: `buffer-model` runs the same six seeded `facade_sequence`s of
  `model.cc` against the Rust facade, with DATA callbacks checked as well.
- `string-production-{random,compatibility,format,allocation,ownership,integer,split,copy}`:
  each replayed the matching `gate.py` profile (or `test_compatibility.py`)
  body, check for check, from an addon loaded beside the built extension. The
  profiles compile the same patched `string.cc` and Rust archive under
  sanitizers, and every hybrid PHPT in the PHPT gate runs the loaded extension's
  String and Buffer methods.
- `string-production-{storage,metadata,facade}` and `buffer-legacy-build`, which
  built a second extension only for the storage and facade consumers: their
  String edits, transitions and legacy facade lifecycles are the
  `string-format`/`string-split`/`string-copy`, `string-transitions` and
  `buffer-model` cases.
