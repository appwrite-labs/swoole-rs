# Error message boundary (M11)

## Message lookup

- `swr_v1_error_message(int32_t)` returns one of 130 known messages as an
  immutable, process-lifetime C string, or null for any other code. It allocates
  nothing and has no mutable state.
- `swr_v1_error_resolve(code, formatter)` returns the same pointer for a known
  code. On a miss it calls the optional formatter synchronously, exactly once,
  with only the input integer, and returns its result. A null formatter returns
  null on a miss. The formatter must not throw, unwind, call Zend/PHP, bail out
  or retain state. It owns the bytes it returns; Rust neither reads nor frees them.
- `swr_v1_error_strerror` is `swoole_strerror`'s body. Rust classifies the
  code, does the known lookup and formats unknown codes. Codes below the pinned
  500 boundary go to the native formatter, which calls system `strerror`.

Unknown messages are formatted without allocating into each thread's 32-byte
buffer. Byte layout matches the native `snprintf` buffer: Apple colon spelling,
signed decimal, NUL position, retained trailing bytes and, within a thread,
pointer reuse. A thread's next unknown code overwrites its message.
Known-message lookups are safe to run concurrently.

Known limitation: known-error throughput was measured at 0.59x upstream, and a
later macOS measurement at 0.82x. The regression is unresolved; the current
known/system/unknown paths have not been re-measured.

## Ignored-error set

`swoole_rs_error_state.h` declares `swr_v1_error_ignore`,
`swr_v1_error_is_ignored`, `swr_v1_error_ignored_release` and
`swr_v1_error_ignore_global`. The process-global set behind `swoole_ignore_error`
and `swoole_is_ignored_error` is a Rust static that lazily owns an opaque
`HashSet<i32>`; null is the empty initial state.

- Insert is idempotent and accepts any int32. If allocation fails, the existing
  set is kept, a failed first insert leaves the holder null, and Rust returns
  -2 without unwinding. `swoole_ignore_error` then aborts: upstream threw
  `std::bad_alloc`, which nothing in the extension catches.
- The global set lives until exit. No callbacks or Zend values are retained.
- The set is process-local and copied on fork; it is not shared memory. It has no
  synchronization: callers must keep writes apart from other reads and writes.
  Concurrent reads after initialization are safe.

## Last error and throw

`sw_error` is `swr_v1_error_buffer()`: each thread's `SW_ERROR_MSG_SIZE` (16384) byte
buffer, a Rust `thread_local` that lives as long as its thread (upstream's was a C++
`thread_local` array). `swoole_get_last_error_msg` returns it and
`swoole_clear_last_error_msg` writes its first byte. `swoole_throw_error` writes the
code's message to stderr and aborts: upstream threw `swoole::Exception`, and no
handler in the extension catches one, so it terminated.

`src/core/error.cc` is not built, and the module has no C or C++ source: every
function above is Rust under its upstream symbol.

Tests: `tests/modules/error/`.
