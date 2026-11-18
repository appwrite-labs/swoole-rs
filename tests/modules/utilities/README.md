# Utility and logging tests

ABI: [native/utilities/contract.md](../../../native/utilities/contract.md). Rust
owns the hash arithmetic, shared spin acquisition, the locks module's wait/wake
calls and the Logger (`crates/corebase/src/utilities/logger.rs`).

Upstream's inline string helpers (`swoole::ltrim`, `rtrim`, `substr_len`,
`starts_with`, `ends_with`, `swoole_strnstr`, `swoole_strnpos`,
`swoole_str_istarts_with` and the rest) have no caller left in the hybrid, so they
left `swoole_util.h` and `swoole.h` with their Rust bodies (#2089).
`core-tests/src/core/string.cpp`'s seven TESTs of them run on the pin only, in the
buffer module's `string-originals` (tests/modules/buffer/string/test_gate.py); its other
16 test `swoole::String` (tests/modules/buffer/README.md).

Use a release archive matching the current utility sources, headers and overlays.

## Pinned differential tests

`crates/corebase/src/utilities/upstream.rs` runs the Rust hashes (signed and unsigned
`char`), level clamp, labels, rotation formats and the
logger's line and microsecond formatting (truncation, NUL and errno) against the
pinned bodies the oracle crate compiles (`crates/oracle/shim/utilities.cc`), and
the spinlock across threads and a forked process. Austin is compared only on
ASCII input: the pinned body shifts negative promoted `char`s. It runs in
`corebase-unit`.

## The wait record

`wait.py` (`utilities-wait`, Linux) built `wait.cc` against the pinned misc.cc futex
bodies and the `swr_v1_utils_wait`/`_wakeup` exports. The oracle crate compiles the same
bodies (`swoole_rs_oracle::locks::wait`/`wake`), so its checks are Rust tests in
`crates/corebase/src/utilities/upstream.rs` (Linux, in `corebase-unit`) over
`futex_wait` and `futex_wakeup`. The tests convert their typed results to the pinned
integer results for comparison.

| `wait.cc` | Rust test |
| --- | --- |
| `differential`: wake on words 0, 1, 2, `0x7fffffff`, `0x80000000`, `0xffffffff` with n of `INT_MIN`, -2, -1, 0, 1, 2 and `INT_MAX`; waits of 1 µs, 1 ms and 1.5 ms, and of -123, -0 and 0 where the word isn't 0; result, errno (from `EDOM`) and word equal | `utilities::upstream::futex_wait_and_wake_match_the_pin`, same inputs and comparison |
| `shared_process`, pinned then Rust: a forked child waits (2 s, `alarm(5)`) on a shared word, the parent sees `futex` in its `/proc/<pid>/wchan` within 3 s, one wake returns 1, the child's wait returns 0 and the word ends 0 | `utilities::upstream::futex_wakes_a_shared_waiter`, the same steps in a single-threaded child test process |

The gate compiled `wait.cc` under ASan and UBSan; the tests have no sanitizer of their own.

## Logger

The Logger is Rust (`crates/corebase/src/utilities/logger.rs`); its crate tests cover
files, rotation, diagnostics and concurrent `put` with `reopen`. No C++ calls it:
`swoole::Logger` and the log macros other than `swoole_fatal_error` left
`swoole_log.h` (#4310), and its one export, `swr_v1_logger_put`, is crates/ffi's
fatal path's.

`set_date_format` takes a NUL-terminated string and so measures its format before
`strftime` reads it. Upstream's `log.date_format_long_string` passed a
`swoole::String`'s unterminated buffer with warnings filtered, where upstream never
measured it; its port (below) passes a terminated one.

`core-tests/src/core/log.cpp`'s TESTs are ported to
`crates/corebase/src/utilities/logger/ported.rs`, one Rust test per TEST with the same
inputs and expected values, over a logger of their own:

| TEST | Rust test |
| --- | --- |
| `log.level` | `utilities::logger::ported::level` |
| `log.date_format_long_string` | `utilities::logger::ported::date_format_long_string` |
| `log.date_with_microseconds` | `utilities::logger::ported::date_with_microseconds` |
| `log.rotation` | `utilities::logger::ported::rotation` |
| `log.redirect_1` | `utilities::logger::ported::redirect_1` |
| `log.redirect_2` | `utilities::logger::ported::redirect_2` |
| `log.ignore_error` | `utilities::logger::ported::ignore_error` |
| `log.set_stream` | `utilities::logger::ported::set_stream` |

The worker type is the logger host's identity, which sets the line's symbol.
`file_get_contents` and `File::read_content` read the file with `std::fs`.
`spawn_exec_and_wait` forks; a failed assertion in the child exits 1, where
upstream's helper exited 0 regardless. `printf` is one `write` to fd 1, the
line-buffered terminal stdout upstream's run had. `ignore_error` runs in a child
too, so the process logger and the ignored code stay out of the other tests.

`log.pretty_name` and `log.pretty_name_lambda` have no Rust test:
`Logger::get_pretty_name` named the function for `__SW_FUNC__`, which no C++ left
expands, and Rust logs pass their origin's name. `log.date_format` (null `sscanf`
destinations) and `log.open_fail` (no assertion) are unqualified.

`core-tests/src/core/util.cpp`'s one case, `util.bitmap`, has no Rust test:
`swoole::BitMap` had no user in the hybrid, so it left `swoole_util.h` with its
Rust storage and there is no bitmap left to test.

## Hash GoogleTests

Cargo runs the four cases from `core-tests/src/core/hash.cpp` in
`tests/modules/utilities/hash.rs` against the Rust functions.

| Upstream case | Rust test |
| --- | --- |
| `hash.php` | `php` |
| `hash.jenkins` | `jenkins` |
| `hash.austin` | `austin` |
| `hash.crc32` | `crc32` |

## Loaded PHP consumers

`php_logger.py --engine ... [--build-manifest target/ext/build-manifest.json]`
(`utilities-php-logger`): PHP Logger thresholds, rotation/reopen, date
rejection/recovery, binary NUL, inherited/private file custody, and four forked
writers with 128 records per file.

## Known gaps

- Unqualified originals: Austin signed-shift UB.
- Native macOS open of an invalid UTF-8 filename fails; that domain is untested.
