# Memory and collection ABI

Module-local ABI v1: Rust functions its crates call by path, and the `sw_shm_*`
upstream symbols below. Supported hosts are 64-bit macOS and Linux.

## General rules

- Pointers and borrowed ranges must be valid for their documented size and
  lifetime. NULL handles are rejected; dangling pointers cannot be detected.
- Mutable state/output must be exclusive and disjoint from borrowed input/output.
  No borrowed Rust reference survives a call.
- Rust allocations are fallible; no caller-controlled size causes an infallible
  allocation or unchecked wrap.
- Defensive rejection of invalid/overflowing scalar input is not equivalence with
  undefined original behavior.

## Fixed pool

Rust only (`crates/memory/src/fixed.rs`): the table's row pool. `FixedPool`, the
owner of an arena of its own, is test support (`tests/modules/memory/members.rs`).
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
- `debug_step` (test support, with `FixedPool`) walks the live links for
  `FixedPool::debug`: negative max prints 1 row, max 0 prints 2, larger limits stop
  after max+2 or list end.

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
- `Record::push`, `pop` (consume) and peek take the record's mutex around in/out
  as upstream, with `SW_CHAN_LOCK`; without it a `Record` stays on one thread.
  `print` reads the queue under the same mutex.
- `Record::signal(notify)`: notify writes one 8-byte token, wait reads one
  (blocking, retrying `EINTR`); 0 or -1.
- `Record::print` writes upstream's text through C stdio.
- `channel_full(state)`: 40-byte snapshot of head/tail, size/bytes, count and tags;
  returns 0/1, -1 for NULL. Uses explicit unsigned wrapping arithmetic.

## Global memory pages

Rust only (`crates/memory/src/global.rs`): `GlobalMemory`'s pages, under its lock.
Pages are anonymous mappings, `MAP_SHARED` (shared, as `sw_shm_malloc`'s) or
`MAP_PRIVATE` (which a fork copies, as it would `sw_malloc`'s memory), rounded up to
whole system pages, and are unmapped when the pool is.

- alloc writes the 8-byte block header size and returns a zeroed, 8-aligned
  payload: a mapping starts zeroed and no block is handed out twice, so nothing
  zeroes it again. Size 0 still consumes a header. A full page starts a new one. A block
  that can't fit a page is refused with upstream's warning.
- Fork: a shared GlobalMemory used in a different PID starts a new pool, which
  keeps the inherited one: its pages and pointers stay valid until the pool is
  deleted, parent offsets unchanged. `swoole_fork` relies on this.
  Arbitrary multithreaded-fork mutex state is not addressed.
- Huge sizes are rejected rather than reproducing upstream's u32 alignment wrap.

## Shared mapping

`sw_shm_*` (`members.rs`): an anonymous `MAP_SHARED` mapping whose first 8 bytes
hold its total size; the payload follows. A zero-byte allocation still reserves
the header; overflowing sizes return NULL.

## Upstream members

`crates/memory/src/members.rs` holds upstream's memory members. With
`upstream-symbols`, `sw_shm_malloc`, `sw_shm_calloc` and `sw_shm_free` are under
their upstream C++ symbols (`sw_shm_protect` is `Shm::protect`: nothing native called
it). Upstream's `swoole_memory.h`
is deleted and no C++ is left over `GlobalMemory`: `members::GlobalMemory` is a Rust
owner whose constructor returns upstream's throws as errors (-1
`Exception(SW_ERROR_INVALID_PARAMS)`, -2 `std::bad_alloc`).
`SwooleG.memory_pool` is the `GlobalMemory` in `members::pool` (`init`,
`swoole_init`'s; `delete`, `swoole_clean`'s), whose `alloc` aborts where upstream's
throws. `FixedPool` and `GlobalMemory` have no C++ caller left: `FixedPool` lives
with upstream's GoogleTests, ported to `tests/modules/memory/`, and `GlobalMemory`'s
unshared flavour serves only them.
`swr_table_make` allocates through `members::pool::alloc`; the pool's `free` is
a no-op, so nothing calls it. These run allocators, mmap and logs with a Rust frame active.

## Shared tables

Rust owns `swoole::Table` (crates/memory/src/table.rs), and `Swoole\Table`
(crates/phpcollections) is its only user; no C++ view is left. Two blocks stay
shared across `fork()` as upstream's are: the header (`size_of::<Table>()` bytes from
`members::pool::alloc`, holding sizes, counters and the pool and iterator mutexes) and, at
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
- Each process owns its tables' private parts (columns and cursor) in one list; the
  shared header only points at them. `Table::destroy` drops the calling process's and
  `table::release` (MSHUTDOWN) every one left, so a process never frees through a
  pointer another process wrote into the header.
