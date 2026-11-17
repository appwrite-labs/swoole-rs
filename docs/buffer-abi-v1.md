# Buffer ABI v1

Types: `native/include/swoole_rs_buffer.h`. Feature value 2 (graduated: always on).

Rust owns one queue and its data allocations. Entries are Rust data, sendfile
tokens or CLOSE markers; the front cursor (offset and generation) is Rust's too. The socket
record's `Buffer` (declared in `native/buffer/swoole_buffer.h`) holds only the Rust
handle, and its methods are Rust (`crates/corebase/src/buffer/cpp.rs`), which only Rust calls.
Callers read the first entry with `typed_front()`, commit sends with
`typed_consume()`, queue a close with `typed_close()` and drop the entry with `pop()`.

## Ownership and accounting

- `Buffer::create` returns a unique queue, or `NOMEM`. Chunk size 0 means
  `INT32_MAX`. `Buffer::release` drops it, or hands it back while a token is queued.
- Append copies borrowed slices into stable Rust allocations.
- Vector offset is an unsigned byte offset into the concatenated slices.
  Empty slices are allowed. Offset equal to the total succeeds and inserts
  nothing; beyond the total is `INVALID`. Overflowing sums are `OVERFLOW`.
- Queue length counts copied Rust bytes, including already-sent bytes until the
  chunk is popped. Pop subtracts the entry's copied length. Total length cannot
  exceed `UINT32_MAX`. Tokens count zero bytes.
- The queue never dereferences, calls or frees a token or its disposer; pop hands
  both back as a `TypedDetached`, whose `dispose()` runs the disposer once, after the
  queue call returns. Data pointers survive append; pop invalidates the removed front.
- Release with pending tokens fails without mutation; otherwise it drops remaining
  data. `~Buffer` drains and disposes tokens first.

## Errors, threads, fork

Allocation is fallible. On failure no entry, view or accounting changes. No
panic, exception or callback crosses the seam. `Buffer`'s methods abort on any
failed status. Existing `void` append signatures remain and never drop bytes
silently.

One owner uses a handle at a time. Sequential transfer between threads is fine
once views are no longer used. A quiescent fork gives parent and child
independent copy-on-write queues; each releases its own. No Rust threads, locks
or global state exist.

Six bytes, chunk size 4, vector offset 3 make a chunk of capacity 4 and length
3; after pop the length is 0. Exact vector-boundary offsets select the suffix
that starts there.

## String

`CppString` and its operations are Rust (`crates/corebase/src/buffer/string.rs`).
It has Rust layout; no String member exports or shared field-layout contract remain.
`native/buffer/swoole_string.h` declares an opaque `swoole::String` because native
socket records retain pointers. They never inspect or mutate the pointed-to fields.
Rust callers use the buffer's methods and `view()`; `OwnedString` disposes allocations
owned by a scope. Oracle shims keep their separate local String records and pinned
assertions (see `tests/modules/buffer/README.md`).
