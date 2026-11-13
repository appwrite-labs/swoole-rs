# Utilities ABI

Feature bit 512 (`utilities`). Patches `include/swoole.h`; this directory's
`swoole_log.h`, `swoole_hash.h` and `swoole_version.h` replace
upstream's and keep what C++ and the utilities gates use; nothing built includes upstream's
`swoole_util.h`. Declarations are in
[swoole_rs_utilities.h](swoole_rs_utilities.h). Tests:
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
- Step functions use a caller-local `uint32_t` phase initialized to 0. Rust returns
  an action; native code performs it and calls again with refreshed facts. All IO,
  allocation, warnings and field writes happen natively between steps. Null or
  unknown phase returns INVALID without reading facts.

## Hashing and logging helpers

- `swoole_hash_php`/`_jenkins`/`_austin` (`crates/utilities/src/misc.rs`): 64-bit
  wrapping DJBX33A, Jenkins (original u32/u32/u64 mix and truncated block counter),
  Austin (64-bit state, signed-int promotions), promoting the host `char`. Austin has
  undefined negative left shifts on high bytes upstream; only ASCII Austin input is
  defined parity, Rust wraps deterministically otherwise.
- `swr_v1_utils_pretty_range`: offset/length for `Logger::get_pretty_name`. Never
  writes input. Invalid outputs/ranges return -1; outputs are zeroed first; no
  bracket returns an empty span. Native builds the `std::string` afterwards.
## Concurrency primitives

- `swr_v1_utils_spinlock(u32 *, cpus, backoff_limit)`: SeqCst compare-exchange like
  native `__sync`, relaxed reads, `spin_loop` backoff, OS thread-yield on exhaustion.
  Caller unlocks with its existing native release store. Backoff multiplication is
  checked. Null/unaligned pointers return immediately.
- `swr_v1_utils_wait(u32 *, timeout)` / `swr_v1_utils_wakeup(u32 *, n)`:
  the locks module's futex wait, called by `sw_atomic_futex_wait`
  (`crates/utilities/src/misc.rs`) only when `HAVE_FUTEX` is set. Other builds use `wait_registry.rs`'s `registry_wait`/`_wakeup` over a Rust registry
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

### Logger (`swr_v1_logger_*`)

`swoole::Logger` holds one `SwrLogger` handle; every operation is Rust's, under one
lock (the level is an atomic, so `get_level` takes no lock). Diagnostics go
through corebase's log host under upstream's method names; worker identity,
`swoole_redirect_stdout` and backtraces call `swr_logger_host` (Rust, in `crates/process`).
Both run after the lock is released, so a diagnostic can log through the same
logger. The process logger (`SwooleG.logger`) is `crates/process`'s
(`logger_op`'s `LOGGER`, which corebase's `swr_v1_core_logger` points at), and a log Rust emits runs the macros' semantics in
`swr_v1_utils_log`: the last error, the level check, the line in `sw_error`, the
put and, for `swoole_error`/`swoole_sys_error`, the exit. A failed open warns as upstream's
`Logger::open` did; a failed open during `put`'s rotation, which deadlocked
upstream, warns after the line is written. `get_file`/`get_real_file` pointers are
valid until the file next changes.

## Logging macros (v2)

`swoole_log.h` macros call two whole-program exports. Contexts are caller
stack PODs, uniquely borrowed for one step. No allocation, registry or Logger layout
change. `fatal_error` is the original forwarder.

`swr_v2_utils_log_frontdoor_step(ctx, operation, first, second, unsigned_width, high)`
returns `SwrLogAction {action, level}`.

- Operations 0..9: info, notice, warning, debug, trace, sys_notice, sys_warning,
  error_log, error, sys_error.
- Actions 0..7: done, publish, threshold, mask, ignored, format, put, exit. Facts
  answer only the preceding action.
- `unsigned_width`: 0 signed, 32/64 usual unsigned comparison, 128/129 for
  unsigned/signed 128-bit (with `high`), 256 a host floating `>=` result, 257 a
  completed user-type condition.

Operand rules: arithmetic and enum operands are marshalled with the usual integer
conversions; floating operands are compared with host C++ `>=` before entering Rust
(no float-to-int conversion). User-defined comparison types and enums with ADL
`operator>=` keep the exact original conditional expression natively; Rust consumes
only its truth value. Class/noncopyable operands are never copied. All argument
expressions, including `__func__`/`__PRETTY_FUNCTION__`/`__SW_FUNC__`, run in the
caller's scope; there are no lambdas or callbacks. For `error_log`, the error is
evaluated at publication, ignored query and formatting separately, and the level at
admission and output separately.

Bare-if macros stay bare `if` statements: the guard runs the pure program up to its
FORMAT or MASK request, the format expression stays in the caller, and a fresh
context resumes at phase 4 for output, so `else` binding is preserved. Other macros
keep `do/while`. Compiled-off debug/trace macros evaluate nothing.

`swr_v2_utils_log_hex_step(ctx, length, count)` owns row iteration, offsets,
per-byte effects, printability and header/footer decisions. Actions: 0 done,
1 header, 2 hex format, 3 ctype, 4 text byte, 5 text dot, 6 row, 7 footer. Native
`sprintf` counts feed unsigned wrapping accumulators. Data and length are captured
once. Each byte is re-read for hex format, for ctype, and (if printable) for text.

Unqualified: late non-ADL or block-scope enum operator lookup, unparenthesized macro
arguments that change precedence (bare assignment/ternary), numeric argument
evaluation order (compiler-specific), GNU statement expressions, and actual exit
behavior. On Linux, an ASan-instrumented utility archive cannot be loaded into PHP
because PHP uses `RTLD_DEEPBIND`; PHP-level tests use a plain archive.
