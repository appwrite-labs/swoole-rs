# Memory and collection ABI

Module-local ABI v1. `swoole_rs_memory.h` declares the GlobalMemory functions C++
calls; the rest are Rust functions its crates call by path. Supported
hosts are 64-bit macOS and Linux.

## General rules

- Pointers and borrowed ranges must be valid for their documented size and
  lifetime. NULL handles are rejected; dangling pointers cannot be detected.
- Mutable state/output must be exclusive and disjoint from borrowed input/output.
  No borrowed Rust reference survives a call.
- Rust allocations are fallible; no caller-controlled size causes an infallible
  allocation or unchecked wrap.
- Rust-allocated heap nodes/queues are released only through their Rust owner.
  Native tokens/data are never freed or dereferenced by Rust.
- Invalid external node positions return failure before indexing.
- Defensive rejection of invalid/overflowing scalar input is not equivalence with
  undefined original behavior.

## Fixed pool

Rust only (`crates/memory/src/fixed.rs`): `FixedPool` and the table's row pool.
The 48-byte header and 24-byte slice headers keep the pinned layout and live in
the arena, so a shared pool stays visible across fork. Callers serialize.

- `Layout::new` plans an owned arena (at least two slices, width rounded to 8) or
  a pool over borrowed bytes (width kept, odd included, complete slices floored).
  Invalid or overflowing plans are `None`.
- `construct` zeroes the header, padding included, and links the slices; payload
  and trailing bytes are untouched. A misaligned arena is refused untouched.
- alloc returns the last idle slice's payload, NULL when exhausted; free returns
  false without mutation for a bad range or stride, a duplicate free or empty
  accounting. The `FixedPool` members keep upstream's assertions and last error.
- Slice headers may be unaligned (odd table widths): raw unaligned access only.
  alloc/free publish only head/tail/slice_use through raw field addresses.
- `debug_step` walks the live links for `FixedPool::debug`: negative max prints 1
  row, max 0 prints 2, larger limits stop after max+2 or list end.

## Heap

Rust only. Rust owns a vector of stable, individually allocated node PODs with
upstream's `HeapNode` fields and 1-based position.

- create takes min/max as exactly 0/1, zeroes output first, returns 0/-1.
- push returns a stable node or NULL; on OOM logical contents are unchanged
  (capacity may grow).
- count/at do not mutate or allocate. update/remove return 0/-1 and do
  not allocate. Call them only on live nodes from the same heap.
- pop returns the data token or NULL when empty (NULL is also a valid token).
- remove/pop use swap-last and strict priority comparison with the pinned
  left-child tie choice; equal priorities give the original 1/3/2 three-node order.
  No FIFO guarantee.
- peek/at views stay valid until that node is popped/removed or the heap is
  destroyed. Callers must not free nodes. Destruction frees nodes, not their data
  tokens.
- The heap retains at most one removed 24-byte node for reuse by a later push; a
  second removal frees the previous spare. Vector growth is checked before taking
  the spare.

## Core channel

`channel::Record` is the whole record of upstream's `swoole::Channel` (the
server's message box is its one user): the queue state, a
process-shared mutex and the notification pipe (read end first). The owner allocates `channel_bytes(size,maxlen)` bytes (the record
plus the ring and its maxlen+4 overflow space) from the heap or, for
`SW_CHAN_SHM`, a shared mapping, and frees them after `channel_close`. Rust owns
the payload and unaligned 32-bit length coding, head/tail wrap tags, message/byte
accounting, the mutex and the pipe.

- `channel_bytes` returns 0 for `size < maxlen`, `maxlen > INT32_MAX` or overflow.
- `channel_init(record,size,maxlen,flags)`: 0, -1 invalid, 1 `pipe()` failed
  (errno intact), 2 mutex failed; nothing to release on failure. Flag values
  2/4/8 are upstream's `SW_CHAN_LOCK`/`NOTIFY`/`SHM`; other bits are ignored.
- in returns 0/-1; out returns length/-1. Zero-length messages are valid.
- consume=0 peeks without mutation; consume=1 pops.
- Too-short output, invalid lengths and wrap tags outside 0/1 fail without
  mutating state, storage or output.
- Partial wrap uses the pinned overflow-space behavior.
- `channel_lock(record,lock)` locks or unlocks the mutex; push, pop and peek take
  it around in/out as upstream. Unlocked in/out require caller serialization.
- `channel_signal(record,notify)`: notify writes one 8-byte token, wait reads one
  (blocking, retrying `EINTR`); 0 or -1.
- `channel_print` writes upstream's text through C stdio.
- `channel_full(state)`: 40-byte snapshot of head/tail, size/bytes, count and tags;
  returns 0/1, -1 for NULL. Uses explicit unsigned wrapping arithmetic.

## Global memory pages

Rust only (`crates/memory/src/global.rs`): `GlobalMemory`'s pages, under its lock.
Pages come from `sw_shm_malloc` (shared) or `sw_malloc`, rounded up to whole
system pages, and are freed when the pool is.

- alloc writes the 8-byte block header size and returns a zeroed, 8-aligned
  payload; size 0 still consumes a header. A full page starts a new one. A block
  that can't fit a page is refused with upstream's warning.
- Fork: a shared GlobalMemory used in a different PID starts a new pool, which
  keeps the inherited one: its pages and pointers stay valid until the pool is
  deleted, parent offsets unchanged. `swoole_fork` relies on this.
  Arbitrary multithreaded-fork mutex state is not addressed.
- Huge sizes are rejected rather than reproducing upstream's u32 alignment wrap.

## Ring buffer

Rust only (`crates/memory/src/ring.rs`). The 32-byte shared arena header keeps
the pinned layout (release counter at offset 28). `layout` rounds a request to 8
bytes, at least 48 with the header. Record header: u16 lock, u16 index, u32 length.

- Single producer allocates/collects. Multiple threads/processes may release
  distinct records.
- Release counter and record-lock words use aligned Rust atomics; Rust never holds
  a reference to the whole shared header. Release stores the record lock (load
  then release-store) before the SeqCst counter increment; the producer
  acquire-loads before reading released lengths. The collection loop's counter
  load is relaxed (a scheduling bound); per-record acquire loads remain required.
- Caller byte access must finish before release; each pointer is released exactly
  once. Sequential duplicate release fails without changing the counter.
- Rust owns alignment, wrap padding, collection and index/count rollover. The
  original shrinking-bound collection loop is kept so offsets and failure timing
  match. alloc returns NULL for zero/overflow/insufficient space (the member
  asserts size>0); free returns false (the member asserts).
- `storage_alloc` reads the producer fields into a cursor and publishes them back,
  even when alloc returns NULL.

## Shared mapping

`sw_shm_*` (`members.rs`): an anonymous `MAP_SHARED` mapping whose first 8 bytes
hold its total size; the payload follows. A zero-byte allocation still reserves
the header; overflowing sizes return NULL. Reallocation copies
min(old_total-8, new_total-8) payload bytes.

## Upstream members

`crates/memory/src/members.rs` holds upstream's memory members. With
`upstream-symbols`, `GlobalMemory`'s out-of-line members and `sw_shm_malloc`,
`sw_shm_calloc`, `sw_shm_free` and `sw_shm_protect` are under their upstream C++
symbols. `native/memory/swoole_memory.h` (upstream's is deleted) keeps `GlobalMemory`'s constructor (it throws on the
`swr_v1_memory_global_memory_new` status: -1 `Exception(SW_ERROR_INVALID_PARAMS)`,
-2 `std::bad_alloc`) and its inline members, for core-tests. It has no vtable: its one
member is the `impl` slot, and `SwooleG.memory_pool` is Rust's (`members::pool::new`,
`swoole_init`'s; `delete`, `swoole_clean`'s).
`FixedPool`, `RingBuffer` and `sw_shm_realloc` have no C++ caller left; their members
stay Rust-only for upstream's GoogleTests, ported to `tests/modules/memory/`.
`swr_table_make` allocates through `members::pool::alloc`; the pool's `free` is
a no-op, so nothing calls it. These run allocators, mmap and logs with a Rust frame active.

## Shared tables

Rust owns `swoole::Table` (crates/memory/src/table.rs), and `Swoole\Table`
(crates/phpcollections) is its only user; no C++ view is left. Two blocks stay
shared across `fork()` as upstream's are: the header (`size_of::<Table>()` bytes from
`sw_mem_pool()`, holding sizes, counters and the pool and iterator mutexes) and, at
create, the arena (`arena_size()` bytes from `sw_shm_malloc`). `Table::destroy`
frees the rest and returns the arena for the caller to free with the header.

- Rows: 88-byte header (next at 16, key at 24) and the
  column data. Rows are padded to 8 bytes in the arena so lock words and links are
  aligned; `calc_memory_size`/`get_memory_size` and the slice counts still report
  upstream's unpadded accounting.
- Row locks are upstream's spin lock: spin when there is more than one CPU, take
  over a lock whose owner is gone (`kill(pid, 0)` and `ESRCH`) or held for more
  than 2000 ms, and log upstream's warning through the host's warning callback.
- Keys are cut to 63 bytes and hashed by the host's callback (`swoole_hash_php`,
  or Zend's string hash for `Swoole\Table`). `set` returns null with the bucket
  still locked when the collision pool is exhausted, after counting the
  conflict, and reports `SW_ERROR_MALLOC_FAIL` through the host's error callback.
  Deleting a bucket's own row moves its first collision row in.
- Columns are process-private (copied on fork), as upstream's maps were.
  Duplicate names keep the first declaration for lookup; every declaration
  takes row space. Columns cannot be added once the table is created.
- The iterator's cursor and row copy are process-private; its mutex is shared.
