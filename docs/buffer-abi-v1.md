# Buffer ABI v1

Declarations: `native/include/swoole_rs_buffer.h` and
`native/buffer/swoole_rs_buffer_typed.h`. Feature value 2 (graduated: always on).
Upstream targets: `src/core/buffer.cc`, `include/swoole_buffer.h`,
`src/core/string.cc`, `include/swoole_string.h`.

Rust owns one queue and its data allocations. Entries are Rust data, sendfile
tokens or CLOSE markers; the front cursor (offset and generation) is Rust's too
([typed-queue.md](../native/buffer/typed-queue.md)). The C++ `Buffer` holds the
Rust handle and one borrowed `BufferChunk` view that `front()` fills from the
current front. There is no second queue or side map.

## Ownership and accounting

- `swr_v1_buffer_typed_create` writes a unique handle, or null on failure. Chunk
  size 0 means `INT32_MAX`. Each non-null handle is released exactly once.
- Append copies borrowed input into stable Rust allocations. Inputs, data and
  outputs must be valid, aligned where typed, and disjoint from the handle,
  queued data and each other. Byte buffers need only byte alignment. Null input
  is allowed for zero length/count.
- Vector offset is an unsigned byte offset into the concatenated descriptors.
  Empty descriptors are allowed. Offset equal to the total succeeds and inserts
  nothing; beyond the total is `INVALID`. The facade rejects negative `off_t`.
  Overflowing sums are `OVERFLOW`; lengths above `isize::MAX` are rejected.
- Queue length counts copied Rust bytes, including already-sent bytes until the
  chunk is popped. Pop subtracts the entry's copied length. Total length cannot
  exceed `UINT32_MAX`. Tokens count zero bytes.
- Rust never dereferences, calls or frees a token or its disposer; pop hands both
  back and C++ disposes after Rust returns. Data pointers survive append; pop
  invalidates the removed front.
- `release(NULL)` succeeds. Release with pending tokens fails without mutation;
  otherwise it drops remaining data and the handle. C++ teardown drains and
  disposes tokens first. Null required outputs return `INVALID` without being
  dereferenced. Failed front/create/pop zero their output; failed stats leaves
  outputs unchanged.

## Errors, threads, fork

Allocation is fallible. On failure no entry, view or accounting changes. No
panic, exception or callback crosses the seam. The facade maps `NOMEM` to
`std::bad_alloc`, `OVERFLOW` to `std::length_error` and `INVALID` to
`std::invalid_argument` after the Rust call returns. Existing `void` append
signatures remain and never drop bytes silently.

One owner uses a handle at a time. Sequential transfer between threads is fine
once views are no longer used. A quiescent fork gives parent and child
independent copy-on-write queues; each releases its own. No Rust threads, locks
or global state exist.

## Upstream's `front()`, `pop()` and `alloc()`

`front()` returns the borrowed view of the current entry (null when empty): DATA
with `value.str`, `length` = `size` = copied bytes and `offset` = the Rust cursor;
a sendfile token in `value.ptr`; or, for a CLOSE pushed by `alloc`, that chunk
itself. `pop()` detaches one entry, first running (noexcept, on live bytes) a
`destroy` the caller set on the view. `alloc()` takes `TYPE_CLOSE` with size 0 and
queues a real `BufferChunk` with a native disposer.

Deliberate differences: queued DATA is immutable, so writing the view's `offset`
does nothing (partial sends commit through `typed_consume`), and
`alloc` refuses `TYPE_DATA` and `TYPE_SENDFILE` (sendfile goes through
`typed_sendfile`). No caller in the build does either.

Pinned upstream underflows: six bytes, chunk size 4, vector offset 3 creates a
chunk of capacity 4 and length 3; pop then reports length `UINT32_MAX`. Rust
reports 0. Exact vector-boundary offsets also select the correct suffix.

## String

`swoole::String` is defined in Rust (`crates/buffer/src/string.rs`): the C++ class
keeps its fields, which Rust mirrors with layout asserts on both sides, and Rust
defines every method with a body under its upstream symbol. `String::split` stays a
three-line C++ method that lends its `std::function` handler to Rust.
