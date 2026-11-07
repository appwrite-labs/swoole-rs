# Memory subsystem validation

Opt-in checks for the Rust-backed memory module (fixed/global/ring pools, heap,
channel, shared mapping and table). ABI lifetimes and native contracts are in
[native/memory/contract.md](../../../native/memory/contract.md).

The `*.rs` files here are the crate's unit tests, included by
`crates/memory/src/*.rs` via `#[path]`.

## Native differential gate

```
python3 tests/modules/memory/gate.py --source /absolute/pinned/upstream \
  --extension /absolute/hybrid/swoole.so --reference-extension /absolute/reference/swoole.so --sanitize \
  --archive /absolute/build/rust-prefix/lib/libswoole_rs_ffi.a \
  --output target/reports/memory/native.json
```

Compiles the pinned C++ sources and the hybrid build's patched sources (the
build `--archive` comes from) into `probe.cc` facades, loads each through PHP FFI
alongside the extension built from the same headers (the reference for the pinned
probe, the hybrid for the Rust one: the probes read SwooleG through those headers), and requires identical checksum traces. It
never rebuilds the archive. It also checks
that every original native `assert(...)` expression survives the overlays, that
the overlaid headers reject a build without the memory feature and accept one
configured only through `config.h`, and runs exact untimed constructor and
column preflights. Rust-only checks cover mapping shrink and odd-width table
rows. `--sanitize` adds ASan/UBSan to the C++ facades only (not Rust or PHP).
`--prepare-benchmark DIR` (unsanitized only) stores optimized binaries, a source
snapshot and a hash manifest for `benchmark.py`.

Known differences: shrinking `realloc` copies `min(old, new)` payload and is
compared on Rust only; odd-width native rows use aligned physical
storage with unchanged logical byte accounting.

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

## Original native GoogleTests and Rust ASan

`memory-native-originals` runs upstream's 19 fixed_pool, global_memory,
ringbuffer, table and channel GoogleTests unchanged on the reference and
the hybrid tree (`CORE_TESTS` in tools/dev.py, via tools/core_tests.py; Linux,
integrate). `memory-rust-asan` runs the crate's unit tests under Rust ASan
(`tools/cargo_asan.py --package swoole-rs-memory`). `heap.random` is a Rust test
in `heap.rs` (#1513).

## Benchmark

```
python3 tests/modules/memory/gate.py ... --prepare-benchmark target/memory-benchmark
python3 tests/modules/memory/benchmark.py --prepared target/memory-benchmark \
  --output target/reports/memory/benchmark.json [--verify-only]
```

Verifies the prepared extension, source snapshot and binaries against the
manifest hashes, then runs alternating upstream/rust pairs (`--trials` 3-15,
`--loops`) and reports per-case medians and `rust_over_upstream` ratios. All
trials must produce one checksum and exactly the manifest's timing cases. Run
without concurrent workloads. Measured ring/channel cases are slower than
upstream, which blocks default activation.

## Unit tests

```
python3 -m unittest tests/modules/memory/test_benchmark.py
```

- `test_benchmark.py`: `benchmark.py` timing-case inventories with mocked runs.

## Gate audit (#488)

Three phases remain. `memory-fixed-native-construction`, `memory-mapping-operation`
and `memory-table-compatibility` compiled the hybrid `src/memory` C++ bodies, and
went with them (#1500):

- `memory-native`: the FFI checksum differential of the pinned and overlaid
  pools, channel, mapping and table. It also checks that every native
  `assert` survives the overlays (a deleted source's as `// assert(...);`-marked
  Rust asserts in `crates/memory/src/members.rs`) and that headers reject a
  build without the memory feature.
- `memory-table-witnesses`: table fork, grow, clear-batch and odd-width
  observations that must equal upstream's. No PHPT compares these across engines.

