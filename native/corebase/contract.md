# Core base helpers

Feature bit 8192 (`corebase`). Header: `swoole_rs_corebase.h`. `src/core/base.cc`
and `src/core/misc.cc` are gone: what stable Rust can't express of them (the
globals, C++ object lifetimes and the std::string/std::function APIs) is
`native/corebase/globals.cc`. Other callers are the readers of the `SwooleG` fields Rust owns. No
public C++ signature changes; `swoole::Global` loses the fields Rust owns.

Rust holds numeric/byte/path comparison and formatting policy. Apart from the
core state record below it keeps no state, pointers or borrows after return,
performs no allocation, and calls no
native allocator, user callback, Zend operation or C++ constructor. Host libc
`strtoul`, `strncasecmp` and `toupper` are called directly so locale and errno
behave as upstream; input strings must stay valid for the call. No exception,
bailout or panic crosses the ABI. All spans and pointer differences fit
`PTRDIFF_MAX`. Native `long`/`unsigned long` map to Rust `c_long`/`c_ulong`.

## Helpers

- `type_size`: original ASCII character mapping; unknown returns 0.
- `swoole_common_divisor` / `_multiple`: Rust asserts both inputs are positive,
  as upstream does (a failed assertion aborts). LCM wraps the `uint32` product
  before division (65536*65536 -> 0). Rust uses Euclid's remainder loop.
- `itoa`: writes decimal bytes then NUL into caller-owned storage and returns the
  length. Handles `LONG_MIN`. Buffers need only fit the
  actual value.
- `swoole_dec2hex`: asserts base 2..36, formats backwards into scratch of
  `sizeof(unsigned long)*8+1` bytes and returns a libc `strndup` of the digits.
- `hex`: accepts all ASCII A..Z/a..z (as upstream, beyond F). Counts an optional
  `0x` prefix even without digits, wraps `size_t` per digit, then converts to
  `unsigned long`. The facade passes its compiler's plain-`char` signedness
  (`hex_mode`); in unsigned mode all bytes go to `toupper` as non-negative.
  Output must be writable and disjoint from input.
- `swoole_version_compare`: calls host `strtoul` left then right on every component, keeping
  sticky errno/ERANGE and platform differences. Skips exactly one arbitrary
  separator between equal components.
- `parse_stoi`: host decimal `strtol`, accepted only within `int` range. The
  facade first builds the same `std::string` upstream's `std::stoi` needed (same
  `bad_alloc` point), saves and clears errno, captures ERANGE and restores errno.
  Output is written only on success; on no-conversion or out-of-range it reruns
  native `std::stoi` so the original exception is thrown.
- The global hook table (`hook.rs`) is Rust's: `swoole_add_hook`,
  `swoole_call_hook` (over the list as the call began) and `swoole_isset_hook`.

## `mkdir_next`

Bounded path copying and component iteration. The native stack owns a
zero-initialized two-`size_t` state and a `PATH_MAX` scratch buffer. Return
values: 1 = next NUL-cut prefix is ready for native `access(R_OK)`/`mkdir(0755)`
(the next call restores the slash and continues); 0 = done; 2 = emit the original
too-long warning; 3 = invalid state/storage. Input, output and state must be
disjoint; state/output must not change between steps.

Order matches upstream: raw `std::string` length check first, then copy up to the
first NUL, with the trailing-slash decision taken from the last raw byte, so
embedded-NUL paths keep upstream's odd behavior. Repeated slashes and
relative/root prefixes are not normalized. Empty input and a full `PATH_MAX-1`
string needing an appended slash are undefined upstream; Rust rejects them.

## Lifecycles (`lifecycle.rs`)

Rust defines `swoole_thread_init`, `swoole_thread_clean`, `swoole_is_main_thread`,
`swoole_clean` and `swoole_fork` under their C++ symbols, and `swoole_fork_exec`'s
fork (`swr_v1_core_fork_exec`). Each C++ effect (`SwooleG = {}`, an object's
`new`/`delete`, the timer, event, signal and coroutine leaves) is an entry of the
host `swoole_init` registers (`swr_v1_core_native_host`); before that the
sequences do nothing and `swoole_fork` returns -1.

- Clean releases the hook lists, then: clear signals, clean the thread,
  close the logger when there is one, delete the logger and memory pool, and
  reset Global last.
- Thread init allocates the stack buffer when absent, blocks signals only for
  non-main threads, then publishes `main_thread`. Thread clean frees the timer,
  then the reactor, then the buffer, each when present.
- Fork: PRECHECK returns before fork; child DAEMON returns before the timer and
  overrides EXEC. A failed fork warns, sleeps a second and retries. Fork-exec
  failure returns `pid_t(0)` without warning or retry; the child callback runs
  before `exit(0)`. A returning fatal handler resumes the sequence.

## Core state (`global.rs`)

Rust owns `SwooleG`'s scalars in one process-wide record of atomics: the init
guard, `cpu_num`, `dns_tries`, the flags (`running`, `enable_signalfd`,
`enable_kqueue`, `enable_coroutine`, `print_backtrace_on_error`, `wait_signal`,
`dns_lookup_random`, `signal_alarm`), the counters and limits (`current_task_id`,
`signal_fd`, `signal_listener_num`, `signal_async_listener_num`, `trace_flags`,
`pagesize`, `max_file_content`, the AIO and io_uring sizes and times), the
bug-report text, and native pointers Rust only holds (memory pool, stdout,
logger, server, AIO pipe socket, the fatal-error and backtrace hooks). Each has a
`swr_v1_core_<field>`/`swr_v1_core_set_<field>` pair. `swoole::Global` keeps the
member names as `swoole::RustField` views over those pairs, so upstream's
`SwooleG.field` code (other modules' patched lines and the upstream core-tests
alike) compiles unchanged. A view can't be copied, so passing one through `...`
fails to compile instead of printing garbage. `SwooleG = {}` leaves the record
alone; `swr_v1_core_reset` clears it. The allocator, limits and task/resolver
paths are `config_state.rs`; the containers are C++ objects.

`swr_v1_core_init` is `swoole_init`'s guard: false when already initialised,
otherwise it resets the record and sets running, coroutines on, the page size,
`SW_MAX_FILE_CONTENT`, `SW_MAX(1, sysconf(...))` CPUs, one DNS try and the
`srandom(time(nullptr))` seed; `swoole_init` keeps the native effects in
upstream's order. `swr_v1_core_bug_report_init` writes base.cc's bug-report
banner, `uname` and compiler lines around the TLS line native code passes.
`swr_v1_core_max_sockets` is the `RLIMIT_NOFILE` soft limit as `uint32_t`,
capped at `SW_SESSION_LIST_SIZE`, or -1 with errno kept. `swoole_clean` resets
the record after the native `Global`.

Upstream's own C++ symbols defined in Rust (`export_name`): `swoole_common_divisor`/`_multiple`, `swoole_dec2hex`,
`swoole_version_compare`, `swoole_dump_ascii`/`_hex`, `swoole::microtime`,
`swoole_set_trace_flags`, `swoole_set_print_backtrace_on_error`, `sw_logger`,
`swoole_set_stdout_stream`/`swoole_get_stdout_stream` (`FILE` mangles as its libc's
struct tag), `swoole_hex2dec` (in the target's plain-char mode), `swoole_dump_bin`
(over crates/framing's unpack),
`DataHead::dump`, both `swoole_rand` overloads, `swoole_random_int`, and
`swoole_version`/`swoole_version_id`/`swoole_api_version_id` (C names), and
`sw_malloc`/`sw_free`/`sw_calloc`/`sw_realloc`, which call libc's allocator
directly. The `corebase-native` gate compares them with the pinned `base.cc`.

Whole operations in Rust (`host.rs`), with native code keeping only their
warnings: `swoole_shell_exec`, `swoole_dump_ascii`/`_hex`, `swoole_print_backtrace` without Boost,
`microtime`, `DataHead::dump`, `swoole_rand`/`swoole_random_int` (a per-thread
xoshiro256** seeded by getentropy), `swoole_system_random` and
`swoole_redirect_stdout(int)` (each dup2's errno is handed back for its warning).

`sw_snprintf`, `sw_vsnprintf` and `sw_printf` are C variadics, so they're a C
shim in the Rust archive (`crates/corebase/src/format.c`, C linkage).

Still native: the rest of `SwooleG` and TLS, C++ containers/objects, filesystem IO
and other runtime operations.

The lower `swoole::Thread` (`native/corebase/swoole_thread.h`; upstream's header is
deleted) owns a generated `CoreThread` record: Rust controls
its pthread handle, status publication, moves, joins and detaches. pthread starts
the native callable trampoline directly, so C++ exceptions and pthread forced
unwind do not cross a Rust child-entry frame. The cancellable `pthread_join` and
`std::system_error` run in native code after Rust returns.
