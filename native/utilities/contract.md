# Utilities ABI

Feature bit 512 (`utilities`). Patches `include/swoole.h`; this directory's
`swoole_log.h`, `swoole_hash.h` and `swoole_version.h` replace
upstream's and keep what C++ and the utilities gates use; nothing built includes upstream's
`swoole_util.h`. `swoole_log.h` keeps the levels, the fatal-error forwarder and the
compiled-off trace macro: no C++ left logs through `swoole::Logger` or the other log macros. Nothing
C++ calls is declared here: the spinlock and the futex wait/wake are Rust's, called by path. Tests:
`tests/modules/utilities/README.md`. Depends on the locks module for wait/wake.

## General rules

- Supported hosts are LP64 macOS/Linux.
- No C++ object layout, Rust reference, allocator or PHP callback crosses the v1
  ABI. Borrowed byte ranges are valid for their stated length; outputs are aligned,
  writable and disjoint from inputs. Null input is allowed only at length 0.
  Lengths above `isize::MAX` are rejected. No borrow survives a call.
- Callers pass host `char` signedness explicitly (`signed_char`), so
  `-funsigned-char` builds keep their arithmetic and ctype promotions.
- ctype/libc functions (`tolower`, `isspace`, `strncasecmp`, `memchr`, `memcmp`)
  are the host's, so locale behavior is unchanged. Invalid negative ctype arguments
  are undefined upstream and outside parity.

## Hashing

- `swoole_hash_php`/`_jenkins`/`_austin` (`crates/utilities/src/misc.rs`): 64-bit
  wrapping DJBX33A, Jenkins (original u32/u32/u64 mix and truncated block counter),
  Austin (64-bit state, signed-int promotions), promoting the host `char`. Austin has
  undefined negative left shifts on high bytes upstream; only ASCII Austin input is
  defined parity, Rust wraps deterministically otherwise.
## Concurrency primitives

- `swr_v1_utils_spinlock(&AtomicU32, cpus, backoff_limit)`: SeqCst compare-exchange like
  native `__sync`, relaxed reads, `spin_loop` backoff, OS thread-yield on exhaustion.
  Caller unlocks with its existing native release store. Backoff multiplication is
  checked. Null/unaligned pointers return immediately.
- `sw_atomic_futex_wait` / `sw_atomic_futex_wakeup` (`crates/utilities/src/misc.rs`):
  the locks module's futex wait where `HAVE_FUTEX` is set (Linux). Other builds use `wait_registry.rs`'s `registry_wait`/`_wakeup` over a Rust registry
  that `sw_atomic_wait_init` adopts from the shared memory pool before any fork.
  Caller owns aligned process-shared storage and its lifetime. errno is
  preserved. Timeouts outside the positive `time_t` range are undefined upstream.

## Logger

### Formatting

The logger's level clamp (0..6), labels (unknown levels are INFO), rotation
formats (unknown modes, SINGLE included, are daily), `put` line and `<.%lld>`
microsecond suffix are Rust-only (`crates/utilities/src/lib.rs`), with
`sw_snprintf`'s truncation: at most capacity-1 bytes plus NUL, the full length at
capacity 0.

### Logger

The logger is Rust's (`crates/utilities/src/logger.rs`); no C++ handle on it is left.
Every operation is under one lock (the level is an atomic, so `level` takes no lock).
Diagnostics go through corebase's log host under upstream's method names; worker
identity and `swoole_redirect_stdout` call the process's `LoggerHost` (in
`crates/process`). Both run after the lock is released, so a diagnostic can log
through the same logger. The process logger (`SwooleG.logger`) is `crates/process`'s
(`logger_op`'s `LOGGER`, which corebase's `swr_v1_core_logger` points at), and a log
Rust emits runs the macros' semantics in `swr_v1_utils_log`: the last error, the level
check, the line in `sw_error`, the put and, for `swoole_error`/`swoole_sys_error`, the
exit. A failed open warns as upstream's `Logger::open` did; a failed open during
`put`'s rotation, which deadlocked upstream, warns after the line is written.

`swr_v1_logger_put(logger, level, content, length)` is the one export: crates/ffi's
`fatal.c` declares it itself and passes the process logger.
