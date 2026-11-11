# Memory subsystem validation

Checks for the Rust memory module (fixed and global pools, channel, shared
mapping and table). ABI lifetimes and native contracts are in
[native/memory/contract.md](../../../native/memory/contract.md).

The `*.rs` files here are the crate's unit tests, included by
`crates/memory/src/*.rs` via `#[path]`.

## Table witnesses

Upstream PHPTs for this module run in the merge tier's PHPT gate (the `phpt-*` jobs), which runs every pinned PHPT on upstream and the all-modules hybrid. To attribute a gate regression to this module, see *Attributing a regression* in [corpus/upstream/README.md](../../../corpus/upstream/README.md).

```
python3 tests/modules/memory/witnesses.py --engine upstream=/absolute/reference/swoole.so \
  --engine hybrid=/absolute/immutable-prefix/modules/swoole.so --require-memory-bridge hybrid \
  --output target/reports/memory/table-witnesses.json
```

`witnesses.py` (`memory-table-witnesses`) runs four PHP witnesses, not upstream PHPTs, on every engine and requires their observations to match. `--require-memory-bridge ENGINE` rejects an engine that does not advertise memory feature bit 64:

- `table_state.php`: duplicate-column first lookup, binary names/keys, iteration,
  increment after delete, memory sizes.
- `table_fork.php`: fork rounds with create/grow/iterate/delete/destroy cycles and
  column-index teardown in children.
- `table_storage.php`: odd-width logical memory, iteration and fork.
- `table_clear_batches.php`: multi-batch clear/reuse, float zero, fork.

Known failure: on Linux, `force_unlock.phpt` fails in both upstream and hybrid
engines (18/19 pass). The child's OOM is delayed by Zend
(`zend_handle_delayed_oom()`) until after the table row is unlocked, so the
expected dead-owner warning never appears. macOS emits the warning.

## Upstream's GoogleTests, as Rust tests

Upstream's fixed_pool, global_memory, table and core channel GoogleTests are Rust
tests here, one per TEST with its inputs and expected values (#1513, #1665).
Nothing in the build includes `swoole_table.h` or the `FixedPool` facade any more,
so they left with these ports.

`memory/ringbuffer.cpp`, `core/heap.cpp` and `fixed_pool.realloc` have no port:
their subjects went (#2599, #2849). Upstream calls `RingBuffer` from nowhere but that TEST, and the timer,
`swoole::Heap`'s one caller, keeps its own heap (`crates/timer/src/heap.rs`), so
nothing in the extension used the Rust ports. `fixed_pool.realloc` tests
`sw_shm_realloc`, which nothing else called.

| Upstream TEST (core-tests/src/) | Rust test |
| --- | --- |
| `memory/fixed_pool.cpp` `fixed_pool.alloc` | `members::tests::fixed_pool_alloc` |
| `memory/global_memory.cpp` `global_memory.alloc` | `members::tests::global_memory_alloc` |
| `memory/table.cpp` `table.create` | `table::tests::table_create` |
| `memory/table.cpp` `table.iterator` | `table::tests::table_iterator` |
| `memory/table.cpp` `table.iterator_2` | `table::tests::table_iterator_2` |
| `memory/table.cpp` `table.conflict1` | `table::tests::table_conflict1` |
| `memory/table.cpp` `table.conflict2` | `table::tests::table_conflict2` |
| `memory/table.cpp` `table.conflict3` | `table::tests::table_conflict3` |
| `memory/table.cpp` `table.conflict4` | `table::tests::table_conflict4` |
| `memory/table.cpp` `table.get_value` | `table::tests::table_get_value` |
| `memory/table.cpp` `table.lock` | `table::tests::table_lock` |
| `memory/table.cpp` `table.size_limit` | `table::tests::table_size_limit` |
| `memory/table.cpp` `table.lock_crash` | `table::tests::table_lock_crash` |
| `memory/table.cpp` `table.lock_race` | `table::tests::table_lock_race` |
| `core/channel.cpp` `channel.push` | `channel::tests::channel_push` |
| `core/channel.cpp` `channel.peek` | `channel::tests::channel_peek` |
| `core/channel.cpp` `channel.notify` | `channel::tests::channel_notify` |

Where a port differs from the C++ in form, not in what it checks: the table's
header and arena are shared mappings made directly (`Table::make` takes the header
from `SwooleG`'s pool, which a unit test has no `swoole_init` for);
`global_memory.alloc` skips `free`, a no-op; and the
fork tests wait for their own child rather than every child, and also check its exit
status.

`memory-unit` runs them; `memory-rust-asan` runs them under Rust ASan
(`tools/cargo_asan.py --package swoole-rs-memory`).
