# Core-base validation

The corebase overlay patches `src/core/base.cc` so that helper policy (type
size, gcd/lcm, stoi env parsing, itoa/radix/hex, version compare, dirname,
recursive mkdir), lifecycle sequencing (clean, thread init/clean, fork, task
tmpdir) and hook insertion decisions come from Rust. Rust owns `SwooleG`'s
scalars and native pointers (`crates/corebase/src/global.rs`), which
`swoole::Global` exposes as `RustField` views under their upstream names;
`swoole_init` calls it for their defaults. Native code keeps `SwooleG`'s C++
objects, TLS, allocation, logging and callback execution. See
`native/corebase/contract.md`.

On Homebrew macOS add `--include /opt/homebrew/opt/openssl@3/include` to the
native-TU gates.

## Registered gates

- `constants.py --source PINNED --output R` (`corebase-constants`): compares 326
  core/error identifiers and defaults against the clean pin, including C++ types,
  sizes, signedness and string bytes in Apple and non-Apple configuration branches.

- `differential.py --source CONFIGURED_PINNED --archive A --output R [--sanitize] [--signed-char | --unsigned-char] [--include DIR]`
  (`corebase-native`): compiles the complete original `base.cc` (pinned) and
  the hybrid build's `misc.cc`, where base.cc's glue went (the build `A` comes from,
  with its headers and `config.h`), into separate binaries with `driver.cc`, feeds
  both the corpus from `cases.py` and requires identical output/errno, plus the
  independently expected values. It also runs seven extracted upstream `TEST(base, ...)`
  bodies (dec2hex, hex2dec, version_compare, common_divisor, common_multiple,
  itoa, type_size) and the `allocator.cc` strndup interposer. The hybrid
  binary must contain real calls to each Rust helper; the original none. For the
  helpers Rust defines under upstream's own symbols (gcd, lcm, version compare,
  dec2hex) the hybrid misc.o must not define them and the archive must.
  `--unsigned-char` also covers bytes 128..255.
- `mkdir_startup.py --engine native=... --engine rust=... --output R`
  (`corebase-startup`): PHP startup through `sys_temp_dir` (create, repeated
  slashes, file parent), comparing warnings and `swoole_last_error`. The Rust
  build manifest must contain the current corebase patch.
## Helpers

Rust unit tests: the task-directory policy in `crates/corebase/src/lifecycle.rs`,
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

## Known failures and gaps

- Locales missing on the host are reported as unavailable, not passed.
- The corpus targets LP64 macOS/Linux only.
- Not covered: Rust-side sanitizers, malloc/system OOM, PHP init with the Rust
  init path, `SW_THREAD` runtime, and performance.
