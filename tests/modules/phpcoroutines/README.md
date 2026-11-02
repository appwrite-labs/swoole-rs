# PHP Coroutine/System/Scheduler frontends

The `phpcoroutines` feature (macro `SWOOLE_RS_PHPCOROUTINES`, feature bit
134217728) is opt-in. It patches the three pinned frontend translation units
(`ext-src/swoole_coroutine.cc`, `swoole_coroutine_system.cc`,
`swoole_coroutine_scheduler.cc`) and selected bodies of `php_swoole_coroutine.h`
to call Rust policy/metadata in `crates/phpcoroutines`. Native Zend, VM, context
and IO effects stay in C++ and run after Rust returns; the native fallback is
unchanged. See `native/phpcoroutines/contract.md` for the ABI.

Selected `PHPContext` field source types (deferred-owner pointer, primitive
metadata cells) and private static state types differ from native, so consumers
must rebuild against the selected SDK. Public layouts and offsets are unchanged.

## Patches

Edit `migration/patches/phpcoroutines/000{1..4}-*.patch` with `tools/patches.py`
(see [hybrid build](../../../docs/hybrid-build.md#editing-patches)). `overlay.py`
applies them to the pinned sources for the source-shape tests and `gate.py`; the
`*_bodies.py` modules hold the signatures those tests check.

## Gates

`PINNED` is `target/upstream/src`; `ARCHIVE` is an FFI static library
(`libswoole_rs_ffi.a`).

- `gate.py --source PINNED --output OUT.json [--archive ARCHIVE] [--sanitize] [--rust-sanitize]`
  compiles native and selected copies of ordinary frontend bodies against inert
  native-effect recorders and requires identical traces: Scheduler add/start,
  user-yield registry (global and TLS), System file/sleep/resolver bodies,
  coroutine options, create, stats (with and without io_uring), defer, Scheduler
  set, activation, context query, backtrace, the three MINIT bodies (PHP 8.3 and
  8.4), context transfer, config state (global and TLS), autoload policy and the
  common header bodies. Without `--archive` it builds a module-only staticlib;
  with it, it checks the archive's ABI version and feature bit. `--sanitize`
  adds ASan/UBSan to the C++; `--rust-sanitize` (fresh archive only) builds Rust
  with ASan and checks the objects import ASan symbols.
- `methods.py --source PINNED --archive ARCHIVE --output OUT.json [--sanitize]`
  compares the Rust method-catalogue projection against the pinned Zend method
  tables (56 rows, 57 with `SW_CORO_TIME`).
- `ffi.py --output OUT.json [--archive ARCHIVE]` builds a phpcoroutines-only
  Cargo archive (or uses the supplied one) and runs the C11/C++14 consumer
  `ffi.c` over every export.
- `context_metadata.py --source PINNED --archive ARCHIVE --output OUT.json`
  runs the primitive context-metadata model (`context_metadata.cc`), normally
  and with native ASan/UBSan.
- `join_metadata.py --source PINNED --archive ARCHIVE --output OUT.json` runs
  the join identity-set model (`join_metadata.cc`), normally and with native
  ASan/UBSan.
- `sdk.py --source PINNED --sdk-report PHPHELPERS_SDK.json --output OUT.json`
  reuses the captured phphelpers SDK profiles to compile all three selected TUs
  (native, phpcoroutines-only over the pin, and the hybrid build tree's) in C++14/C++17, checks the Rust imports of each object (none for native), and
  asserts `PHPContext`, `SchedulerObject` and autoload-owner layouts match native.
- `stats_runtime.py --profile statistics|idle-queries|startup|configuration
  --engine native=SO --engine selected=SO --build-manifest JSON --archive ARCHIVE
  --source-ref REF --output OUT.json` checks that the build manifest and patches
  contain the compiled Rust source and actual callers for the profile, then runs
  the matching PHP fixture (`statistics.php`, `idle_queries.php`, `startup.php`,
  `configuration.php`) on both engines and requires identical output.

`tools/dev.py` registers these as `phpcoroutines-*` phases.

## Unit tests

The `test_*.py` files check the generated patches (exact native fallback,
effect ordering, Rust call sites) and the `stats_runtime.py`/`gate.py`
validators. From this directory:

```
SWOOLE_UPSTREAM_SOURCE=PINNED python3 -m unittest $(ls test_*.py | sed 's/\.py//')
```

Tests that need the pinned source skip without it.

## Known gaps

- The 175 upstream PHPTs (130 Coroutine, 18 System, 27 Scheduler) have not
  been run against the selected build.
- Models use inert stubs: no real callbacks, Zend objects/GC, coroutine
  lifecycle, bailout, allocation failure, ZTS or thread execution.
- On Linux PHP 8.2 builds without `ZEND_CHECK_STACK_LIMIT`, the stack-bound
  callers are reported UNSUPPORTED by `sdk.py`. `SW_CORO_TIME` is absent in the
  captured profiles, so `getExecuteTime` is UNSUPPORTED there too.
- Recent batches (scheduler slot, method catalogue, autoload owner, scheduler
  callback owner) were only run on macOS; Linux is not yet run.
- No performance measurement; default activation stays off.
