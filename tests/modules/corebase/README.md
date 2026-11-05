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
  the hybrid build's `base.cc` (the build `A` comes from, with its headers and
  `config.h`) into separate binaries with `driver.cc`, feeds both the corpus from
  `cases.py` and requires identical output/errno, plus 934 independently
  expected values. It also runs eight extracted upstream `TEST(base, ...)`
  bodies (dec2hex, hex2dec, version_compare, common_divisor, common_multiple,
  dirname, itoa, type_size) and the `allocator.cc` strndup interposer. The hybrid
  binary must contain real calls to each Rust helper; the original none. For the
  helpers Rust defines under upstream's own symbols (gcd, lcm, version compare,
  dec2hex) the hybrid base.o must not define them and the archive must.
  `--unsigned-char` also covers bytes 128..255.
- `mkdir.py --source S --archive A --output R [--sanitize]` (`corebase-mkdir`):
  270 traced `swoole_mkdir_recursive` cases (30 paths x 9 syscall scenarios,
  through `mkdir-shim.h`/`mkdir-driver.h`) compared between original and
  the hybrid build's `base.cc`, plus real private-filesystem checks per engine.
- `mkdir_startup.py --engine native=... --engine rust=... --output R`
  (`corebase-startup`): PHP startup through `sys_temp_dir` (create, repeated
  slashes, file parent), comparing warnings and `swoole_last_error`. The Rust
  build manifest must contain the current corebase patch.
- `canonical.py --source PINNED_GIT --config CONFIG_H --archive A --output R [--jobs N] [--openssl-dir P] [--php-dir P] [--ccache EXE | --no-ccache]`
  (`corebase-lifecycle`): builds a native and a corebase-only full library
  with `tools/native_core.py`, requires the selected upstream base GTests to
  pass in both (12 on macOS, all 33 active on Linux). Its report is also
  consumed by `tests/modules/tls/gate.py --allocation-native-report`.

## Helpers

Rust unit tests: the task-directory policy in `crates/corebase/src/lifecycle.rs`,
the hook policy in `hook.rs`, the core state record in `global.rs`.

`patching.py`, `cases.py` and `supervisor.py` are shared helpers.
`supervisor.py` runs child Python tools in-process so SIGTERM/SIGINT unwind
their process groups; a forced SIGKILL does not.

## Known failures and gaps

- The full native GTest inventory fails in both engines: `canonical.py`
  excludes units that do not build or run on the host (on macOS coroutine/hook,
  coroutine/system, lock/lock, network/socket, os/os, os/process_pool,
  os/signal, server/http_server, server/server, coroutine/iouring; on Linux
  coroutine/iouring), so `full_native_inventory_passed` is false. Only the
  selected base cases gate.
- macOS `base.futex` fails its wakeup assertion and times out in both engines
  (the native fallback returns 0 where the test expects 1). Five macOS base
  cases that use fixed `/tmp` paths are not run.
- On Linux, an inherited `LD_DEBUG` breaks `base.shell_exec`'s exact stderr
  check; the gates strip loader tracing variables.
- Locales missing on the host are reported as unavailable, not passed.
- The corpus targets LP64 macOS/Linux only.
- Not covered: Rust-side sanitizers, malloc/system OOM, PHP init with the Rust
  init path, `SW_THREAD` runtime, and performance.
