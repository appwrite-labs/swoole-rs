# Core base helpers

Opt-in feature bit 8192 (`corebase`). Header: `swoole_rs_corebase.h`. Callers are
in `src/core/base.cc` and the readers of the `SwooleG` fields Rust owns. No
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
- `dirname`: returns the last-slash prefix length, preserving root and
  binary/NUL bytes; no slash returns empty.
- `hook_add_action`: stateless decision for hook-list creation and front/back
  insertion. Native allocates and mutates the list and owns callbacks.

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

## `lifecycle_next`

Returns typed native actions from a zeroed 8-byte caller-stack phase/index
cursor. Each reentrant call uses its own cursor. Invalid cursor/operation returns
`UINT32_MAX`, which the native shim aborts on; callers zero-initialize and stop
on DONE.

| Operation | Meaning | Argument |
| --- | --- | --- |
| 1 | thread init | is main thread |
| 2 | thread cleanup | - |
| 3 | Global cleanup | native hook count |
| 4 | `swoole_fork` | - |
| 5 | `swoole_fork_exec` | - |

Native shims perform exactly one requested effect per step.

- Global cleanup observes hook slots one at a time (a destructor may add a later
  list before its observation point), keeps deleting-list pointers published and
  never revisits a deleted slot. Then: clear signals, clean the thread,
  conditionally close the current logger, delete the logger and memory pool, and
  reset Global last. Pointers are re-read after each close/delete.
- Thread init: observe the stack buffer, allocate when absent, block signals only
  for non-main threads, then publish `main_thread`. An allocation exception stops
  later effects.
- Thread cleanup: observe/free timer, then reactor, then observe/delete/null the
  buffer.
- Fork: native flags are checked with `static_assert`. After the fork effect the
  copied observation is 0 = failure, 1 = child, 2 = parent; Rust keeps no pid.
  PRECHECK returns before fork; child DAEMON returns before timer observation and
  overrides EXEC. Failed fork warns, sleeps once, then retries. Fork-exec failure
  returns `pid_t(0)` without warning or retry; the child callback runs before
  native `exit(0)`. A returning fatal handler resumes the sequence; throwing or
  exiting handlers never re-enter Rust.

## Core state (`global.rs`)

Rust owns these `SwooleG` fields in one process-wide record of atomics: the init
guard, `cpu_num`, `wait_signal`, `dns_lookup_random`, `dns_tries`, the memory
pool and the stdout stream (the last two are native pointers Rust only holds).
Native code reads them through `swr_v1_core_*` accessors; `SW_CPU_NUM`,
`sw_mem_pool()` and `swoole_get_stdout_stream()` keep their names.
`swr_v1_core_init` is `swoole_init`'s guard: false when already initialised,
otherwise it resets the record and sets `SW_MAX(1, sysconf(...))` CPUs, one DNS
try and the `srandom(time(nullptr))` seed; `swoole_init` keeps the native effects
in upstream's order. `swr_v1_core_max_sockets` is the `RLIMIT_NOFILE` soft limit
as `uint32_t`, capped at `SW_SESSION_LIST_SIZE`, or -1 with errno kept.
`swoole_clean` resets the record after the native `Global`. The unused `uname`,
`use_async_resolver` and `use_name_resolver` fields are gone.

Upstream's own C++ symbols defined in Rust (`export_name`), with no body left in
base.cc: `swoole_common_divisor`/`_multiple`, `swoole_dec2hex`,
`swoole_version_compare`, `swoole_dump_ascii`/`_hex`, `swoole::microtime`,
`DataHead::dump`, both `swoole_rand` overloads, `swoole_random_int`, and
`sw_malloc`/`sw_free`/`sw_calloc`/`sw_realloc`, which call libc's allocator
directly (`SwooleG.std_allocator` only ever held it). The `corebase-native` gate
checks the hybrid base.o leaves the first four to the Rust archive.

Whole operations in Rust (`host.rs`), with native code keeping only their
warnings: `swoole_shell_exec`, `sw_wait_for` (through a non-throwing predicate
trampoline), `swoole_dump_ascii`/`_hex`, `swoole_print_backtrace` without Boost,
`microtime`, `DataHead::dump`, `swoole_rand`/`swoole_random_int` (a per-thread
xoshiro256** seeded by getentropy), `swoole_system_random` and
`swoole_redirect_stdout(int)` (each dup2's errno is handed back for its warning).

Still native: varargs, the rest of `SwooleG` and TLS, C++ containers/objects, filesystem IO, hook
traversal and other runtime operations.
