# Utility and logging tests

ABI: [native/utilities/contract.md](../../../native/utilities/contract.md). Rust
owns the hash arithmetic, shared spin acquisition, the locks module's wait/wake
calls, pretty-name slicing and the Logger (`crates/utilities/src/logger.rs`).

Upstream's inline string helpers (`swoole::ltrim`, `rtrim`, `substr_len`,
`starts_with`, `ends_with`, `swoole_strnstr`, `swoole_strnpos`,
`swoole_str_istarts_with` and the rest) have no caller left in the hybrid, so they
left `swoole_util.h` and `swoole.h` with their Rust bodies, and
`core-tests/src/core/string.cpp` has nothing left to test (#2089).

Use a release archive matching the current utility sources, headers and overlays.

## Body and policy gates

```sh
python3 tests/modules/utilities/gate.py --source /pinned/reference \
  --archive /inputs/libswoole_rs_ffi.a --sanitize --output target/utilities.json
python3 tests/modules/utilities/wait.py --source /pinned/reference \
  --archive /inputs/libswoole_rs_ffi.a --sanitize --output target/utilities-wait.json
```

| Script (dev.py phase) | Checks |
| --- | --- |
| `gate.py` (`utilities-native`) | Pinned vs Rust: defined hashes, pretty ranges, levels/labels/rotation, exact line/microsecond formatting, truncation, NUL and errno, thread/shared-mapping spin witnesses; signed and unsigned char |
| `wait.py` (`utilities-wait`) | Wait/wake result, errno and word; real shared fork wake, Linux futex wait channel and fallback, macOS polling |

`--rust-asan` (with `--sanitize`) additionally requires the utility Rust objects
to carry ASan imports. On macOS the installed Apple and Rust ASan
runtimes are incompatible: use `--native-ubsan-only --asan-runtime
/absolute/path/to/librustc-stable_rt.asan.dylib`. Rust's std stays uninstrumented.

## Logger

The Logger is Rust (`crates/utilities/src/logger.rs`); its crate tests cover
files, rotation, diagnostics and concurrent `put` with `reopen`. The upstream
`log.cpp` GoogleTests run against it through `storage.py`'s originals families below.

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
| `--global-logger-originals` | `level`, `date_format_long_string`, `date_with_microseconds`, `ignore_error` |
| `--redirect-originals` / `--redirect-original` | `redirect_1` and `redirect_2` (or just the first) |

Originals run with real GoogleTest and strict XML/status/exit checks. The global
and redirect profiles need complete exports beside both DSOs. Registered as
`utilities-public-storage` and `utilities-{logger,hash,global-logger,redirect}-originals`.

The original spawn helper calls `exit(0)` after its callback, even after a child
`ASSERT` failure, so parent XML and exit status cannot prove child success.
`child_assertions.h` journals child failures and completion; a negative control
fails a synthetic child assertion and must be rejected on every engine. The
redirect originals fail with a pipe stdout (the original `printf` reads the log
before flushing), so `terminal_stdout.py` runs them with a real PTY stdout; the
pipe outcome remains a FAIL.

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
