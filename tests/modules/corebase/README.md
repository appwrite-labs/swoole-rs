# Core-base validation

The corebase overlay deletes `src/core/base.cc` and `src/core/misc.cc` so that helper policy (type
size, gcd/lcm, stoi env parsing, itoa/radix/hex, version compare, dirname,
recursive mkdir), the lifecycles (clean, thread init/clean, fork, task
tmpdir) and the hooks come from Rust, with `native/corebase/globals.cc` for what
stable Rust can't express. Rust owns `SwooleG` and `SwooleTG` (`crates/corebase/src/
config_state.rs`, `global.rs`, `lifecycle.rs`); `swoole::Global` and
`swoole::ThreadGlobal` are their C++ views. Native code keeps the C++ objects,
allocation, logging and callback execution. See `native/corebase/contract.md`.

On Homebrew macOS add `--include /opt/homebrew/opt/openssl@3/include` to the
native-TU gates.

## Registered gates

- `constants.py --source PINNED --output R` (`corebase-constants`): compares 326
  core/error identifiers and defaults against the clean pin, including C++ types,
  sizes, signedness and string bytes in Apple and non-Apple configuration branches.

- `differential.py --source CONFIGURED_PINNED --archive A --output R [--sanitize] [--signed-char | --unsigned-char] [--include DIR]`
  (`corebase-native`): compiles the complete original `base.cc` (pinned) and
  the hybrid build's `globals.cc`, where base.cc's glue went (the build `A` comes from,
  with its headers and `config.h`), into separate binaries with `driver.cc`, feeds
  both the corpus from `cases.py` and requires identical output/errno, plus the
  independently expected values. It also runs seven extracted upstream `TEST(base, ...)`
  bodies (dec2hex, hex2dec, version_compare, common_divisor, common_multiple,
  itoa, type_size) and the `allocator.cc` strndup interposer. The hybrid
  binary must contain real calls to each Rust helper; the original none. For the
  helpers Rust defines under upstream's own symbols (gcd, lcm, version compare,
  dec2hex) the hybrid globals.o must not define them and the archive must.
  `--unsigned-char` also covers bytes 128..255.
- `mkdir_startup.py --engine native=... --engine rust=... --output R`
  (`corebase-startup`): PHP startup through `sys_temp_dir` (create, repeated
  slashes, file parent), comparing warnings and `swoole_last_error`. The Rust
  build manifest must contain the current corebase patch.
## Helpers

Rust unit tests: the task-directory policy in `crates/corebase/src/config_state.rs`,
the hook policy in `hook.rs`, the core state record in `global.rs`.

`core-tests/src/core/base.cpp` cases whose helpers #1384 deleted, and where
their assertions run now:

| Upstream TEST | Rust test or reason |
| --- | --- |
| `base.datahead_dump` | `host.rs` `base_datahead_dump` (same input and assertion; `print` is gone) |
| `base.mkdir_recursive` | `lib.rs` `base_mkdir_recursive`, over `swr_v1_corebase_mkdir_complete` |
| `base.hook`, `base.snprintf` | `hook.rs` and `host.rs` tests |
| `base.intersection` | `crates/http` `static_context/selection.rs` `base_intersection`, over the index-file loop that took its job |
| `base.dirname`, `base.string_format` | none: nothing in the hybrid takes a dirname or formats into a fresh buffer, so there is no function to test |

`sw_wait_for` has no upstream case; `crates/locks` `poll` keeps its backoff.

`patching.py` and `cases.py` are shared helpers.

`dns-timeout-originals.json` holds the original
`swoole_coroutine_system/gethostbyname_timeout.phpt`. On an ordinary resolver the
unknown name fails before its 1 ms deadline on both engines (last error 711, not
712), so the c-ares workflow (`cares.yml`) runs it under `slow_resolver.py`: new
mount and network namespaces whose local DNS server answers `www.github.com` at
once and every other name NXDOMAIN after 0.5 s. Only c-ares builds report 712 for
a lookup that outlives its deadline; the thread-pool lookup reports
`SW_ERROR_CO_TIMEDOUT` (10018) on both engines, so the case isn't run there. The
test and its assertions are unchanged.

## Known failures and gaps

- Locales missing on the host are reported as unavailable, not passed.
- The corpus targets LP64 macOS/Linux only.
- Not covered: Rust-side sanitizers, malloc/system OOM, PHP init with the Rust
  init path, `SW_THREAD` runtime, and performance.
