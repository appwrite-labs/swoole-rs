# Buffer ABI v1

Feature value 2 is always enabled. Native socket headers forward-declare
`swoole::Buffer`. Rust owns the queue, its allocations and its front cursor.
`OutputQueue` lends the queue to Rust send operations in
`crates/corebase/src/buffer/cpp.rs`.

## Ownership and accounting

- `Buffer::new` creates an empty queue. A chunk size of zero selects `INT32_MAX`.
- Append copies borrowed slices into stable Rust allocations.
- Vector offset is an unsigned byte offset into the concatenated slices.
  Empty slices are allowed. Offset equal to the total succeeds and inserts
  nothing; beyond the total is `INVALID`. Overflowing sums are `OVERFLOW`.
- Queue length counts copied Rust bytes, including already-sent bytes until the
  chunk is popped. Pop subtracts the entry's copied length. Total length cannot
  exceed `UINT32_MAX`. Tokens count zero bytes.
- The queue never dereferences, calls or frees a token or its disposer; pop hands
  both back as a `Detached`, whose `dispose()` runs the disposer once, after the
  queue call returns. Data pointers survive append; pop invalidates the removed front.
- Dropping an `OutputQueue` drains its resident queue and disposes each detached
  entry. A borrower that has taken the queue owns its disposal.

## Errors, threads, fork

Queue operations return Rust errors. The socket wrapper checks append results
and treats failure as fatal.

One owner uses a handle at a time. Sequential transfer between threads is fine
once views are no longer used. A quiescent fork gives parent and child
independent copy-on-write queues; each releases its own. No Rust threads, locks
or global state exist.

Six bytes, chunk size 4, vector offset 3 make a chunk of capacity 4 and length
3; after pop the length is 0. Exact vector-boundary offsets select the suffix
that starts there.

## String

`StringBuffer` and its operations are Rust (`crates/corebase/src/buffer/string.rs`).
It has Rust layout; no String member exports or shared field-layout contract remain.
`native/buffer/swoole_string.h` declares an opaque `swoole::String` because native
socket records retain pointers. They never inspect or mutate the pointed-to fields.
Rust callers use the buffer's methods and `view()`; `OwnedString` disposes allocations
owned by a scope. Oracle shims keep their separate local String records and pinned
assertions (see `tests/modules/buffer/README.md`).
