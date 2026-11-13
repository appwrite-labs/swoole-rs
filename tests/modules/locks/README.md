# Locks validation

Rust implements mutex, reader/writer, spin, barrier and coroutine locks. PHP Lock, Coroutine Lock,
Thread Lock, Atomic/Long and Thread Atomic/Long are Rust classes (`crates/phplocks`, see
[docs/zend.md](../../../docs/zend.md)) over the same atomic words. See
[native/locks/contract.md](../../../native/locks/contract.md) for geometry,
ownership, timeout, ordering, fork, coroutine and platform contracts.

`overlay.py` names the pin for `platforms.py`'s report.

## Rust gates

`gate.rs` and `constructor.rs` (the `gate` and `constructor` tests of `swoole-rs-locks`,
so they run in the PR tier's `rust` job and `locks-unit`) are the former `gate.cc` and
`constructor.cc`, check for check. Nothing in C or C++ called the lock ABI they drove
(`swr_v1_locks_size`, `_align`, `_init`, `_lock`, `_unlock`, `_destroy`,
`_coroutine_try`, `_coroutine_release`), so those exports are gone and each check runs
against the safe type that was the export's body. The reference side is the pinned
pthread bodies and the gate's own driver in `crates/oracle` (`shim/locks.cc`), built with
what configure finds on the target.

| `gate.cc` check | Rust test (`gate.rs`) | Export → safe API |
| --- | --- | --- |
| `swr_v1_locks_size`/`_align` of kinds 3 and 1 against `sizeof`/`alignof` the pinned `MutexImpl` and `RWLockImpl` | `layout` | `swr_v1_locks_size(kind)` → `Lock::words(kind)` words of `AtomicU64`, and `SharedMutex` itself |
| `entry::exercise() == exercise<RustLock<3>, RustLock<1>, swoole::Barrier>()`: four threads' 8,000 locked increments, try-locks, 5 ms and 10 ms timed locks, zero and negative timeouts, rwlock readers and writer, a shared mutex across fork, a four-party barrier | `exercise_matches_the_pinned_locks` | `swr_v1_locks_init`/`_lock`/`_unlock`/`_destroy` → `Lock::private`, `Lock::shared`, `Lock::lock`, `Lock::unlock`, drop |
| `entry::spin()` against `spin<RustLock<5>>()` (Linux) | `spin_matches_the_pinned_spinlock` | as above, kind 5 |
| `swr_v1_locks_size(99) == 0`, `swr_v1_locks_init(nullptr, 3, 0) == EINVAL` | `unknown_kinds_and_missing_storage_are_refused` | `Lock::words(99) == 0`, `Lock::private(99)` and `Lock::shared` with no storage make no lock |
| `shared_barrier<swoole::Barrier>()`, and the pinned one where it is the word barrier | `shared_barrier` | unchanged (`barrier_init`, `barrier_wait`, `barrier_destroy`) |
| `atomics()`: 20,000 rounds against the `__sync` builtins, four threads' 80,000 increments, a null word | `atomics` | unchanged (`swr_v1_locks_atomic32`, `swr_v1_locks_atomic64`) |
| `wait_wake()`: the pinned futex wait and Rust's on Linux, the pinned registry elsewhere | `wait_and_wake` | unchanged (`swr_v1_locks_wait`, `swr_v1_locks_wakeup`) |
| `coroutine_policy()`: acquire, reentry, busy, wait, release twice, reentry over a zeroed word, the backoff sequence | `coroutine_policy` | `swr_v1_locks_coroutine_try`/`_release` → `CoroutineLock::acquire`/`unlock` over a host whose sleep is cancelled; acquired and reentrant are code 0, busy `EBUSY`, wait one sleep |

Checks of the exports' own argument validation that have no safe form, because the
type system now refuses the argument: a null or misaligned storage pointer
(`swr_v1_locks_init(nullptr, ..)`, and crates/locks' former
`lock_and_unlock_refuse_null_misaligned_and_unknown_storage`), and a coroutine distinguished
by its cookie under the same id (`swr_v1_locks_coroutine_try(&word, 42, 7, 43, 7, 1) == 3`):
`CoroutineLock` knows a coroutine by its id alone, so the test waits as another id. The
gate's `--rust-asan` probe (a one-past word passed to `swr_v1_locks_coroutine_try`) went
with the pointer parameter.

| `constructor.cc` row | Rust test (`constructor.rs`) | Export → safe API |
| --- | --- | --- |
| `direct(kind, shared, stage)` for kinds 1 and 3, private and shared, faulted at the lock's init (0), the attribute's init (1) and, shared, its pshared setting (2); kind 5 at its init on Linux | `every_kind_reports_a_faulted_init_and_cleans_up` | `swr_v1_locks_init` → `Lock::private` and `Lock::shared`, which make no lock; errno and the call counts are as before. A private lock allocates and frees its own storage once (the ABI's caller supplied it, so the gate counted none) |
| the rows' result (`ENOMEM`, `EAGAIN`, `ENOTSUP`) | `a_shared_mutex_returns_the_first_error` | `SharedMutex::init`, the constructor that returns the error; `Lock`'s return `None` |

The test binary defines the pthread init, attribute and pshared symbols itself, as the
gate did, and passes through to libc unless the calling thread is watching.

## Other gates

- `state.py --engine ... [--require-zts]` (`locks-state`): paired PHP witness (`state.php`) for
  integer wrap, CAS, lock behaviour, coroutine mutual exclusion, owner reentrancy,
  nonblocking contention, cancellation, constructor/clone/serialization rules; under ZTS
  also real threads sharing 32/64-bit atomics. Observations must match.
- `platforms.py --source S --output R` (`locks-platforms`): compiles the header as C11/C++14 on the
  host and checks synthetic FreeBSD/Android (and Apple mobile on macOS) macros
  are rejected.
- Upstream PHPTs for this module run in the merge tier's PHPT gate (the `phpt-*` jobs), which runs every pinned PHPT on upstream and the Cargo build. To attribute a gate regression to this module, see *Attributing a regression* in [corpus/upstream/README.md](../../../corpus/upstream/README.md).

## Ported GoogleTests

`core-tests/src/lock/lock.cpp` no longer builds: the `Mutex`, `RWLock`, `SpinLock` and
`CoroutineLock` classes left the build (Swoole\Lock and Coroutine\Lock are crates/locks'
`Lock` and `CoroutineLock`). Its 14 TESTs are Rust tests in `crates/locks`
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
