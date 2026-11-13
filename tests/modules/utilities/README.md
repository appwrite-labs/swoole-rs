# Utility and logging tests

ABI: [native/utilities/contract.md](../../../native/utilities/contract.md). Rust
owns the hash arithmetic, shared spin acquisition, the locks module's wait/wake
calls, pretty-name slicing and the Logger (`crates/utilities/src/logger.rs`).

Upstream's inline string helpers (`swoole::ltrim`, `rtrim`, `substr_len`,
`starts_with`, `ends_with`, `swoole_strnstr`, `swoole_strnpos`,
`swoole_str_istarts_with` and the rest) have no caller left in the hybrid, so they
left `swoole_util.h` and `swoole.h` with their Rust bodies (#2089).
`core-tests/src/core/string.cpp`'s seven TESTs of them run on the pin only, in the
buffer module's `string-originals` (tests/modules/buffer/string/test_gate.py); its other
16 test `swoole::String` (tests/modules/buffer/README.md).

Use a release archive matching the current utility sources, headers and overlays.

## Pinned differential tests

`crates/utilities/src/upstream.rs` runs the Rust hashes (signed and unsigned
`char`), pretty-name ranges, level clamp, labels, rotation formats and the
logger's line and microsecond formatting (truncation, NUL and errno) against the
pinned bodies the oracle crate compiles (`crates/oracle/shim/utilities.cc`), and
the spinlock across threads and a forked process. Austin is compared only on
ASCII input: the pinned body shifts negative promoted `char`s. It runs in
`utilities-unit`.

## Wait gate

```sh
python3 tests/modules/utilities/wait.py --source /pinned/reference \
  --archive /inputs/libswoole_rs_ffi.a --sanitize --output target/utilities-wait.json
```

`wait.py` (`utilities-wait`, Linux) checks the wait/wake result, errno and word,
a real shared fork wake, and the futex wait channel and fallback.

## Logger

The Logger is Rust (`crates/utilities/src/logger.rs`); its crate tests cover
files, rotation, diagnostics and concurrent `put` with `reopen`.

Its C exports state their callers' contract in the parameter types where one exists:
`swr_v1_logger_free` takes back the box `swr_v1_logger_new` made, and `swr_v1_logger_open`
and `swr_v1_logger_set_date_format` take a NUL-terminated string as corebase's `Text`
(`SwrText` in `swoole_rs_utilities.h`). `set_date_format` therefore measures its format
before `strftime` reads it. Upstream's `log.date_format_long_string` passed a
`swoole::String`'s unterminated buffer with warnings filtered, where upstream never
measured it; its port (below) passes a terminated one.

`core-tests/src/core/log.cpp`'s TESTs that read the log back are ported to
`crates/utilities/src/logger/ported.rs`, one Rust test per TEST with the same
inputs and expected values, over a logger of their own:

| TEST | Rust test |
| --- | --- |
| `log.level` | `logger::ported::level` |
| `log.date_format_long_string` | `logger::ported::date_format_long_string` |
| `log.date_with_microseconds` | `logger::ported::date_with_microseconds` |
| `log.redirect_1` | `logger::ported::redirect_1` |
| `log.redirect_2` | `logger::ported::redirect_2` |
| `log.ignore_error` | `logger::ported::ignore_error` |

The worker type is the logger host's identity, which sets the line's symbol.
`file_get_contents` and `File::read_content` read the file with `std::fs`.
`spawn_exec_and_wait` forks; a failed assertion in the child exits 1, where
upstream's helper exited 0 regardless. `printf` is one `write` to fd 1, the
line-buffered terminal stdout upstream's run had. `ignore_error` runs in a child
too, so the process logger and the ignored code stay out of the other tests. The
rest of `log.cpp` runs through `storage.py`'s originals families below.

`core-tests/src/core/util.cpp`'s one case, `util.bitmap`, has no Rust test:
`swoole::BitMap` had no user in the hybrid, so it left `swoole_util.h` with its
Rust storage and there is no bitmap left to test.

## Loaded PHP consumers

`storage.py` takes `--source`, `--engine native=DSO --engine selected=DSO`,
`--build-manifest` (`target/ext/build-manifest.json`, which must name the selected
DSO), `--archive` (whose tree supplies the selected SDK headers) and at most one
profile flag. The native SDK is the pin's `include/` with the upstream build's `config.h`:

| Profile | Checks |
| --- | --- |
| default | 54 filename cases, 11 levels, long-path rename/reopen, reset retaining FILE, caller streams, one writer plus one reopener (1,024 records/64 reopens), ten date formats, 32 reset combinations; helper layout must match each engine's SDK (`storage_summary.py`) |
| `--backtrace` | Default/filter emit no trace; enabled/reset emit two symbolized traces and three ordered records |
| `--pretty` | Public pretty-name method vs the pinned body, 42,096 cases |
| `--logger-originals` | Unchanged GoogleTests `pretty_name`, `pretty_name_lambda`, `rotation`, `set_stream` |
| `--pretty-originals` | The two pretty-name originals only |
| `--hash-originals` | `hash.php`, `hash.jenkins` |

Originals run with real GoogleTest and strict XML/status/exit checks. Registered as
`utilities-public-storage` and `utilities-{logger,hash}-originals`.

Other consumers:

- `php_logger.py --engine ... [--build-manifest target/ext/build-manifest.json]`
  (`utilities-php-logger`): PHP Logger thresholds, rotation/reopen, date
  rejection/recovery, binary NUL, inherited/private file custody, and four forked
  writers with 128 records per file.

## Logging macros (`macro_profile.py`)

```sh
python3 tests/modules/utilities/macro_profile.py \
  --production-receipt /absolute/path/to/originals.json \
  --output target/macro-profile/report.json
```

Takes a passing `storage.py` originals receipt, re-verifies its SDK
headers and DSOs, and runs unchanged macro bodies through the production Logger
DSOs for four debug/trace combinations per engine: filtered argument laziness,
repeated level/error reads, last-error publication, trace masks, labels/order and
hex rows. `--candidate-header` / `--candidate-archive` test a private frontdoor
implementation (macOS also needs `--asan-runtime`).
`--single-profile`, `--cpp-standard` and `--extra-cxx-flag` adjust the build. The
output directory must exist.

## Known gaps

- Unqualified originals: Austin signed-shift UB, `log.date_format` (null scanf
  destinations) and `open_fail` (uncontrolled path, no assertion).
- Native macOS open of an invalid UTF-8 filename fails; that domain is untested.
