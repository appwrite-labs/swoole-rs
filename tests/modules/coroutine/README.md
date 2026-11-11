# Coroutine validation

Reference pin: `76c657d88be39b7af6bf85e46bcbac0da7387760`. `PINNED_TREE` below is
the pinned upstream Git checkout (`target/upstream/src`); `ARCHIVE` is the real
Rust FFI static archive. Most gates refuse an existing `--report` path.

## Upstream GoogleTests in Rust

`core-tests/src/coroutine/base.cpp` and `coroutine/file.cpp` no longer build. Their
TESTs are Rust functions in `base.rs`, one per TEST with the same inputs and expected
values, run by `hooks-system-rust` (tests/modules/hooks/system.py, which links the
case binary with the crates and system.py's static libswoole.a (the pin's thirdparty and native/'s Zend-free sources); Linux):

| Upstream TEST | Rust case |
| --- | --- |
| `coroutine_base.<name>`, all 26 (`create` through `c_api`) | `coroutine_base.<name>` |
| `coroutine_async_file.async_file` | `coroutine_async_file.async_file` |

The facade's getters are field reads over crates/coroutine's record, its operations
the `swr_v1_coroutine_*` exports and upstream symbols. `coroutine::run` and
`Coroutine::bailout` take a function and its data (`swoole_coroutine_create`,
`swr_v1_coroutine_bail`); `swoole_event_defer` has C++ linkage, so
tests/modules/hooks/system/fixture.cc wraps it. The repo-owned
`rust_coroutine_lifetime.cancellation_copy_failure_preserves_wait` witness checked
that a throwing `CancelFunc` copy left the wait intact; it went with C++'s
`std::function` cancel callbacks, since nothing is copied now.

Upstream PHPTs for this module run in the merge tier's PHPT gate (the `phpt-*` jobs), which runs every pinned PHPT on upstream and the Cargo build. To attribute a gate regression to this module, see *Attributing a regression* in [corpus/upstream/README.md](../../../corpus/upstream/README.md).

## Source-body and ABI gates

```
python3 tools/cargo_asan.py --package swoole-rs-coroutine --report OUT.json
```

- `tools/cargo_asan.py` runs crates/coroutine's unit tests under Rust ASan (the plain
  run is `coroutine-unit`).
- The scheduler, coroutine state, the live map, stacks and the context switch (crates/coroutine)
  are covered by its Rust unit tests and, through real context switches, by
  `base.rs`'s upstream GoogleTests and the coroutine PHPT families.
- Channel is covered by `channel.rs` (crates/coroutine's unit tests): upstream's
  seven `coroutine_channel` TESTs from core-tests/src/coroutine/channel.cpp, one
  Rust test each (`coroutine_channel_<name>`) with the same inputs and
  assertions, and the two channel-deleting waiter cases of the former
  `lifetime.cpp`, over a thread-per-coroutine model of Swoole's coroutines and
  timers; and by the `swoole_channel_coro` and `swoole_coroutine/cancel` PHPTs. The live map lists newest first, the order `Coroutine::list()`
  shows from upstream's `std::unordered_map` (#357).

The C ABI contract is [native/coroutine/contract.md](../../../native/coroutine/contract.md).
