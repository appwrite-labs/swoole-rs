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

`core.py` builds full native reference and coroutine-only libraries and runs the
33 unchanged GoogleTests (32 before v6.2.4 added `coroutine_base.expired_origin`), plus three supplemental real-context tests from
`lifetime.cpp`, each in its own process group. `--ucontext` derives a private
config selecting ucontext. `--rust-asan` requires ASan imports in the archive;
ASan stack-switch warnings yield `warned`, not `passed`. On macOS pass
`--gtest-prefix` pointing at a C++14-compatible GoogleTest (1.12).

## Source-body and ABI gates

```
python3 tests/modules/coroutine/compile_gate.py --source PINNED_TREE [--timing] [--stack-usage] \
  [--backend asm|ucontext|thread] [--standard c++14|c++17] --report OUT.json
python3 tests/modules/coroutine/channel_state_gate.py --source PINNED_TREE [--archive ARCHIVE] --report OUT.json
python3 tests/modules/coroutine/stack.py --source PINNED_TREE --archive ARCHIVE --sanitize --report OUT.json
python3 tests/modules/coroutine/production_stack.py --source PINNED_TREE --source-ref REF \
  --selected-export SELECTED/export.json --native-export NATIVE/export.json --output OUT.json
```

- `compile_gate.py` compiles original, patched and disabled `base`, `channel`,
  `context` and `thread_context` translation units; the disabled overlay must
  fail closed with its guard message.
- `channel_state_gate.py` runs the Rust unit tests (ordinary and with Rust ASan),
  stages the current overlays and runs the C11 ABI fixture `context_cells.c`.
- `stack.py` (stack markers) compares native vs selected source fragments on
  inert metadata. `--rust-instrumented` (Linux with `--sanitize` only) also
  requires Rust ASan imports in the archive.
- The scheduler, coroutine state and Channel (crates/coroutine) are
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
- `production_stack.cc` still models the old protected Context fields and direct
  imports, so it does not match the current overlays.
- Optional thread-context is unsupported; `core.py` rejects a config defining
  `SW_USE_THREAD_CONTEXT`.

The C ABI contract is [native/coroutine/contract.md](../../../native/coroutine/contract.md).
