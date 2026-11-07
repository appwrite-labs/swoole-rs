# Locks validation

Opt-in Rust implementations (feature bit 1024) cover mutex, reader/writer,
spin, barrier and complete coroutine-lock ownership. PHP Lock, Coroutine Lock,
Thread Lock, Atomic/Long and Thread Atomic/Long are Rust classes (`crates/phplocks`, see
[docs/zend.md](../../../docs/zend.md)) over the same atomic words. See
[native/locks/contract.md](../../../native/locks/contract.md) for geometry,
ownership, timeout, ordering, fork and coroutine contracts, and
[PLATFORMS.md](PLATFORMS.md) for the macOS/Linux platform guard.

`overlay.py` applies `migration/patches/locks/` to the pinned sources for the gates;
edit the patches with `tools/patches.py`.

```sh
python3 tools/build_hybrid.py --dev --jobs 2
python3 tests/modules/locks/gate.py --source target/upstream/src \
  --archive target/release/libswoole_rs_ffi.a --sanitize
```

## Native gate

- `gate.py --source S --archive A [--sanitize] [--rust-asan] [--output R]`
  (`locks-native`): compiles the pinned pthread bodies, the hybrid's Mutex and
  Barrier and the Rust lock ABI (RW and spin locks have no C++ class in the hybrid)
  into one driver (`gate.cc`) and compares thread/fork/RW/timeout/
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

- `state.py --engine ... [--require-zts]` (`locks-state`, integrate lane): paired PHP witness (`state.php`) for
  integer wrap, CAS, lock behaviour, coroutine mutual exclusion, owner reentrancy,
  nonblocking contention, cancellation, constructor/clone/serialization rules; under ZTS
  also real threads sharing 32/64-bit atomics. Observations must match.
- `threads.py --engine ... --php-extension sockets`: real ZTS threads with a
  barrier, 20,000 increments per Thread Atomic width and 2,000 mutex-protected
  updates. Requires ZTS engines; also used by `tests/modules/integration/zts.py`.
- `constructor.py --source S --archive A --output R [--sanitize] [--rust-asan]` (`locks-constructor`):
  Mutex constructors (and Rust's init for every kind) with faults injected at the native pthread init/attribute/
  pshared boundaries; checks no leaked storage, attribute cleanup, exception
  code/message and first-error propagation through the direct ABI.
- `platforms.py --source S --output R` (`locks-platforms`): compiles the header as C11/C++14 on the
  host and checks synthetic FreeBSD/Android (and Apple mobile on macOS) macros
  are rejected.
- `zts/Dockerfile` and `zts/build.py`: PHP 8.5 ZTS image and a build of
  reference and Rust thread-enabled extensions inside it (expects the repo at
  `/work`, the pinned upstream at `/reference` and a prior Linux
  `dev-rust` build).

## Known failures

- `constructor.py`: the native baseline leaks on failed construction and is
  reported as `native_baseline_passed: false`; the gate checks the Rust rows.

## Gate audit (#487)

`locks-native` is kept. It is a thread, fork, futex and timeout differential of
the pinned pthread bodies against the Rust overlay under ASan/UBSan, with about
160,000 checks no PHPT reaches.

Three scripts that no phase ran now are phases:

- `locks-constructor`: fault injection at pthread init, attribute and pshared calls.
- `locks-platforms`: the platform guard compile.
- `locks-state`: wrap at `PHP_INT_MAX`, CAS and lock observations, which must
  match upstream.

`owned.py` is deleted. Its ordinary add, sub, cmpset, set and lock round trips
are `swoole_atomic/atomic.phpt` and `swoole_lock/{mutex,lock_read}.phpt`, which
the PHPT gate runs on both engines.

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
