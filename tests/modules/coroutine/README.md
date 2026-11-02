# Coroutine validation

Reference pin: `c7e514509451beafd4d70ce5b78943d51e76381c`. `PINNED_TREE` below is
the pinned upstream Git checkout (`target/upstream/src`); `ARCHIVE` is the real
Rust FFI static archive. Most gates refuse an existing `--report` path.

## Original tests

```
python3 tests/modules/coroutine/core.py --source PINNED_TREE --config CONFIG_H --archive ARCHIVE \
  --build-root PRIVATE_DIR [--sanitize] [--ucontext] [--rust-asan] [--lifetime-only] --output OUT.json
```

Upstream PHPTs for this module run in the merge tier's PHPT gate (the `phpt-*` jobs), which runs every pinned PHPT on upstream and the all-modules hybrid. To attribute a gate regression to this module, see *Attributing a regression* in [corpus/upstream/README.md](../../../corpus/upstream/README.md).

`core.py` builds the full native reference and the archive's hybrid tree (every
module's patches, `overlay_hybrid_tree`, #796) and runs the
33 unchanged GoogleTests (32 before v6.2.4 added `coroutine_base.expired_origin`), plus three supplemental real-context tests from
`lifetime.cpp`, each in its own process group. `--ucontext` derives a private
config selecting ucontext. `--rust-asan` requires ASan imports in the archive;
ASan stack-switch warnings yield `warned`, not `passed`. On macOS pass
`--gtest-prefix` pointing at a C++14-compatible GoogleTest (1.12).

## Source-body and ABI gates

```
python3 tests/modules/coroutine/compile_gate.py --source PINNED_TREE --archive ARCHIVE [--timing] [--stack-usage] \
  [--backend asm|ucontext|thread] [--standard c++14|c++17] --report OUT.json
python3 tests/modules/coroutine/rust_asan.py --report OUT.json
python3 tests/modules/coroutine/production_stack.py --source PINNED_TREE --source-ref REF \
  --selected-export SELECTED/export.json --native-export NATIVE/export.json --output OUT.json
```

- `compile_gate.py` compiles `base`, `channel`, `context` and `thread_context`
  from the pin (original) and from the archive's hybrid tree with its switches
  (patched, `hybrid_tree`); without `SWOOLE_RS_COROUTINE` (disabled) `base` and
  `channel` must fail closed with their guard message.
- `rust_asan.py` runs crates/coroutine's unit tests under Rust ASan (the plain
  run is `coroutine-unit`).
- The scheduler, coroutine state, Channel and stacks (crates/coroutine) are
  covered by its Rust unit tests and, through real context switches, by
  `core.py`'s upstream GoogleTests and the coroutine PHPT families. The
  coroutine map (`Coroutine::coroutines`, `print_list`, lookups) stays
  upstream's `std::unordered_map`: `Coroutine::list()` exposes its order (#357).
- `production_stack.py` builds the Context constructor/destructor/query into a
  private PHP addon against each engine's SDK and checks marker observations.

## Benchmarks

```
python3 tests/modules/coroutine/prepare_benchmark.py --core-build CORE_PAIR_DIR --output NEW_DIR
python3 tests/modules/coroutine/benchmark.py --prepared NEW_DIR --exclusive-host-lease ID --output OUT.json
```

`prepare_benchmark.py` copies an optimized, assertion-enabled native core pair
(`core-upstream`, `core-rust`), links `benchmark.cc` and runs untimed
ten-operation checks for create/yield/channel/cancel/fanin. `benchmark.py`
verifies the prepared binary/library hashes and runs alternating timed trials.

## Known failures and blocked activation

- `core.py --ucontext` on macOS fails all 32 originals per engine, including a
  native SIGSEGV in `_setcontext`. Assembly contexts pass on macOS and Linux.
- Optional thread-context is unsupported; `core.py` rejects a config defining
  `SW_USE_THREAD_CONTEXT`.

The C ABI contract is [native/coroutine/contract.md](../../../native/coroutine/contract.md).
