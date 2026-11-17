# Core-base validation

The corebase overlay deletes `src/core/base.cc` and `src/core/misc.cc` so that helper policy (type
size, gcd/lcm, stoi env parsing, itoa/radix/hex, version compare, dirname,
recursive mkdir), the lifecycles (clean, thread init/clean, fork, task
tmpdir), the hooks and `swoole_init` (crates/ffi) come from Rust. Rust owns `SwooleG` and `SwooleTG` (`crates/corebase/src/
config_state.rs`, `global.rs`, `lifecycle.rs`); `swoole::Global` and
`swoole::ThreadGlobal` are their C++ views. Native code keeps the C++ objects,
allocation, logging and callback execution. See `native/corebase/contract.md`.

On Homebrew macOS add `--include /opt/homebrew/opt/openssl@3/include` to the
native-TU gates.

## Registered gates

- `constants.py --source PINNED --output R` (`corebase-constants`): compares 327
  core/error identifiers and defaults against the clean pin in Apple and non-Apple
  configuration branches. Those the C headers export compare by C++ type, size,
  signedness and string bytes; the enums no header exports (and `SW_PIPE_READ`/`WRITE`)
  compare by the values `cargo run -p swoole-rs-corebase --example constants`
  (`corebase_enums.rs`) prints. `tls-constants` (`--include-tls`) adds the pinned
  `swoole_ssl.h` and the TLS crate's example (`tls_enums.rs`).

- `mkdir_startup.py --engine native=... --engine cargo=... --build-manifest M --output R`
  (`corebase-startup`): PHP startup through `sys_temp_dir` (create, repeated
  slashes, file parent), comparing warnings and `swoole_last_error`. The Cargo
  engine must be the one `tools/build_ext.py` recorded in `M` (`tools/ext_identity.py`).
## The record views record

`record_views.py` (`corebase-record-views`) compiled a C++ probe against `swoole.h` and
read `SwooleG` and `SwooleTG` through their C++ views. No compiled C++ reads either, so the
`SwooleTG` macro and `swoole::Global`'s `running`, `enable_coroutine` and `pagesize` views
left the header, and its checks are Rust tests (`cargo test -p swoole-rs-corebase`, in
`corebase-unit`):

| Probe check | Rust test or reason |
| --- | --- |
| init's libc allocator (callable), `max_sockets > 0`; clean nulls the allocator and zeroes `max_sockets` | `config_state::tests::storage_init_task_directory_and_reset` |
| `dns_cache_refresh_time` written in place, zeroed by clean | the same test: 2.5's bits stored, 0 after the reset |
| `running`, `enable_coroutine` and `pagesize > 0` after init, `running` false after clean | `global::tests::record_lifecycle` |
| `SwooleTG.main_thread`, and each thread's own record: zero on a new thread, set there without touching the main thread's `id` 19 and `type` 3 | `lifecycle::tests::thread_records_are_per_thread` |
| `SwooleG.max_sockets = 91` written and read back in place, and `SwooleG`'s and `SwooleTG`'s addresses stable across clean | none: no compiled C++ reads `SwooleG`, whose `swoole::Global` layout both sides assert, and `SwooleTG` is Rust's alone |

## The number helpers record

`numbers.rs` (`cargo test -p swoole-rs-corebase --test numbers`, in `corebase-unit`) holds
what the `corebase-native` gate (`differential.py`, `driver.cc`, `cases.py`, `patching.py`,
`allocator.cc`) asked of the number helpers in `crates/corebase/src/lib.rs`. The gate built
the complete pinned `base.cc` and a hybrid unit into two binaries with `driver.cc`, fed
both `cases.py`'s corpus and required the same value and errno per case, plus the
independently computed value. The test calls the Rust functions with typed arguments and
the same pinned bodies, which `crates/oracle` copies unchanged from the vendored `base.cc`
(`shim/corebase.cc`), in one process: each case presets errno, calls, and reads errno back,
on each side. Same inputs, same order within an operation, same independent values; each
operation's test asserts its share of the corpus (5,455 cases and 915 independent values
in all where `char` is signed, 6,095 and 1,171 where it is unsigned), counting a locale
the host lacks as unavailable, never as passed.

`--signed-char` and `--unsigned-char` compiled the gate's C++ with `-fsigned-char` or
`-funsigned-char`; the second added the single bytes 128 to 255 to the hex inputs. The
Rust `swoole_hex2dec` promotes in the target's plain-`char` mode and the oracle compiles
the pin in that same mode, so each target proves its own: 1 to 127 where `char` is signed
(x86-64, macOS), 1 to 255 where it is unsigned (arm64 Linux).

| Gate part | Rust test or reason |
| --- | --- |
| `type` (128), `gcd` (9), `lcm` (15), `radix` (348), `decimal` (115) cases with their expected values | `corpus_type`, `corpus_gcd`, `corpus_lcm`, `corpus_radix` and `corpus_decimal`, over `swoole_type_size`, `swoole_common_divisor`, `swoole_common_multiple`, `swoole_dec2hex` and `swoole_itoa`. `swoole_itoa` took `char *`; it writes into a 128-byte slice, as the driver's buffer was, and the count, digits and NUL are compared |
| `hex` cases in five locales from errno 7, the C and POSIX ones with expected values (750, or 1,390 with high bytes) | `corpus_hex`, over `swoole_hex2dec` |
| `version` cases in five locales from errno 0 and 7 (4,000) | `corpus_version`, over `swoole_version_compare` |
| `envstoi` cases in five locales from errno 34 (90): the pinned `swoole_get_env` against `swoole.h`'s inline one, which asked `swr_v1_corebase_parse_stoi` first and ran `std::stoi` itself where that refused | `corpus_envstoi`: `parse_stoi` gives the pin's value from the same errno wherever the pin returns one, as does `get_env`, which reads the variable itself, and none exactly where `std::stoi` threw `invalid_argument` or `out_of_range`, with the pin's output left alone. The exception's `what()` and the errno after a throw came from `std::stoi` on both sides, the header's own on the hybrid's; that header function is gone |
| `envstoi_allocfail`: a failed `std::string` allocation inside the header's `swoole_get_env` throws `bad_alloc` before Rust runs | none: it tested only the deleted C++ wrapper; `parse_stoi` takes a slice and allocates nothing |
| The seven extracted `TEST(base, ...)` bodies, run on each binary | `base_dec2hex`, `base_hex2dec`, `base_version_compare`, `base_common_divisor`, `base_common_multiple`, `base_itoa`, `base_type_size`: one test per TEST, same inputs and expected values, on the Rust function and on the pinned body |
| `allocator.cc`: a `strndup` interposer under `swoole_dec2hex` on each binary (one allocation of the exact length, a reentrant `swoole_hex2dec`, `ENOMEM` and null, a retry) | `dec2hex_allocates_once_through_strndup`: the test binary defines `strndup`, so both sides allocate through it; same three calls, counts, lengths and errno |
| `prove_calls`: the hybrid binary must call `swr_v1_corebase_parse_stoi` and the original must not | none: the export and its one caller, `swoole.h`'s inline `swoole_get_env`, are gone, since no C++ but the driver called either |
| `prove_definitions`: the archive, not the hybrid unit, defines the seven upstream symbols | none for the test, which calls the functions by path. `swoole_itoa`'s symbol is gone with its `char *` form; the other six are still defined under `upstream-symbols` |

The gate compiled its C++ under ASan and UBSan (`--sanitize`), which covered the pinned
bodies, `driver.cc` and the `swoole.h` inline wrapper, not the Rust archive. The test has
no sanitizer of its own; `cargo fuzz`'s ASan build instruments the oracle's C++.

## The resolver record

`resolver.rs` (`cargo test -p swoole-rs-corebase --test resolver`) holds what two native
checks asked of `crates/corebase/src/resolver_request.rs`, against the Rust API that was
under their C exports. Its binary defines `getaddrinfo` and `freeaddrinfo`, as the gate
did, and passes to libc's unless a test set a fake answer. The exports
(`swr_v1_core_resolver_new`, `_create`, `_span`, `_resize`, `_assign`, `_lookup`, `_host`,
`_next`) and the C++ views over them (`ResolverOwner`, `ResolverAddresses`,
`GethostbynameRequest`, `GetaddrinfoRequest`) had no caller but these checks. The gate
built its C++ with `-Wall -Werror` under ASan/UBSan; the test doesn't.

| Former check | Rust test or reason |
| --- | --- |
| `resolver_guard.cc` (`corebase-dns-request-guard`) | `diagnostic_refusal_cleans_up_and_publishes_nothing`: same fake entry (family 12345), error 55 and count 77, refusal then acceptance, the same statuses, call counts, freed list and published fields. The 0x5a address comes from a faked `AF_INET6` answer in place of `resize(1)` and `memset`, and is read back with `ResolverRequest::results` |
| the guard's contained `std::bad_alloc` | none: the diagnostic is a Rust closure, which throws nothing |
| the guard's unchanged `data` pointer after a refusal | none: Rust lends no pointer into the addresses; their length and bytes are compared |
| `rust_dns_request.numeric_host_owner_and_independent_copies` (hooks fixture) | `numeric_host_owner_and_independent_records`: the same two lookups and expected texts, over two records in place of a C++ copy, assignment and self-assignment |
| `rust_dns_request.hints_vectors_and_failure_publication` (hooks fixture) | `hints_vectors_and_failure_publication`: the same hints, the family -123 failure, and the count and address bytes it must keep. The parts over a C++ copy (an edited copy's text, `clear`, `resize`, assignment) have no counterpart |
| `rust_dns_request.native_output_append_exception_is_contained` (hooks fixture) | `an_ipv6_result_reads_back_as_its_text`, for the `AF_INET6` texts it read. Its other half has no form: it threw `std::bad_alloc` from a C++ `emplace_back` between two `swr_v1_core_resolver_next` calls, and `ResolverRequest::addresses` builds its list in Rust |

## Helpers

Rust unit tests: the task-directory policy in `crates/corebase/src/config_state.rs`,
the hook policy in `hook.rs`, the core state record in `global.rs`.

`core-tests/src/core/base.cpp` cases whose helpers #1384 deleted, and where
their assertions run now:

| Upstream TEST | Rust test or reason |
| --- | --- |
| `base.clamp` | `crates/process` `process_pool.rs` `base_clamp`, over `max_package_size`'s clamp (the one `swoole::clamp` caller) |
| `base.datahead_dump` | `host.rs` `base_datahead_dump` (same input and assertion; `print` is gone) |
| `base.mkdir_recursive` | `lib.rs` `base_mkdir_recursive`, over `mkdir_recursive` (test adapter preserves the original status assertion) |
| `base.hook`, `base.snprintf` | `hook.rs` and `host.rs` tests |
| `base.intersection` | `crates/http` `static_context/selection.rs` `base_intersection`, over the index-file loop that took its job |
| `base.dirname`, `base.string_format` | none: nothing in the hybrid takes a dirname or formats into a fresh buffer, so there is no function to test |

`sw_wait_for` has no upstream case; `crates/corebase/src/locks` `poll` keeps its backoff.

`dns-timeout-originals.json` holds the original
`swoole_coroutine_system/gethostbyname_timeout.phpt`. On an ordinary resolver the
unknown name fails before its 1 ms deadline on both engines (last error 711, not
712), so the c-ares variant (`variant.yml`) runs it under `slow_resolver.py`: new
mount and network namespaces whose local DNS server answers `www.github.com` at
once and every other name NXDOMAIN after 0.5 s. Only c-ares builds report 712 for
a lookup that outlives its deadline; the thread-pool lookup reports
`SW_ERROR_CO_TIMEDOUT` (10018) on both engines, so the case isn't run there. The
test and its assertions are unchanged.

`core-tests/src/os/thread.cpp`'s `start_with_signals_blocked` is
`crates/corebase/src/thread.rs`'s test of the same name. Its
`start_failure_restores_signal_mask` makes the C++ callable's second copy throw;
Rust's `Thread::start` moves the body once and never copies it, so the case has no
counterpart.

`core-tests/src/core/time.cpp` tested `swoole_util.h`'s `swoole::time<T>(steady)` and
`get_timezone`. Nothing in the hybrid called them any more (Rust reads its own clocks), so
they left the header with `Timer::get_absolute_msec`; the TESTs run over the Rust clocks
that took their callers, same inputs and expected values (`cargo test`):

| Upstream TEST | Rust test or reason |
| --- | --- |
| `time.get_ms` | none: the system clock in milliseconds had no caller, so there is no function to test |
| `time.get_ms_steady` | crates/timer `runtime::tests::time_get_ms_steady`, over `absolute_msec` (`Timer::get_absolute_msec`) |
| `time.get_seconds` | `global::tests::time_get_seconds`, over `time_now` |
| `time.get_timezone` | crates/corebase/src/utilities `utilities::helpers::tests::time_get_timezone`, over `timezone`; like upstream's, it needs a zone at or east of UTC |

## Known failures and gaps

- `numbers.rs` reports locales missing on the host as unavailable, not passed.
- Its corpus targets LP64 macOS/Linux only.
- Not covered: Rust-side sanitizers, malloc/system OOM, PHP init with the Rust
  init path, `SW_THREAD` runtime, and performance.

`sys::tests::socket_option_errors_keep_errno_and_failed_output` checks that socket-option
Results retain the failing syscall errno after another operation changes it, leave the
output untouched on failure, and round-trip byte-valued options. Existing socket-call
assertions retain their original integer/optional observations through local adapters.

Filesystem status calls (`access`, `mkdir`, `unlink`, `chdir`, `chroot`) return
`io::Result<()>`. Recursive mkdir returns `MkdirError`, carrying the syscall errno
or a named path/state failure. The cursor tests retain every original assertion
through local status adapters. `corebase-startup` checks real creation, repeated
slashes and a file-parent error against the pinned engine, including warnings
and last-error publication after releasing the task-path lock.

`corebase::sys` descriptor tests cover typed `socket`, `dup` and `dup2` results:
errors retain errno, failed duplication leaves its target open, self-duplication
returns the target, and duplicated pipe descriptors share their original behavior.
PHP and native adapters retain their existing integer and warning contracts.

`sys::fcntl` and `sys::poll` retain captured errno in `io::Result`; readiness and
zero timeout counts remain distinct from errors. Unit tests cover descriptor
flag/duplication behavior, failed flag operations, pipe readiness, ignored negative
descriptors and `POLLNVAL` as a successful readiness result. Native poll adapters
retain their integer ABI, and caller EINTR retries use the captured error.

The socket, event-data, task and TLS protocol flags are integer-backed Rust types.
The constants probes print every named flag's bits and compare them with the
pinned headers, alongside the exhaustive enum tables. Socket options, task-file
flags, event payload selection and TLS protocol options retain their existing
runtime tests; conversion to integers happens at the native fields and calls.

`lent.rs` checks that capped output counts preserve zero, never exceed the lent
allocation, and saturate oversized limits without wrapping signed syscall counts.

`spare.rs` verifies scalar receive appends, zero/error results, datagram truncation,
and a real short `readv` spanning an empty room without exposing unwritten bytes.
