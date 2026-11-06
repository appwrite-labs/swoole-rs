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
| `test_iovec.py --archive A [--sanitize]` | Twelve positive POSIX iovec append cases; native and selected layout and observation counts must match. |
| `test_typed.py --archive A [--sanitize]` | Typed queue facade compiled and run under C++14/17/20. |

`swoole::Buffer`'s methods are Rust (`crates/buffer/src/cpp.rs`) under their upstream
symbols; `src/core/buffer.cc` is not built. Upstream's `core-tests/src/memory/buffer.cpp`
is ported there, one Rust test per `TEST`:

| Upstream `TEST` | Rust test |
| --- | --- |
| `buffer.append_iov` | `cpp::tests::append_iov` (64 seeded runs of the same lengths, offsets and checks) |

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
- `Buffer` has no `front()`, `alloc()` or `BufferChunk` objects: callers read the
  first entry with `typed_front()`, commit partial sends with `typed_consume()` and
  queue a close with `typed_close()`. Nothing in the build set a chunk's `destroy`.
- `swoole_server/sendfile_02.phpt` fails in both engines: `Swoole\Client::getSocket()`
  is missing because the build lacks `SWOOLE_SOCKETS_SUPPORT`.
- The lifecycle half-close case fails on upstream (truncated response) and on any
  hybrid built without the separate half-close fix.

## Ownership

| Responsibility | Owner |
| --- | --- |
| Queue chunks, bytes, count, offset validation, failure rollback | Rust |
| `swoole::Buffer`'s methods, sendfile tokens and their disposers | Rust, under the upstream symbols |
| POSIX iovec adaptation | Rust reads the `iovec` array in place |
| `swoole::String`'s methods (growth, edits, copy/move, search, split, random bytes) | Rust, under the upstream symbols, calling the String's own allocator |
| `split`'s `std::function` handler, `format_impl`'s printf varargs, inline accessors | C++ |

The ltrim/rtrim/strnpos/strnstr original String tests exercise native helpers in
`include/swoole_util.h` and `include/swoole.h`, not Rust code.

## Gate audit (#473)

Kept, each with what nothing else reaches:

- `string-originals` runs upstream's own `core-tests/src/core/string.cpp`, which
  no other phase builds.
- `buffer-typed`: the typed facade's only C++14/17/20 builds, under sanitizers.
- `buffer-posix-iovec`: `Buffer::append(iovec)` through the C++ ABI against
  upstream under sanitizers.
- `buffer-string-compatibility`: positive String edits and C++14/17/20 whole-TU builds.
- `buffer-network`: `Http\Response::sendfile` with an offset and a length behind
  4 MiB of queued output; no original PHPT passes a length.

Deleted:

- `buffer-native` and `buffer-callbacks` (#1361): `buffer.cc` left the build, so
  `buffer.cpp`'s assertions are the crate's `append_iov` test, and `BufferChunk`'s
  `destroy` callback, which those gates threw from, no longer exists.

- `string-{split,integer,ownership,allocation,format,random,copy,transitions}`:
  they drove the String step-machine exports, which whole Rust methods under the
  upstream symbols replaced; `string-originals` runs upstream's assertions on them.
- `buffer-facade`, `buffer-model` and `buffer-typed-layout` (#1333): they drove
  the untyped facade, a branch the shipped extension never compiled, or compared
  the layouts of the two. The crate's unit tests keep the allocation-failure
  sweeps over the one queue.
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
