# Buffer ABI v1

Declarations: `native/include/swoole_rs_buffer.h`. Feature value 2
(graduated: always on). Upstream targets: `src/core/buffer.cc`,
`include/swoole_buffer.h`, `src/core/string.cc`, `include/swoole_string.h`.

Rust owns one queue and its data allocations. Entries are either Rust data or
opaque native tokens. The C++ `Buffer` facade keeps one embedded, borrowed
`BufferChunk` view of the current Rust data front; native entries keep their
existing C++ `BufferChunk` pointer. There is no second queue or side map.
Socket/reactor decisions and sendfile callbacks remain upstream.

## Ownership and accounting

- Create writes a unique handle, or null on failure. Chunk size 0 means
  `INT32_MAX`. Each non-null handle is released exactly once.
- Append copies borrowed input into stable Rust allocations. Inputs, data and
  outputs must be valid, aligned where typed, and disjoint from the handle,
  queued data and each other. Byte buffers need only byte alignment. Null input
  is allowed for zero length/count.
- Vector offset is an unsigned byte offset into the concatenated descriptors.
  Empty descriptors are allowed. Offset equal to the total succeeds and inserts
  nothing; beyond the total is `INVALID`. The facade rejects negative `off_t`.
  Overflowing sums are `OVERFLOW`; lengths above `isize::MAX` are rejected.
- Queue length counts copied Rust bytes, including already-sent bytes until the
  chunk is popped. Pop subtracts the entry's copied length, not its capacity.
  Total length cannot exceed `UINT32_MAX`. Native tokens count zero bytes.
- Push-native takes a unique non-null token only on success. Rust never
  dereferences or frees it. Front returns kind 1 and the token; pop returns it
  through the mandatory output pointer. A data pop writes null.
- Front on an empty queue returns `EMPTY` and a zeroed view. Data views have
  kind 0, length = capacity = copied bytes, a borrowed writable pointer and a
  null token. Pointers survive append; pop invalidates the removed front.
- The facade's cached front identity and offset survive repeated `front()` and
  append; pop resets them. A destroy callback on a cached Rust DATA view runs in
  C++ before the Rust pop, while the bytes are alive. Native token disposal runs
  after detach. No callback runs during a Rust call or may reenter the Buffer.
- `release(NULL)` succeeds. Release with pending native tokens fails without
  mutation; otherwise it drops remaining data and the handle. Native teardown
  drains and disposes tokens first. Empty pop writes null and returns `EMPTY`.
  Null required outputs return `INVALID` without being dereferenced. Failed
  front/create/pop zero their output; failed stats leaves outputs unchanged.

## Errors, threads, fork

Allocation is fallible. On failure no entry, view or accounting changes. No
panic, exception or callback crosses the seam. The facade maps `NOMEM` to
`std::bad_alloc`, `OVERFLOW` to `std::length_error` and `INVALID` to
`std::invalid_argument` after the Rust call returns. Existing `void` append
signatures remain and never drop bytes silently. `alloc(TYPE_DATA, size)` stays
a native-owned token path.

One owner uses a handle at a time. Sequential transfer between threads is fine
once views are no longer used. A quiescent fork gives parent and child
independent copy-on-write queues; each releases its own. No Rust threads, locks
or global state exist.

## Deliberate differences from upstream

Pinned upstream underflows: six bytes, chunk size 4, vector offset 3 creates a
chunk of capacity 4 and length 3; pop then reports length `UINT32_MAX`. Rust
reports 0. Exact vector-boundary offsets also select the correct suffix.

## Facade call costs

`Buffer::pop()` does not materialize an unread front: it runs a cached DATA
view's destroy callback if present, calls Rust pop, then invalidates the cache
and deletes any returned native token. A four-chunk append-then-drain cycle
makes 10 ABI calls (1 append, 5 stats, 4 pop) without front reads and 14 with
them. `Buffer::append(const char *, uint32_t)` goes through the same
`swr_v1_buffer_append` → generic `appendv` path as vector input; the generic
path is not specialized for one descriptor.

## String

`swoole::String` is defined in Rust (`crates/buffer/src/string.rs`): the C++ class
keeps its fields, which Rust mirrors with layout asserts on both sides, and Rust
defines every method with a body under its upstream symbol. `String::split` stays a
three-line C++ method that lends its `std::function` handler to Rust.

## Typed mode

`SWOOLE_RS_BUFFER_TYPED`, which configure defines whenever both the Rust Buffer
and the Rust reactor are built, removes the facade's embedded cached
`rs_front_` chunk and flag, leaving only the Rust owner pointer. Without it the
layout above is unchanged. Public `BufferChunk` and native CLOSE/sendfile objects
are unchanged; legacy mutable DATA front/allocation is refused in typed mode.
External code that mutates Buffer internals is supported only without typed mode.
The Rust owner allocation is larger even with typed mode off (Rust Buffer without
the Rust reactor).
