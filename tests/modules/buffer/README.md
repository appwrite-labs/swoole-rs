# Buffer and String gates

Gates for the Rust `swoole::Buffer` queue (`crates/buffer`) and `swoole::String`
(`crates/buffer/src/string.rs`, gates in [string/](string/)).
Native runners compile the upstream side from pinned upstream
`76c657d88be39b7af6bf85e46bcbac0da7387760`, extracted into a disposable tree, and
the Rust side from the headers and switches of the Cargo build whose SDK the supplied
Rust archive belongs to (`tools.build_ext.native_flags`); the String gates link that
build's native archives with the Rust archive (`native_link`). They never rebuild the
archive. The `test_*.py` gates run under pytest (`python3 -m pytest tests/modules/buffer/<file>`) with the
shared options in `tests/conftest.py` and read sources from `target/upstream/src`. `--sanitize` adds C++ ASan/UBSan; the Rust archive is not instrumented.

## Native queue gates

| Command | Checks |
| --- | --- |
| `baseline.py --sanitize` | Pinned upstream only: original `core-tests/src/memory/buffer.cpp` assertions (64 seeded runs) plus a scatter/gather matrix that records upstream defects. |
| `test_typed.py --archive A [--sanitize]` | Typed queue facade compiled and run under C++14/17/20. |

`swoole::Buffer`'s methods are Rust (`crates/buffer/src/cpp.rs`) under their upstream
symbols; `src/core/buffer.cc` is not built. Upstream's `core-tests/src/memory/buffer.cpp`
is ported there, one Rust test per `TEST`:

| Upstream `TEST` | Rust test |
| --- | --- |
| `buffer.append_iov` | `cpp::tests::append_iov` (64 seeded runs of the same lengths, offsets and checks) |

The project-written `iovec.cc` fixture is ported there too:

| Fixture | Rust test |
| --- | --- |
| `iovec.cc` (twelve positive offset-0 vectors and the `iovec` layout) | `cpp::tests::positive_iovec_vectors` |

## String gates

| Command (dev.py phase) | Checks |
| --- | --- |
| `string/test_gate.py --archive A --sanitize` (`string-originals`) | `core-tests/src/core/string.cpp` compiled unchanged under GoogleTest: all 23 TESTs on the pin; on the hybrid the two whose subject is still C++ there (`format_1`, `format_2`: `format` is a template in the header). Seven (`ltrim`, `rtrim`, `strnpos`, `strnstr`, `substr_len`, `starts_with`, `ends_with`) test inline helpers that left the hybrid's headers (#2089), so they run on the pin only |

`swoole::String`'s methods are Rust. The other fourteen TESTs are ported to
`string/originals.rs` (`cargo test -p swoole-rs-buffer`), one Rust test per `TEST`, each C++
call replaced by the Rust method that is its body:

| Upstream `TEST` | Rust test |
| --- | --- |
| `string.move_and_copy` | `string::originals::move_and_copy` |
| `string.append` | `string::originals::append` |
| `string.write` | `string::originals::write` |
| `string.repeat` | `string::originals::repeat` |
| `string.release` | `string::originals::release` |
| `string.ub` | `string::originals::ub` (without `s1 = s1`, which upstream's operator ignores and Rust's borrows cannot spell) |
| `string.explode` | `string::originals::explode` |
| `string.explode_2` | `string::originals::explode_2` |
| `string.pop_1` | `string::originals::pop_1` |
| `string.pop_2` | `string::originals::pop_2` |
| `string.reduce_1` | `string::originals::reduce_1` |
| `string.reduce_2` | `string::originals::reduce_2` |
| `string.reduce_3` | `string::originals::reduce_3` |
| `string.append_number` | `string::originals::append_number` |

The project-written `compatibility.cc` consumer is ported there too:

| Fixture | Rust test |
| --- | --- |
| `compatibility.cc` (44 checks of append, write, integer, repeat, copy, termination and reduce) | `string::originals::compatibility` |

The header declares only what C++ still calls or a fixture links: the sized constructor,
copy and move construction, the destructor, `grow`, `substr`, `extend`, `reserve`,
`release`, `repeat`, `append` (bytes and `int`), `append_random_bytes`, `write(String)`,
`set_null_terminated`, `print`, `pop`, `reduce` and the `format` templates. Assignment is
deleted; the byte-pointer constructor, `contains`, `starts_with`, `ends_with`, `equals`,
`append(String)`, `write(bytes)` and `split` have no C++ caller and no symbol: Rust uses
`CppString`'s methods and its `view()`.

Run as `python3 tests/modules/buffer/<script>`; `S` is the pinned upstream
checkout (`target/upstream/src`).

## PHP gates

- `test_network.py --engine NAME=SO ... [--php PHP]`: ordered 4 MiB streaming, server
  reported queued bytes, sendfile offset/length, keep-alive and health after a
  cancelled response.
- `lifecycle.py --engine NAME=SO ... [--php-extension EXT]`: slow readers,
  stream/sendfile cancellation, half-close, churn and pending-write shutdown
  (corpus project `11-buffer-lifecycle`).
- Original PHPTs (`originals.json`, seven Swoole v6.2.5 tests plus
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
| `format_impl`'s printf varargs, inline accessors | C++ |
