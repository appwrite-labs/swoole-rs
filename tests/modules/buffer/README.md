# Buffer and String gates

Gates for the Rust `swoole::Buffer` queue (`crates/corebase/src/buffer`) and `swoole::String`
(`crates/corebase/src/buffer/string.rs`, gates in [string/](string/)).
The native reference gates compile pinned upstream
`e0d606594e13cecd7790baafc5f1e1ead816b77c`, extracted into a disposable tree.
The String gate runs only that oracle; the runtime's original assertions are
Rust tests in `string/originals.rs`. The `test_*.py` gates run under pytest with
the shared options in `tests/conftest.py` and read `target/upstream/src`.
`--sanitize` adds C++ ASan/UBSan to the reference build.

## Native queue gates

| Command | Checks |
| --- | --- |
| `baseline.py --sanitize` | Pinned upstream only: original `core-tests/src/memory/buffer.cpp` assertions (64 seeded runs) plus a scatter/gather matrix. |

`swoole::Buffer`'s methods are Rust (`crates/corebase/src/buffer/cpp.rs`); no C++ calls them, so
none has a symbol, and `src/core/buffer.cc` is not built. Upstream's
`core-tests/src/memory/buffer.cpp` is ported there, one Rust test per `TEST`:

| Upstream `TEST` | Rust test |
| --- | --- |
| `buffer.append_iov` | `cpp::tests::append_iov` (64 seeded runs of the same lengths, offsets and checks) |

The project-written fixtures are ported there too, with the same inputs and checks:

| Fixture | Rust test |
| --- | --- |
| `iovec.cc` (twelve positive offset-0 vectors and the `iovec` layout) | `cpp::tests::positive_iovec_vectors` |
| `typed.cc` (the DATA cursor and a CLOSE marker at chunk sizes 1, 3, 16 and 0, and the front's layout; its C++14/17/20 builds went with the C++ caller) | `cpp::tests::typed_queue_facade` |

## String gates

| Command (dev.py phase) | Checks |
| --- | --- |
| `string/test_gate.py --sanitize` (`string-originals`) | `core-tests/src/core/string.cpp` compiled unchanged under GoogleTest: all 23 TESTs, on the pin only. Seven (`ltrim`, `rtrim`, `strnpos`, `strnstr`, `substr_len`, `starts_with`, `ends_with`) test inline helpers that left the hybrid's headers (#2089); `format_1` and `format_2` test the `format` templates, which left `swoole_string.h` once no compiled C++ instantiated them (#4310) |

`swoole::String`'s methods are Rust. The other fourteen TESTs are ported to
`string/originals.rs` (`cargo test -p swoole-rs-corebase`), one Rust test per `TEST`, each C++
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

`thread_buffer.rs` (`cargo test -p swoole-rs-corebase`) checks that a thread
`swoole_thread_init` never set up gets `None` from `try_with_thread_buffer`, not a
null String.

The hooks, client, HTTP and framing Rust case fixtures use the real `StringBuffer`
for borrowed runtime buffers and `OwnedString` for allocations they own. They no
longer copy the record layout or call the sized constructor, destructor,
`append_random_bytes` or `pop` through Itanium symbols; those four exports are gone.
The original test inputs and assertions are unchanged. The remaining eleven
String member exports and their unused wrappers are gone too; helpers called by
the Rust original tests use Rust's calling convention. `StringBuffer` has Rust layout.
`native/buffer/swoole_string.h` only declares an opaque class: socket and co-socket
records retain pointers, and no compiled native consumer reads its fields.

The oracle's framing, HTTP-frame, HTTP-proxy and RESP shims define their own
String records over pinned bodies. In particular, both framing-stream engines
use the local record in `crates/oracle/shim/framing_stream.cc`, including its
candidate ABI assertions; neither lends a runtime `StringBuffer` to that ABI.
Those oracle layouts and assertions remain unchanged.

Run as `python3 tests/modules/buffer/<script>`; `S` is the pinned upstream
checkout (`target/upstream/src`).

## PHP gates

- `test_network.py --engine NAME=SO ... [--php PHP]`: ordered 4 MiB streaming, server
  reported queued bytes, sendfile offset/length, keep-alive and health after a
  cancelled response.
- `lifecycle.py --engine NAME=SO ... [--php-extension EXT]`: slow readers,
  stream/sendfile cancellation, half-close, churn and pending-write shutdown
  (corpus project `11-buffer-lifecycle`).
- Original PHPTs (`originals.json`, seven Swoole v6.3.0 tests plus
  `examples/test.jpg`):
  `python3 corpus/upstream/run.py --manifest tests/modules/buffer/originals.json --source S --engine upstream=SO --engine hybrid=SO --require-all --isolate-cases`.
  On Linux PHP 8.2 add `--php-extension curl --php-extension sockets` for both engines.

## Known failures and intentional differences

- Rust handles empty `Buffer::append(iovec)` vectors and offsets at or beyond
  the total length explicitly; those are not run against upstream.
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
| `swoole::Buffer`'s methods, sendfile tokens and their disposers | Rust |
| `swoole::String`'s methods (growth, edits, copy/move, search, split, random bytes) | Rust, calling the String's own allocator |

`OwnedString` wraps the Rust buffer with its existing disposal path.
`popped_owner_keeps_the_prefix_and_frees_each_allocation_once` checks an exact
13-byte allocation, an independently retained unread tail, prefix truncation and
one allocator release for each owner. The pointer-returning Rust pop still transfers its block.
