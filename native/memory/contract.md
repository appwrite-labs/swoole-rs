# Memory and collection ABI

Module-local ABI v1, declared callback-free in `swoole_rs_memory.h`. Supported
hosts are 64-bit macOS and Linux.

## General rules

- No native allocator, lock, exception, PHP/Zend operation or syscall runs with a
  Rust frame active. Native work runs before/after Rust calls on the owning thread.
- Pointers and borrowed ranges must be valid for their documented size and
  lifetime. NULL handles are rejected; dangling pointers cannot be detected.
- Mutable state/output must be exclusive and disjoint from borrowed input/output.
  No borrowed Rust reference survives a call.
- Rust allocations are fallible; no caller-controlled size causes an infallible
  allocation or unchecked wrap.
- Rust-allocated heap nodes/queues are released only through their Rust owner.
  Native tokens/data are never freed or dereferenced by Rust. No callback or
  destructor crosses Rust.
- Invalid external node positions return failure before indexing.
- Defensive rejection of invalid/overflowing scalar input is not equivalence with
  undefined original behavior.

## Fixed pool

`swr_v1_memory_fixed` is a borrowed C POD matching the pinned private arena header:
48 bytes, with 24-byte slice headers. Native code allocates/frees the arena; Rust
owns initialization, idle/busy linked-list mutation, usage and free/reuse order.

- alloc returns the last idle slice's payload; exhaustion returns NULL and native
  code sets the original last error. alloc(size) ignores `size` (width is fixed at
  construction).
- init returns 0/-1. free returns 0/-1 without mutation for invalid range/stride,
  duplicate free or empty accounting. Native facades keep the original assertion
  on free failure.
- At least two complete slices are required. Payload width may be odd (table
  rows), so slice headers use unaligned raw loads/stores; never form Rust
  references to them. The state header is naturally aligned.
- Shared arenas contain no Rust allocator handle, Vec, lock or registry; state and
  links live in the native shared mapping and stay visible across fork. Callers
  serialize externally.
- Tail fragments smaller than a slice are not initialized as nodes (upstream's
  out-of-range partial-slice behavior is not reproduced).
- alloc/free copy the POD into local state and publish only head/tail/slice_use
  through raw field addresses. Immutable total/width getters read fields directly.

`fixed_layout(count,width,available,owned,out)`: owned mode (1) requires count>=2,
available=0, rounds width to 8 with checked u32 arithmetic, and returns
total/usable/slices/width. Borrowed mode (0) requires count=0, keeps the supplied
width (including odd), subtracts the 48-byte header and floors complete 24+width
slices within `available`. At least two slices and an isize-representable arena are
required. Invalid plans zero the 24-byte output and return -1.

`fixed_construct(arena,plan,shared,owned)` validates the plan, flags and header
alignment/extent, then initializes the 48-byte header and slice links. The arena
must be exclusively borrowed for its planned extent and disjoint from the plan.
Payload and trailing bytes are untouched. Owned construction frees a supplied
allocation if Rust rejects it; borrowed construction never takes custody. See
[fixed-construction.md](fixed-construction.md).

`fixed_usage` writes a 12-byte spare/total/width snapshot (spare subtraction uses
defined unsigned wrap). `fixed_debug_step` uses a zero-initialized 16-byte cursor
and 48-byte record, reading links unaligned. Print limit matches pinned behavior:
negative max prints 1 row, max 0 prints 2, larger limits stop after max+2 or list
end. Native performs the printf.

## Heap

Rust owns a vector of stable, individually allocated node PODs. Native `HeapNode`
fields and 1-based position keep their pinned layout. The private C++ `Heap`
layout becomes one opaque pointer, so a full native rebuild is required; old
objects cannot be mixed in.

- create takes min/max as exactly 0/1, zeroes output first, returns 0/-1.
- push returns a stable node or NULL; on OOM logical contents are unchanged
  (capacity may grow).
- count/at/compare do not mutate or allocate. update/remove return 0/-1 and do
  not allocate. Call them only on live nodes from the same heap.
- pop returns the data token or NULL when empty (NULL is also a valid token).
- remove/pop use swap-last and strict priority comparison with the pinned
  left-child tie choice; equal priorities give the original 1/3/2 three-node order.
  No FIFO guarantee.
- peek/at views stay valid until that node is popped/removed or the heap is
  destroyed. Native callers must not delete nodes. Destruction frees nodes, not
  their data tokens.
- The heap retains at most one removed 24-byte node for reuse by a later push; a
  second removal frees the previous spare. Vector growth is checked before taking
  the spare.
- `heap_debug_step` copies one node to a frozen node POD and advances a cursor;
  native code prints.

## Core channel

Snapshot API: a borrowed, independent 64-byte POD of configuration and cursors.
The C++ Channel, lock and pipe never cross Rust. Storage is the logical ring plus
the original maxlen+4 overflow space. Rust owns payload and unaligned 32-bit length
coding, head/tail wrap tags and message/byte accounting.

- in returns 0/-1; out returns length/-1. Zero-length messages are valid.
- consume=0 peeks without mutation; consume=1 pops.
- Too-short output, invalid lengths and wrap tags outside 0/1 fail without
  mutating state, storage or output.
- Partial wrap uses the pinned overflow-space behavior.
- Thread/process concurrency requires native locks; unlocked in/out require caller
  serialization as upstream.

Scalar field storage (additive):

- `channel_storage_plan(size,maxlen,flags,header_size,out)`: 32-byte plan with
  checked header + ring + overflow length. Native passes `sizeof(Channel)`.
  Invalid plans zero output and return -1.
- `channel_storage_init(fields,plan,payload)`: writes all eleven scalar slots via
  an 88-byte descriptor of typed field addresses (aligned, disjoint, exclusive).
  Payload extent must be size+maxlen+4. Native keeps the original flat aggregate
  and public header layout; Lock/Pipe never enter Rust.
- `channel_storage_in/out`: load scalars, publish only producer (tail/tag) or
  consumer (head/tag) cursors plus count/bytes. No allocation. Const peek uses the
  snapshot API.
- `channel_lifecycle(flags,phase,out)`: 20-byte action plan. Phase 0: heap/shared
  allocation, scalar init, optional lock, optional notification pipe. Phase 1:
  optional lock delete, optional pipe close/delete, heap/shared release. Actions
  1..9 are in the header; flag values 2/4/8 are compile-time checked; other bits
  ignored. Invalid phase zeroes output and returns -1. Native RAII owns
  construction rollback (pipe/lock destroyed before storage is freed).
- `channel_full(state)`: 40-byte snapshot of head/tail, size/bytes, count and tags;
  returns 0/1, -1 for NULL. Uses explicit unsigned wrapping arithmetic. The public
  header forwards `full()` to it without changing fields or signatures.

## Global memory pages

An opaque Rust registry owns page addresses, offset, drain state, page size,
shared mode and creator identity. mmap/malloc/free, mutex, getpid and wrapper
lifetime stay native.

- `page_size` rounds a request >=4096 up to a power-of-two system page size, with
  checked u32 overflow and zeroed output on failure.
- `request_valid` returns 0/-1 and supplies the rounded 8-byte payload size for the
  native warning.
- create returns an empty registry or -1 with NULL output. The additive
  `global_open` returns -2 invalid configuration, -1 allocation failure, 0 success.
- add_page registers one disjoint page: 0 success, -1 failure without custody
  (native frees the page).
- alloc returns 0 with zeroed aligned payload, 1/NULL when a page is needed, -1/NULL
  for an invalid request. It writes the 8-byte block header size, advances by
  header+aligned payload, and does not allocate. size=0 still consumes a header.
  Native allocates a page after NEED_PAGE and retries.
- free(ptr) is a no-op. pop-page returns pages in creation order and irreversibly
  enters drain mode (also when empty); no alloc/add afterwards. Release succeeds
  only after all pages were returned and never frees native pages.
- `global_step` with a 24-byte `GlobalEffect` (page token, page size, aligned
  request, action): phase 0 selects heap/shared allocation, phase 1 validates the
  request and selects shared creator-mismatch reset, phase 2 drains a page and
  selects its release. Action 0 is no effect. Calls are serialized by the owner.
- Fork: a shared GlobalMemory alloc in a different PID starts a new private
  registry/page set; inherited pages and pointers stay valid until exit, parent
  offsets unchanged. Arbitrary multithreaded-fork mutex state is not addressed.
- Huge sizes are rejected rather than reproducing upstream's u32 alignment wrap.

## Ring buffer

The public RingBuffer and its 32-byte shared arena header keep the pinned layout
(release counter at offset 28). `ring_layout` returns checked 8-byte-rounded
mapping/payload sizes excluding the header; too-small or overflowing requests
fail. Record header: u16 lock, u16 index, u32 length.

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
  match. alloc returns NULL for zero/overflow/insufficient space (native asserts
  size>0); free returns 0/-1 (native asserts).
- The snapshot ABI publishes cursor/status/count even when alloc returns NULL.
  `ring_storage_alloc` borrows the actual 32-byte C POD header (RingBufferImpl
  derives from it with no added members) and publishes only producer fields.
- `ring_construct(storage,total,shared)`: requires total>=48, 8-aligned, shared 0/1,
  valid header alignment/extent; zeroes the header, sets payload=header+32, payload
  size total-32 and the shared flag. Returns -1 without writes on invalid input.
- Shared mappings hold no Rust state, so forked releases update the same arena.

## Shared mapping

Scalar helpers: checked round-up/add/multiply and reallocation-copy plans return
0/-1 with zeroed outputs on error. Size includes the 8-byte header; multiplication
validates the rounded mapping size. A zero-byte allocation still reserves a header.

Reallocation copies min(old_total-8, new_total-8) payload bytes.

Storage functions:

- `mapping_initialize(base,total)` writes the 8-byte total-length header and returns
  base+8, or NULL.
- `mapping_extent(payload,out)` decodes a live header into a 16-byte base/total POD
  for unmap/protect; -1 with zeroed output on failure.
- `mapping_relocate(old,new)` copies the minimum payload extent between two live,
  disjoint mappings; overlap/invalid metadata rejects before copying.

Native keeps mmap/munmap/mprotect, platform flags or /dev/zero fallback, logging
and rollback. `swr_v2_memory_mapping_step` drives the whole operation; see
[mapping-operation.md](mapping-operation.md).

## Arena lifecycle

`arena_lifecycle(shared,owned,phase)` chooses FixedPool/RingBuffer allocator
effects. Flags 0/1; phase 0 construction, phase 1 release. Returns 0 borrowed/no
effect, 1/2 heap/shared allocation, 3/4 heap/shared release, -1 invalid.

## Shared tables

Row layout: the C POD row is the actual storage base of native TableRow (no derived
members). 88-byte header, alignment 8, next at 16, key at 24, payload at 88. The
lock word is volatile native storage; Rust never locks or borrows it. Row field
access uses raw unaligned loads/stores. Numeric native types are 8 bytes.

Lookup/set/delete:

- Native hashes (existing callback), clamps keys to 63 bytes and holds the root lock
  across lookup/planning/mutation. Chains must be acyclic.
- Rust owns key matching, collision planning, root promotion/deletion, header
  initialization, iterator row selection/copying, numeric encoding and bounded
  length-prefixed strings.
- set plan: 0 existing/root row, 1 native pool allocation needed, -1 invalid.
  Conflict counters increment before a failed allocation; out_flags untouched.
- delete: 0 removed, 1 missing, -1 invalid; returns a detached collision row for
  native disposal.
- Single-row iteration keeps the upstream collision cursor.
- Returned string pointers borrow the locked row; unsafe after unlock/delete/destroy.

Layout: helpers keep row-count rounding, f32 conflict arithmetic, pointer-array
placement, pool bytes and the reported pool memory_size. NaN/infinite sizing,
oversized widths and overflowing layouts are rejected. `table_storage_layout`
rounds record strides to row alignment (8) for physical storage; public
calc_memory_size/get_memory_size still use the logical plan. This adds 0..7 bytes
per record (including conflict rows). Legacy and aligned layouts must never be
mixed across forked processes.

`table_mutation_step`: 144-byte state with typed addresses of six live counters,
root/key/row/release tokens, the plan, and mode/phase.

- Mode 0 (set), phase 0: find row and complete, or request a conflict-row
  allocation (return 1, phase 1). Conflict count/max publish before the request;
  allocation failure leaves row count, links, out_flags and insert/update counters
  unchanged. Native fills only `state.row`; Rust then initializes, counts, links,
  writes flags and counts insert/update.
- Mode 1 (delete): return 2 missing, complete, or request native row release; the
  key is consumed in phase 0 and not read afterward (it may live in the released
  row). Counters update after release.
- Phase 2 is terminal; error returns -1.
- Row/insert/delete/update counters keep SeqCst native ordering; conflict count/max
  now use atomics. Native direct access to these fields must serialize or use
  compatible atomics.

`table_lookup_step`: 56-byte cursor. Phase 0 clamps the u16 key length to 63
(return 1); native calls HashFunc and fills only the hash. Phase 1 selects the
index and reads one row-pointer token (return 0); invalid/replay -1. Native keeps
prelock activity observation and the index<size assertion.

`table_iterator_reset/step`: 32-byte view of cursor addresses, current-row storage
and row size; native holds the iterator mutex. Phase 0 starts; phase 1 consumes the
native lookup/lock result (NULL inactive bucket, else a locked live root). Return 1
requests lookup/lock of the output bucket, 0 completes, -1 rejects. Native unlocks
before the next request. End-of-walk clears only the 88-byte header; reset clears
the full row.

`table_iterator_release_step`: 24-byte copied tokens; frees the current row only if
non-null, then deletes the mutex. Native refreshes the mutex token before resuming.

Column clearing:

- `table_clear_value(row,index,width,kind)`: kinds 1/2 require width 8 and write
  positive zero; kind 3 requires width>=4 and clears only the 4-byte string length.
  Unaligned access, no allocation, returns 0/-1 before any write on invalid input.
- `table_clear_columns(row,descriptors,count,item_size)`: 16-byte descriptors
  (index:usize, width:u32, kind:u32). All validated against item_size before any
  write; overlapping ranges coalesce. NULL descriptors allowed when count=0. Native
  batches at most eight live TableColumn descriptors (128 bytes) per call.
- `table_clear_step`: 240-byte cursor over a read-only pointer-token span from the
  public column vector; requests batches of up to eight in declaration order
  (return 1), native copies index/width/kind, Rust validates the batch then writes.
  0 completes, -1 rejects without restart; completed batches stay committed.

`table_value_step`: 48-byte cursor. Ordinary writes complete in one call. An
overlong string returns 1 before writing; native logs the warning, refreshes
descriptor facts and resumes phase 1 (also for an overlong null string). Terminal
replay rejects.

Metadata and construction:

- `table_metadata_fields`: 128-byte descriptor of typed addresses of Table scalar,
  pointer and counter fields. The bool target is written as 0/1 (sizeof(bool)==1
  checked).
- `table_metadata_initialize` validates configuration before any write, then sets
  size/mask/item/accounting/conflict, pointer fields, created and six statistics.
- `table_creation_step`: 112-byte cursor. Phase 0 refuses an already-created table
  and returns logical/physical plans (return 1). Native allocates and constructs
  pool/iterator under RAII. Phase 1 validates tokens and unchanged metadata, then
  publishes arena/row-array/pool/iterator pointers, logical accounting and created
  (return 0); replay/invalid -1. Exclusive construction only. See
  [table-construction.md](table-construction.md).
- Native rollback: pool is destroyed before its mapping; iterator keeps its row
  buffer until mutex construction succeeds. A failed create leaves the table
  unready with zero memory size and reusable.
- `table_declaration_step`: 96-byte cursor with name, column token and typed
  item-size/index/width addresses. Phase 0 validates type/width/total (return 1:
  construct column); phase 1 publishes index (return 2: append to vector); phase 2
  inserts the name index and commits width (return 0) or requests pop-back
  rollback (return 3). Unknown type -2 (native warning), other failures -1.
  Duplicate names keep first-index lookup while every declaration advances layout.
- `table_teardown_step`: 88-byte cursor, one 16-byte effect per step: column
  delete, vector delete, iterator delete, pool delete, optional shared arena
  release, mutex delete, Table-pool release. The Rust name index is disposed
  first; `memory=NULL` is published after arena release. Return 1 effect, 0
  terminal, -1 invalid. No restart after partial failure. The cursor borrows the
  `column_list->data()` token span; it never copies the vector.

## Table column-name index

Opaque Rust HashMap of owned byte keys to native TableColumn tokens, with a
deterministic hasher (no entropy/syscall). Binary/empty names allowed;
first-insertion-wins on duplicates, matching `unordered_map::emplace`. Duplicates
still advance item_size and enter the public column vector, which owns every
column; destroy drains the index then deletes all vector entries (fixing the
original duplicate-column leak).

- create returns 0/-1 (NULL on failure).
- insert returns 0 inserted, 1 existing, -1 invalid/OOM; caller keeps custody on
  failure/duplicate. Key allocation and reserve happen before insertion.
- get/count/pop do not allocate; pop returns one token for native destruction.
- release rejects a nonempty map. `columns_discard` drops the whole index without
  destructors; the caller must hold every token.
- Metadata is process-private (copy-on-write across fork). Registry init failure
  returns NULL/false rather than propagating an allocator exception.

TableV2 replaces this with a Rust record owner; see [columns-v2.md](columns-v2.md).

## Benchmarks

Prepare optimized binaries (correctness gate runs first, no timing):

```
python3 tests/modules/memory/gate.py --source /absolute/pinned/upstream \
  --extension /absolute/hybrid/swoole.so --reference-extension /absolute/upstream/swoole.so \
  --archive /absolute/build/rust-prefix/lib/libswoole_rs_ffi.a \
  --prepare-benchmark target/memory-benchmark \
  --output target/reports/memory/gate-benchmark-prepared.json
```

Measure on an otherwise idle host:

```
python3 tests/modules/memory/benchmark.py --prepared target/memory-benchmark \
  --trials 7 --loops 100000 --output target/reports/memory/benchmark.json
```

Trials alternate upstream/Rust order and compare workload checksums. Cases: fixed
alloc/free, heap pop/push, locked channel push/pop, ring alloc/free, global 64-byte
alloc, table row/column lookup/read-modify-write.

Known regression: the most recent measured Rust/upstream elapsed-time ratios were
fixed 1.068, heap 1.187, channel 1.437, ring 2.074, global 1.099, table 1.061.
These block default activation. Later seams (ring storage ABI, lookup/clear/value
resume steps) add crossings and have not been measured.
