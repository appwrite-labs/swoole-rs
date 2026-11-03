# Locks and atomics ABI

Opt-in feature bit 1024 (`locks`). Header: `swoole_rs_locks.h`. Patched upstream
files: `ext-src/swoole_atomic.cc`, `ext-src/swoole_thread_atomic.cc`,
`ext-src/swoole_thread_lock.cc`, `src/lock/barrier.cc`, `src/lock/coroutine_lock.cc`,
`src/lock/mutex.cc`, `src/lock/rw_lock.cc`, `src/lock/spin_lock.cc`.

Native callers keep public C++/PHP object layout, allocation provenance,
reference counts, exceptions and Zend operations. The module adds no scheduler,
registry or thread. The one Rust heap is ZTS builds' thread resources: a
`Thread\Atomic`, `Thread\Atomic\Long` or `Thread\Lock` keeps its word or private
lock in an `Arc` (upstream's `ThreadResource`), which crates/phpthread's
containers share between threads. No allocator or PHP callback runs under a Rust
frame, and no panic or exception crosses the ABI.

## Mutex, RW and spin locks

`swr_v1_locks_size(kind)` / `swr_v1_locks_align(kind)` describe opaque,
immovable pthread-backed storage. Kinds: RW = 1, mutex = 3, spin = 5.

- Caller storage must have the returned size and alignment, stay at a fixed
  address until destroy, and be initialized exactly once.
- Only a successful init permits lock/unlock/destroy. After destroy the caller
  frees the storage with the allocator that allocated it: shared locks use the
  native shared pool, private locks use `operator new`/`delete`.
- Null, misaligned or unknown kinds are rejected; the caller must still pass a
  valid live allocation.
- Destroying a live/in-use lock, racing destruction and pthread-undefined
  operations are outside the contract.

Mutex/RW attributes, pshared configuration, init, operations, error returns and
destroy run in Rust via libc. On Linux, timed pthread entry points are selected
by caller capability bits (mutex = 1, RW = 2); macOS and configurations without
them keep the pinned `sw_wait_for` attempt/usleep/backoff order. `LOCK_NB` takes
precedence over a positive timeout; RW `LOCK_SH` takes precedence over exclusive.
Timeout <= 0 blocks. Syscall and pthread errors are returned as-is. Constructor
failure still throws the native `system_error` with upstream errno/string.

Deliberate fixes relative to upstream:

- On pthread lock-init failure Rust destroys the initialized attribute
  (preserving errno); native then frees the storage and throws the same error.
  Upstream leaked the attribute and allocation.
- Attribute init and shared-attribute setup return their first error instead of
  continuing into lock init. Failed setup releases the initialized attribute.

## Barrier

Barrier class layout is unchanged. Linux borrows the native barrier and attr
storage with runtime size/alignment checks and uses libc pthread
init/wait/destroy. Shared attrs are initialized before `setpshared`, fixing
upstream `Barrier::init`'s use of an uninitialized attribute. Non-pthread
barriers keep the original one-shot arrival/count words, 10 ms polling and fence;
fallback words must be aligned native `u32` storage.

Supported platforms: macOS and Linux. The crate and header reject other OSes
(including Android and Apple mobile) at compile time; see
`tests/modules/locks/PLATFORMS.md`.

## Atomics

`swr_v1_locks_atomic32` / `swr_v1_locks_atomic64` operate on borrowed, aligned,
shared native words. Opcodes: load = 0, store = 1, add-fetch = 2, sub-fetch = 3,
compare-set = 4; `expected` is the last argument. Loads/stores are relaxed
(matching native volatile access); RMW/CAS use seq-cst like `__sync`. Arithmetic
wraps at `u32`/`i64` width. CAS returns 0/1 and leaves the word unchanged on
failure. Callers keep the storage live and exclude concurrent destruction.

PHP Atomic, Atomic\Long and their Thread counterparts are Rust classes
(`crates/phplocks`, `swr_v1_phplocks_*`). C++ supplies the global-pool words
(Atomic) and the `ThreadResource`s that hold Thread words; Rust owns the method
tables (over `crates/phpstubs`' arginfo), registration, objects and methods
(see `docs/zend.md`).

`swr_v1_locks_wait` / `swr_v1_locks_wakeup` exist on Linux only and implement
the `HAVE_FUTEX` path: CAS token consumption, a single futex syscall (no hidden
EINTR/spurious-wake retry), timeout conversion and result/errno. NaN/out-of-range
timeout casts are undefined input. PHP Atomic and its Thread counterpart call
them directly; non-futex builds use upstream's C++ wait registry.

## Coroutine lock policy

`src/lock/coroutine_lock.cc` uses `swr_v1_locks_coroutine_try`,
`swr_v1_locks_coroutine_release` and `swr_v1_locks_coroutine_backoff`. The
kernel receives a borrowed atomic word and copied opaque owner cookie/CID values,
never dereferences a Coroutine, and returns reentrant/acquired/busy/wait. The
native facade snapshots the one-time owner check before the wait loop, gets the
current context and emits warnings outside Rust, and publishes owner/CID after
the Rust CAS. Sleeps, cancellation, io_uring wait/wake and errno keep original
order. Rust decides release and reset (no owner check or recursion counter is
added); native zeroes owner/CID and calls io_uring after Rust returns. Backoff:
1 ms doubling, max 100 ms, then wrap to 1 ms. The scheduler, PHP facade and
runtime file/flock hooks stay native.

## Known failures

- Original Linux GoogleTest `lock.rw_try_wr` (line 233) fails in both engines:
  try-write returned 0 after the test's 1 ms/2 ms usleep arrangement.
- macOS `coroutine_lock/lock.phpt` failed once with reply order
  lock2,value3,lock1 instead of value3,lock1,lock2, before the coroutine lock
  policy was converted; PHP's runner passed it on retry.
