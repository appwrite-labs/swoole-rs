# Hooks

Pin `c7e514509451beafd4d70ce5b78943d51e76381c`.

## Native-facade gates

```sh
python3 tests/modules/hooks/async_gate.py --source PINNED_SOURCE --archive BUILD/rust-prefix/lib/libswoole_rs_ffi.a --sanitize --report OWN/async.json
python3 tests/modules/hooks/compile_gate.py --source PINNED_SOURCE --archive BUILD/rust-prefix/lib/libswoole_rs_ffi.a --report OWN/compile.json
```

`packet_gate.py` takes the same arguments. Each `*_gate.py` extracts the pinned function bodies and
the patched ones from the hybrid build `--archive` comes from (`hybrid_tree`), compiles both
against a deterministic native facade (`*_fixture.h`) and runs identical
scenarios through each, comparing effects and order. Syscalls, contexts, TLS
and kernel IO are faked; these are policy/order checks, not runtime proof.
`--sanitize` adds ASan/UBSan to C++ only, not the Rust archive.

- The ordered gate is gone. `wait_for` lost its last callers (flock and `waitpid_safe` are
  Rust) and left the build; its 22 retry and cancel cases are `ported_wait_for_matrix` in
  `crates/hooks/src/flock.rs`, over the same retry loop. The coroutine Socket's transfers and
  `TimerController` moved to Rust with their cases (`ported_*` tests in
  `crates/hooks/src/transfer.rs` and `co_io.rs`).
- `async_gate.py` (426 cases): async event/lambda/sleep/DNS bodies, including
  dispatch rejection, timeout/cancel completion, late canceled completion and
  errno capture. The hybrid side's `async` is the patched one-liner over
  `swr_v1_hook_async` (`crates/hooks/src/offload.rs`), driven through the same
  facade with upstream's `AsyncEvent` layout.
- `packet_gate.py` (240 cases): length/EOF/raw framing paths. Its 384
  vectored all-transfer cases moved to Rust with `readv_all`/`writev_all`
  (`ported_vector_all_matrix` in `crates/hooks/src/vector.rs`).
- The socket policy gate's bodies (the bound-coroutine checks, `set_err`,
  `check_return_value`, `is_available`, `get_event_str` and
  `TimeoutController::has_timedout`) moved to Rust with their cases (`ported_*`
  tests in `crates/hooks/src/co_io.rs`).
- The lifecycle gate's bodies (`shutdown`, `cancel`, `close`,
  `connect(addr)` and `check_liveness`) moved to Rust with their cases
  (`ported_*` tests in `crates/hooks/src/lifecycle.rs`, each comparing Rust
  with upstream's body transcribed over the same fakes).

`compile_gate.py` compiles all eight original and patched translation units as
C++14: the pinned ones, then the pin with every file the hybrid build patched
(`overlay_hybrid_tree`) under that build's `config.h`. io_uring units are
reported UNSUPPORTED when the config does not enable `SW_USE_IOURING`.

## Original tests

`hooks-native-originals` (`tools/core_tests.py`, Linux, integrate lane) runs
upstream's own `core-tests/src/coroutine/{hook,socket,system}.cpp` GoogleTests
unchanged on the reference and on the hybrid tree.

`core-tests/src/memory/lru_cache.cpp` no longer builds (`include/swoole_lru_cache.h`
is deleted). Its TESTs are Rust tests over the DNS cache's `Lru` in
`crates/hooks/src/dns_cache.rs`, with the same inputs and expected values:
`lru_cache.basic` is `lru_cache_basic`, `lru_cache.memory_free` is
`lru_cache_memory_free` and `lru_cache.lru_kick` is `lru_cache_lru_kick`.

Upstream PHPTs for this module run in the merge tier's PHPT gate (the `phpt-*` jobs), which runs every pinned PHPT on upstream and the all-modules hybrid. To attribute a gate regression to this module, see *Attributing a regression* in [corpus/upstream/README.md](../../../corpus/upstream/README.md).

## Known failures

- PHPTs (229), PASS/FAIL/SKIP: Linux reference 183/14/32 vs Rust 181/16/32;
  macOS reference 165/19/45 vs Rust 161/23/45. Rust has candidate-only
  failures on both platforms; base/async_file failures reproduce in the native
  control on macOS. Redis, external services and PHP deprecations account for
  further failures and skips.
- Native originals (101, Linux): reference 93 pass/8 fail, Rust 94 pass/7
  fail. Both fail tcp6, four external proxy cases and two DNS cases; the
  excluded `coroutine_hook.sendmsg_and_recvmsg` segfaults in ~15% of runs on
  both (its child reads the finished parent's stack); the
  reference TLS verify case timed out. An earlier run showed TLS SIGPIPE in both
  engines.
- macOS (clang): `packet_gate.py` stops at a narrowing error in the selected
  `recv_packet` initializer. It passes on Linux, which is where CI runs it.
- Not run: enabled io_uring runtime, ZTS/thread mode, the PHP originals after
  the latest wait/poll, retry, datagram and curl/MongoDB policy moves, and
  performance.

## Gate audit (#481)

All three phases are kept. They compile the pinned and patched C++ bodies against
fakes and inject what a running server can't reliably produce: EAGAIN, EINTR,
partial IO, timer and dispatch failures, cancel races and destructor reentry, all
under ASan/UBSan. Neither a PHPT nor the Rust unit tests reach that C++ glue.

- `hooks-async-policy`, `hooks-packet-policy`: the two fault/order matrices
  above.
- `hooks-compile`: the only C++14 build of the eight patched units.

A body these gates slice may move into Rust: the PR that moves it ports the
gate's cases for it into a `crates/hooks` test and drops the body from the gate.
