# Locks validation

Rust implements mutex, reader/writer, spin, barrier and coroutine locks. PHP Lock, Coroutine Lock,
Thread Lock, Atomic/Long and Thread Atomic/Long are Rust classes (`crates/phplocks`, see
[docs/zend.md](../../../docs/zend.md)) over the same atomic words. See
[native/locks/contract.md](../../../native/locks/contract.md) for geometry,
ownership, timeout, ordering, fork, coroutine and platform contracts.

`overlay.py` reads the pinned sources for the gates' reference side; the hybrid side is
Rust's lock ABI and `Barrier` (whose methods are Rust) over `native/locks/swoole_lock.h`.

```sh
python3 tests/modules/locks/gate.py --source target/upstream/src \
  --archive target/ext/sdk/lib/libswoole_rs_ffi.a --sanitize
```

## Native gate

- `gate.py --source S --archive A [--sanitize] [--rust-asan] [--output R]`
  (`locks-native`): compiles the pinned pthread bodies, the hybrid's Barrier and
  the Rust lock ABI (no lock kind has a C++ class in the hybrid) into one driver (`gate.cc`) and compares thread/fork/RW/timeout/
  barrier/atomic/futex behaviour (about 160,000 checks). `--rust-asan` also
  requires the archive's ASan imports to detect a deliberate one-past pointer
  passed to `swr_v1_locks_coroutine_try`. For a Rust-ASan archive on Linux:

  ```sh
  RUSTC_BOOTSTRAP=1 RUSTFLAGS='-Zsanitizer=address -Zexternal-clangrt=yes' \
    cargo rustc --locked --offline -p swoole-rs-locks --release \
    --target aarch64-unknown-linux-gnu --target-dir target/locks-rust-asan \
    --crate-type staticlib
  ```

- Upstream PHPTs for this module run in the merge tier's PHPT gate (the `phpt-*` jobs), which runs every pinned PHPT on upstream and the all-modules hybrid. To attribute a gate regression to this module, see *Attributing a regression* in [corpus/upstream/README.md](../../../corpus/upstream/README.md).

## Other gates

- `state.py --engine ... [--require-zts]` (`locks-state`): paired PHP witness (`state.php`) for
  integer wrap, CAS, lock behaviour, coroutine mutual exclusion, owner reentrancy,
  nonblocking contention, cancellation, constructor/clone/serialization rules; under ZTS
  also real threads sharing 32/64-bit atomics. Observations must match.
- `constructor.py --source S --archive A --output R [--sanitize] [--rust-asan]` (`locks-constructor`):
  Rust's init for every kind with faults injected at the native pthread init/attribute/
  pshared boundaries; checks nothing is allocated, attribute cleanup and
  first-error propagation through the ABI.
- `platforms.py --source S --output R` (`locks-platforms`): compiles the header as C11/C++14 on the
  host and checks synthetic FreeBSD/Android (and Apple mobile on macOS) macros
  are rejected.

## Ported GoogleTests

`core-tests/src/lock/lock.cpp` no longer builds: the `RWLock`, `SpinLock` and
`CoroutineLock` classes left the build (Swoole\Lock and Coroutine\Lock call the
Rust lock ABI directly). Its 14 TESTs are Rust tests in `crates/locks`
(`cargo test -p swoole-rs-locks`), with the same inputs and expected values, so they
run in the PR tier's `rust` job:

| Upstream TEST | Rust test |
| --- | --- |
| `lock.mutex` | `tests::lock_mutex` |
| `lock.lockwait` | `tests::lock_lockwait` (Linux: without `pthread_mutex_timedlock` the wait polls) |
| `lock.shared` | `tests::lock_shared` |
| `lock.try_rd` | `tests::lock_try_rd` |
| `lock.coroutine_lock` | `coroutine::tests::lock_coroutine_lock` |
| `lock.coroutine_lock_cancel` | `coroutine::tests::lock_coroutine_lock_cancel` |
| `lock.coroutine_lock_rd` | `coroutine::tests::lock_coroutine_lock_rd` |
| `lock.rwlock_shared` | `tests::lock_rwlock_shared` |
| `lock.rwlock` | `tests::lock_rwlock` |
| `lock.rwlock_try_rd` | `tests::lock_rwlock_try_rd` |
| `lock.rw_try_wr` | `tests::lock_rw_try_wr` (barriers in place of upstream's 1 ms/2 ms sleeps, which failed now and then on both engines) |
| `lock.spinlock_shared` | `tests::lock_spinlock_shared` (Linux) |
| `lock.spinlock` | `tests::lock_spinlock` (Linux) |
| `lock.spinlock_try_rd` | `tests::lock_spinlock_try_rd` (Linux) |

The coroutine TESTs run against a scripted host: a contended lock's sleep runs the
other coroutines' steps in upstream's order.
