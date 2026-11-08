# Locks and atomics ABI

Feature bit 1024 (`locks`). Header: `swoole_rs_locks.h`. No lock has a C++ class: the
server, Table and crates/phplocks call this ABI, and `swoole::Barrier`'s methods (declared in
this directory's `swoole_lock.h`) are Rust under their C++ symbols. The module has no C or C++
source. crates/phplocks is `Swoole\Atomic`, `Swoole\Lock`,
`Coroutine\Lock` and the ZTS `Thread\Atomic`, `Atomic\Long` and `Thread\Lock`,
MINITs included. A thread resource keeps its word or private lock in an `Arc`
(upstream's `ThreadResource`), which crates/phpthread's containers share between
threads. No panic or C++ exception crosses the ABI.

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
Timeout <= 0 blocks. Syscall and pthread errors are returned as-is; init failure
returns the error (upstream's `Mutex` constructor threw it as a `system_error`).

Deliberate fixes relative to upstream:

- On pthread lock-init failure Rust destroys the initialized attribute
  (preserving errno) and the caller frees the storage. Upstream leaked the
  attribute and allocation.
- Attribute init and shared-attribute setup return their first error instead of
  continuing into lock init. Failed setup releases the initialized attribute.

## Barrier

Barrier class layout is unchanged (`crates/locks/src/lib.rs`'s `Barrier` is the same
`repr(C)` record). Linux uses libc pthread init/wait/destroy on its barrier and attr,
as upstream's `SW_USE_PTHREAD_BARRIER`. Shared attrs are initialized before `setpshared`, fixing
upstream `Barrier::init`'s use of an uninitialized attribute. Non-pthread
barriers keep the original one-shot arrival/count words, 10 ms polling and fence;
fallback words must be aligned native `u32` storage.

Supported platforms: macOS and Linux. The crate and header reject other OSes
(including Android and Apple mobile) at compile time
(`tests/modules/locks/platforms.py`).

## Atomics

`swr_v1_locks_atomic32` / `swr_v1_locks_atomic64` operate on borrowed, aligned,
shared native words. Opcodes: load = 0, store = 1, add-fetch = 2, sub-fetch = 3,
compare-set = 4; `expected` is the last argument. Loads/stores are relaxed
(matching native volatile access); RMW/CAS use seq-cst like `__sync`. Arithmetic
wraps at `u32`/`i64` width. CAS returns 0/1 and leaves the word unchanged on
failure. Callers keep the storage live and exclude concurrent destruction.

`swr_v1_locks_wait` / `swr_v1_locks_wakeup` exist on Linux only and implement
the `HAVE_FUTEX` path: CAS token consumption, a single futex syscall (no hidden
EINTR/spurious-wake retry), timeout conversion and result/errno. NaN/out-of-range
timeout casts are undefined input. PHP Atomic and its Thread counterpart call
them directly; other hosts use crates/utilities' wait registry.

## Coroutine lock

`crates/locks/src/coroutine.rs` owns the lock record, its shared or private
atomic word, owner cookie/CID, acquisition loop, cancellation and unlock. Shared
words come from the process-shared pool and return to it on destruction.
Ownership metadata belongs to one cooperative scheduler thread; only the word may
be shared between processes. An immutable host (crates/phplocks) supplies the
current coroutine, shared pool allocation, sleep or io_uring futex wait/wake, and
the warning. The one-time reentrant check, seq-cst acquisition, nonblocking
EBUSY, cancellation code, errno propagation and release without an owner check
preserve upstream order. Backoff is 1 ms doubling to 100 ms, then wraps to 1 ms.
