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

## Original native GoogleTests

```
python3 tests/modules/memory/core.py --source /absolute/pinned/upstream \
  --inventory-only --output /private/reports/memory-core-inventory.json
python3 tests/modules/memory/core.py --source /absolute/pinned/upstream \
  --config /private/snapshot/config.h --archive /private/libmemory.a \
  --build-root /private/memory-build --output /private/reports/memory-core.json
```

Runs the 20 unchanged original fixed_pool/global_memory/ringbuffer/table/heap/
channel GoogleTests against both the full native library and the memory overlay.
Linux only; needs CMake, GTest, PHP headers and the upstream native dependencies.
The build root must be outside the reference and the repository. Original test
sources are checked against their pinned hashes after overlays, native assertions
stay enabled (`-O2 -UNDEBUG`), and each test runs in its own bounded process.
Timeouts, missing XML, skips and failures fail.

- `--case group.name` (repeatable) runs a subset; `whole_inventory_passed` is
  then false.
- `--model` also builds `probe.cc`/`standalone.cc` against each library and runs
  the collection, mapping, metadata, odd-row, creation and clearing models.
- `--sanitize` enables native ASan (not UBSan: the packed original C++ has known
  alignment UB). `--rust-asan` additionally requires the archive to have been
  built with `RUSTFLAGS=-Zsanitizer=address`, checked via `nm`.

Known failures:

- With `--sanitize`, `table.size_limit` fails in both engines: the original test
  never destroys its two tables, and LeakSanitizer reports the leaked metadata.
- With `--model --sanitize`, the upstream model leaks duplicate-column metadata
  (fixed in the Rust overlay), so the combined comparison fails.
- Native packed-row locks on odd-width rows fault on ARM64 (32-bit CAS
  alignment). Models use aligned rows; this remains an open gap.

## Rust ASan

```
python3 tests/modules/memory/rust_asan.py --output target/reports/memory/rust-asan.json
```

Builds the crate's unit tests with `-Zsanitizer=address` (via
`RUSTC_BOOTSTRAP=1`), checks the LLVM IR contains ASan instrumentation, and
requires every listed test to pass with none ignored or filtered. Leak detection
is on for Linux, off for macOS. The precompiled standard library is not
instrumented.

Linux PHP-loaded ASan is blocked by `RTLD_DEEPBIND`.

## Compiler-only and model checks

These run from `tools/dev.py` with the pinned source and, where shown, the hybrid
build's `libswoole_rs_ffi.a`.

```
python3 tests/modules/memory/table_compat.py --source /absolute/pinned/upstream \
  --archive /absolute/libswoole_rs_ffi.a [--fixed-pool] [--upstream-unused-parameter-policy] \
  --output target/reports/memory/table-compat.json
python3 tests/modules/memory/fixed_native_construction.py \
  --archive /absolute/build/rust-prefix/lib/libswoole_rs_ffi.a --output target/reports/memory/fixed-construction.json
python3 tests/modules/memory/mapping_operation.py --source /absolute/pinned/upstream \
  --archive /absolute/libswoole_rs_ffi.a --output target/reports/memory/mapping-operation.json
```

The selected side always compiles against the hybrid build's fully patched
headers (`<build>/src/include`, its Rust ABI headers and its module switches),
the original side against the pin.

- `table_compat.py`: compiles the original and selected `table.cc` (and `fixed_pool.cc` with `--fixed-pool`) at
  C++14/17/20 with `-Werror`; any warning fails.
- `fixed_native_construction.py`: compiles the selected `FixedPool` constructor
  bodies with `fixed_native_construction.cc` under ASan/UBSan at C++14/17 and
  checks exact owned/borrowed case counts.
- `mapping_operation.py`: syntax-checks original and selected `shared_memory.cc`
  for anonymous, file and no-`MAP_FAILED` backends, then runs the selected
  `SharedMemory` bodies (`mapping_operation_model.cc`) and the C11 caller
  (`mapping_operation.c`) under ASan/UBSan against the archive.

`fixed_native_construction.py` and `mapping_operation.py` refuse to overwrite an
existing output file and refuse to compile with less than 4 GiB free.

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
without concurrent workloads. Measured heap/ring/channel cases are slower than
upstream, which blocks default activation.

## Unit tests

```
python3 -m unittest tests/modules/memory/test_benchmark.py
```

- `test_benchmark.py`: `benchmark.py` timing-case inventories with mocked runs.

## Gate audit (#488)

All six phases are kept, none deleted:

- `memory-native`: the FFI checksum differential of the pinned and overlaid
  pools, heap, channel, mapping and table. It also checks that every native
  `assert` survives the overlays and that headers reject a build without the
  memory feature.
- `memory-fixed-native-construction`, `memory-mapping-operation`: the selected
  C++ bodies under ASan/UBSan at C++14/17, including every mapping backend.
- `memory-table-compatibility`: the only C++14/17/20 `-Werror` builds of
  `table.cc`.
- `memory-table-witnesses`: table fork, grow, clear-batch and odd-width
  observations that must equal upstream's. No PHPT compares these across engines.

`core.py` and `rust_asan.py` run in no CI lane: #738.

