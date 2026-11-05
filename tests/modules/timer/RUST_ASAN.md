# Rust timer ASan

Instruments the Rust timer queue as well as the native C/C++ callers. Test-only;
no production flags or ABI change. Linux, with the pinned reference available:

```sh
RUSTC_BOOTSTRAP=1 RUSTFLAGS='-Zsanitizer=address -Zexternal-clangrt=yes' \
  cargo rustc -p swoole-rs-timer --release --offline --locked \
  --target-dir target/rust-asan-timer --crate-type staticlib -v
python3 -m pytest tests/modules/timer/test_kernel.py --sanitize --rust-asan \
  --archive target/rust-asan-timer/release/libswoole_rs_timer.a
```

`--rust-asan` declares an externally instrumented archive; it does not build
one. The gate rejects an archive without ASan report imports.

On Linux the C ABI gate passes all 13 topics with native ASan/UBSan, leak
detection and the Rust archive instrumented. The precompiled Rust standard library is not
instrumented. Exhaustion/OOM injection and concurrent-fork allocator safety are
outside this gate.

Rust unit tests under ASan (including the allocation-failure tests) pass on
Linux and macOS:

```sh
RUSTC_BOOTSTRAP=1 RUSTFLAGS=-Zsanitizer=address cargo test -p swoole-rs-timer \
  --release --offline --locked --target-dir target/rust-unit-asan -v -- --nocapture
```

macOS leak detection is off because its runtime lacks it.

## Known failures

- macOS: native linking of an instrumented caller with the Rust ASan archive
  fails. Apple's runtime lacks `___asan_version_mismatch_check_v8`; selecting
  Rust's bundled runtime instead fails because Apple Clang's caller imports
  `___asan_version_mismatch_check_apple_clang_2100`. A matching
  compiler/runtime pair is required.
- PHP-loaded Linux ASan is blocked by PHP's `RTLD_DEEPBIND` loading (see
  [hybrid.md](hybrid.md)).
