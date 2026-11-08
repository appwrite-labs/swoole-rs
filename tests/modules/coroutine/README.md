# Coroutine validation

Reference pin: `c7e514509451beafd4d70ce5b78943d51e76381c`. `PINNED_TREE` below is
the pinned upstream Git checkout (`target/upstream/src`); `ARCHIVE` is the real
Rust FFI static archive. Most gates refuse an existing `--report` path.

## Original tests

```
python3 tests/modules/coroutine/core.py --source PINNED_TREE --config CONFIG_H --archive ARCHIVE \
  --build-root PRIVATE_DIR [--sanitize] [--rust-asan] [--lifetime-only] --output OUT.json
```

Upstream PHPTs for this module run in the merge tier's PHPT gate (the `phpt-*` jobs), which runs every pinned PHPT on upstream and the all-modules hybrid. To attribute a gate regression to this module, see *Attributing a regression* in [corpus/upstream/README.md](../../../corpus/upstream/README.md).

`core.py` builds the full native reference and the archive's hybrid tree (every
module's patches, `overlay_hybrid_tree`, #796) and runs the
26 unchanged `coroutine_base` GoogleTests, plus the supplemental real-context test in
`lifetime.cpp`, each in its own process group. `--rust-asan` requires ASan imports in the archive;
ASan stack-switch warnings yield `warned`, not `passed`. On macOS pass
`--gtest-prefix` pointing at a C++14-compatible GoogleTest (1.12).

## Source-body and ABI gates

```
python3 tests/modules/coroutine/compile_gate.py --source PINNED_TREE --archive ARCHIVE [--timing] [--stack-usage] \
  [--backend asm|thread] [--standard c++14|c++17] --report OUT.json
python3 tools/cargo_asan.py --package swoole-rs-coroutine --report OUT.json
```

- `compile_gate.py` compiles `base`, `context` and `thread_context`
  from the pin (original), and those still in the archive's hybrid tree with its
  switches, plus native/coroutine's shim (patched, `hybrid_tree`; `base.cc` and
  `context.cc` are gone).
- `tools/cargo_asan.py` runs crates/coroutine's unit tests under Rust ASan (the plain
  run is `coroutine-unit`).
- The scheduler, coroutine state, the live map, stacks and the context switch (crates/coroutine)
  are covered by its Rust unit tests and, through real context switches, by
  `core.py`'s upstream GoogleTests and the coroutine PHPT families.
- Channel is covered by `channel.rs` (crates/coroutine's unit tests): upstream's
  seven `coroutine_channel` TESTs from core-tests/src/coroutine/channel.cpp, one
  Rust test each (`coroutine_channel_<name>`) with the same inputs and
  assertions, and the two channel-deleting waiter cases that were in
  `lifetime.cpp`, over a thread-per-coroutine model of Swoole's coroutines and
  timers; and by the `swoole_channel_coro` and `swoole_coroutine/cancel` PHPTs. The live map lists newest first, the order `Coroutine::list()`
  shows from upstream's `std::unordered_map` (#357).

## Known failures

- Optional thread-context is unsupported; `core.py` rejects a config defining
  `SW_USE_THREAD_CONTEXT`.

The C ABI contract is [native/coroutine/contract.md](../../../native/coroutine/contract.md).
