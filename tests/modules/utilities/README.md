# Utility and logging validation (M14)

M14 is opt-in (feature bit 512, ABI in
[native/utilities/contract.md](../../../native/utilities/contract.md)). Rust owns
defined LP64 PHP/Jenkins/Austin hash arithmetic, shared spin acquisition, M16
wait/wake calls, pretty-name slicing, log thresholds/labels/rotation formats,
line/microsecond formatting, Logger action sequencing, scoped filename
construction/comparison/reopen capture and 16 shared inline helpers. The
selected Logger (state ABI 2) keeps its nine persistent logical/string fields in
one Rust context, so selected consumers must rebuild against the selected SDK;
there is no binary layout compatibility with native. Native code keeps FILE/fd/
mutex custody, clock/calendar/libc, identity, descriptor, diagnostic and
backtrace effects after Rust returns.

Use a release archive matching the current utility sources, headers and overlays.

## Body and policy gates

```sh
python3 tests/modules/utilities/gate.py --source /pinned/reference \
  --archive /inputs/libswoole_rs_ffi.a --sanitize --output target/utilities.json
python3 tests/modules/utilities/bytes_gate.py --source /pinned/reference \
  --archive /inputs/libswoole_rs_ffi.a --output target/utilities-bytes.json
python3 tests/modules/utilities/wait.py --source /pinned/reference \
  --archive /inputs/libswoole_rs_ffi.a --sanitize --output target/utilities-wait.json
python3 tests/modules/utilities/put.py --source /pinned/reference \
  --archive /inputs/libswoole_rs_ffi.a --sanitize --threads --output target/utilities-put.json
```

| Script (dev.py phase) | Checks |
| --- | --- |
| `gate.py` (`utilities-native`) | Pinned vs Rust: defined hashes, pretty ranges, levels/labels/rotation, exact line/microsecond formatting, truncation, NUL and errno, thread/shared-mapping spin witnesses; signed and unsigned char |
| `bytes_gate.py` (`utilities-bytes`) | All 16 pinned vs overlaid inline helpers: binary/NUL input, bounds/sentinels/errno, locale classification and casefold, both char modes |
| `wait.py` (`utilities-wait`) | Wait/wake result, errno and word; real shared fork wake, Linux futex wait channel and fallback, macOS polling |
| `put.py --threads` (`utilities-put-threads`) | Complete `Logger::put` bodies with four threads, real mutex/stdio: 16,384 ordered records and 1,024 stream/flag publications per engine |

`--rust-asan` (with `--sanitize`) additionally requires the utility Rust objects
to carry ASan imports (`rust_asan.py`). On macOS the installed Apple and Rust ASan
runtimes are incompatible: use `--native-ubsan-only --asan-runtime
/absolute/path/to/librustc-stable_rt.asan.dylib`. Rust's std stays uninstrumented.

`put.py` requires `--threads`; the old non-thread, configuration, lifecycle and
redirect native-field models were removed because they cannot exercise the
Rust-owned Logger state. The `utilities-put` phase (no `--threads`) now exits
with a usage error. The thread fixture replaces `localtime` with `localtime_r` and
uses a literal date to avoid the original static-`tm` race; it does not cover
concurrent calendar/configuration/open-close or TSan.

Defined domains exclude empty search needles, reverse-search underflow and
undefined signed-char ctype inputs. Reverse search omits offset 0;
case-insensitive search keeps the exact-first-byte guard and host `strncasecmp`.
Unavailable locales (for example Linux `en_US.UTF-8`) are unqualified.

## Whole-class Logger (`logger_state.py`)

```sh
python3 tests/modules/utilities/logger_state.py --source /pinned/reference \
  --native-export NATIVE_EXPORT --sdk-ref COMMIT \
  --archive BUILD/rust-prefix/lib/libswoole_rs_ffi.a --output target/logger-state.json
```

Compiles pinned `log.cc` against the native export's SDK (`coherent_sdk.py`) and
the hybrid build's `log.cc` against that build's headers into separate DSOs, then runs `logger_state_public.cc` against each:
30 public checks (defaults/clamping, file/reopen/close paths, unowned streams,
reset/backtrace retention, rotation, descriptor redirection), stream replacement,
1,024 records during 128 concurrent reopens and 42 filename values. The selected
build also runs the C11 `logger_path_c.c` consumer. Accepts the same
sanitizer flags as above. Registered as `utilities-logger-owned-state`.

## Loaded PHP consumers

`storage.py` takes `--source`, `--engine native=DSO --engine selected=DSO`,
`--build-manifest`, `--archive`, `--source-ref` and at most one profile flag:

| Profile | Checks |
| --- | --- |
| default | 54 filename cases, 11 levels, long-path rename/reopen, reset retaining FILE, caller streams, one writer plus one reopener (1,024 records/64 reopens), ten date formats, 32 reset combinations; helper layout must match each engine's SDK (`storage_summary.py`) |
| `--backtrace` | Default/filter emit no trace; enabled/reset emit two symbolized traces and three ordered records |
| `--pretty` | Public pretty-name method vs the pinned body, 42,096 cases |
| `--logger-originals` | Unchanged GoogleTests `pretty_name`, `pretty_name_lambda`, `rotation`, `set_stream` |
| `--pretty-originals` | The two pretty-name originals only |
| `--hash-originals` | `hash.php`, `hash.jenkins` |
| `--inline-originals` | `ltrim`, `rtrim`, `strnpos`, `strnstr`, `substr_len`, `starts_with`, `ends_with` |
| `--global-logger-originals` | `level`, `date_format_long_string`, `date_with_microseconds`, `ignore_error` |
| `--redirect-originals` / `--redirect-original` | `redirect_1` and `redirect_2` (or just the first) |

Originals run with real GoogleTest and strict XML/status/exit checks. The global
and redirect profiles need complete exports beside both DSOs. Registered as
`utilities-public-storage` and `utilities-{logger,hash,inline,global-logger,redirect}-originals`.

The original spawn helper calls `exit(0)` after its callback, even after a child
`ASSERT` failure, so parent XML and exit status cannot prove child success.
`child_assertions.h` journals child failures and completion; a negative control
fails a synthetic child assertion and must be rejected on every engine. The
redirect originals fail with a pipe stdout (the original `printf` reads the log
before flushing), so `terminal_stdout.py` runs them with a real PTY stdout; the
pipe outcome remains a FAIL.

Other consumers:

- `php_logger.py --engine ... [--build-manifest F --archive F --source-ref R]`
  (`utilities-php-logger`): PHP Logger thresholds, rotation/reopen, date
  rejection/recovery, binary NUL, inherited/private file custody, and four forked
  writers with 128 records per file.
- `lowercase_sdk.py --source S --sdk-export EXPORT --output F`
  (`utilities-lowercase-sdk`): compiles the unchanged HTTP Response TU native and
  selected, both char modes, and checks the lowercase import.
- `lowercase_runtime.py` (`utilities-lowercase-runtime`, same pair arguments plus
  `--source`): the unchanged trailer PHPT from `lowercase-originals.json` and the
  raw HTTP/1 wire fixture `lowercase_wire.py`. Linux needs `--php-extension sockets`.

## Logging macros (`macro_profile.py`)

```sh
python3 tests/modules/utilities/macro_profile.py \
  --production-receipt /absolute/path/to/originals.json \
  --output target/macro-profile/report.json
```

Takes a passing `storage.py --inline-originals` receipt, re-verifies its SDK
headers and DSOs, and runs unchanged macro bodies through the production Logger
DSOs for four debug/trace combinations per engine: filtered argument laziness,
repeated level/error reads, last-error publication, trace masks, labels/order and
hex rows. `--candidate-header` / `--candidate-archive` test a private frontdoor
implementation (macOS also needs `--asan-runtime`).
`--single-profile`, `--cpp-standard` and `--extra-cxx-flag` adjust the build. The
output directory must exist.

## Benchmark preparation

```sh
python3 tests/modules/utilities/prepare_benchmark.py --source /pinned/reference \
  --archive /copies/selected/libswoole_rs_ffi.a \
  --build-manifest /copies/selected/build-manifest.json --output target/prepared.json
python3 -m unittest discover -s tests/modules/utilities -p 'test_*.py'
```

Requires an immutable optimized release archive whose manifest matches the
current utility sources, header and patches. Builds `benchmark.cc` and
`benchmark_logger.cc` at `-O3` and runs all 40 workloads in check mode (no clock;
`elapsed_ns` must be 0): hash, pretty, formatting, trim/search/compare/partition,
enum policy, ready-word wait/wake, six complete `put` bodies, and lowercase at
32/256/4096 bytes. Measurement uses the printed `--measure` template and must run
without concurrent workloads; none has run.

## Known gaps

- Unqualified originals: Austin signed-shift UB, `log.date_format` (null scanf
  destinations) and `open_fail` (uncontrolled path, no assertion).
- Native macOS open of an invalid UTF-8 filename fails; that domain is untested.
- Allocation/exception/lifetime failures, concurrent calendar/configuration/
  open-close, other platforms, matched performance and default activation remain open.
