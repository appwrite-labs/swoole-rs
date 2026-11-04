# Hooks

Pin `c7e514509451beafd4d70ce5b78943d51e76381c`.

## Native-facade gates

```sh
python3 tests/modules/hooks/ordered_gate.py --source PINNED_SOURCE --archive BUILD/rust-prefix/lib/libswoole_rs_ffi.a --sanitize --report OWN/ordered.json
python3 tests/modules/hooks/compile_gate.py --source PINNED_SOURCE --config ACTUAL_GENERATED_CONFIG --report OWN/compile.json
```

`async_gate.py`, `packet_gate.py`, `socket_policy_gate.py` and `lifecycle_gate.py`
take the same arguments. Each `*_gate.py` extracts the pinned function bodies and
the patched ones from the hybrid build `--archive` comes from (`hybrid_tree`), compiles both
against a deterministic native facade (`*_fixture.h`) and runs identical
scenarios through each, comparing effects and order. Syscalls, contexts, TLS
and kernel IO are faked; these are policy/order checks, not runtime proof.
`--sanitize` adds ASan/UBSan to C++ only, not the Rust archive.

- `ordered_gate.py` (26 cases): the hook guard's short circuit and
  `wait_for`'s retry and cancel. The coroutine Socket's transfers and
  `TimerController` moved to Rust with their cases (`ported_*` tests in
  `crates/hooks/src/transfer.rs` and `co_io.rs`).
- `async_gate.py` (434 cases): async event/lambda/sleep/DNS bodies, including
  dispatch rejection, timeout/cancel completion, late canceled completion,
  resume-deletes-event lifetime and errno capture.
- `packet_gate.py` (624 cases): length/EOF/raw framing paths and vectored
  all-transfer paths with EAGAIN/EINTR/error/partial progress.
- `socket_policy_gate.py` (420 cases): Socket header and TimeoutController
  bodies: read/write CID diagnostics, close/exception, closed/null sockets,
  error priority, exact timeout bits and clock phases.
- `lifecycle_gate.py` (1249 cases): shutdown, cancel, close after resume,
  destructor reentry, connect EINTR/EINPROGRESS/timer/option outcomes,
  liveness errno and native exception unwind order.

`compile_gate.py` compiles all eight original and patched translation units as
C++14 using the given `config.h`, and checks that each overlay refuses to
compile when `config.h` does not define `SWOOLE_RS_HOOKS`. io_uring units are
reported UNSUPPORTED when the config does not enable `SW_USE_IOURING`.

## Original tests

`hooks-native-originals` (`tools/core_tests.py`, Linux, integrate lane) runs
upstream's own `core-tests/src/coroutine/{hook,socket,system}.cpp` GoogleTests
unchanged on the reference and on the hybrid tree.

Upstream PHPTs for this module run in the merge tier's PHPT gate (the `phpt-*` jobs), which runs every pinned PHPT on upstream and the all-modules hybrid. To attribute a gate regression to this module, see *Attributing a regression* in [corpus/upstream/README.md](../../../corpus/upstream/README.md).

## Known failures

- PHPTs (229), PASS/FAIL/SKIP: Linux reference 183/14/32 vs Rust 181/16/32;
  macOS reference 165/19/45 vs Rust 161/23/45. Rust has candidate-only
  failures on both platforms; base/async_file failures reproduce in the native
  control on macOS. Redis, external services and PHP deprecations account for
  further failures and skips.
- Native originals (101, Linux): reference 93 pass/8 fail, Rust 94 pass/7
  fail. Both fail tcp6, four external proxy cases and two DNS cases; the
  reference TLS verify case timed out. An earlier run showed TLS SIGPIPE in both
  engines.
- macOS (clang): `packet_gate.py` stops at a narrowing error in the selected
  `recv_packet` initializer and `lifecycle_gate.py` at its `native-dtor`
  scenario. Both pass on Linux, which is where CI runs them.
- Not run: enabled io_uring runtime, ZTS/thread mode, the PHP originals after
  the latest wait/poll, retry, datagram and curl/MongoDB policy moves, and
  performance.

## Gate audit (#481)

All six phases are kept. They compile the pinned and patched C++ bodies against
fakes and inject what a running server can't reliably produce: EAGAIN, EINTR,
partial IO, timer and dispatch failures, cancel races and destructor reentry, all
under ASan/UBSan. Neither a PHPT nor the Rust unit tests reach that C++ glue.

- `hooks-ordered`, `hooks-async-policy`, `hooks-packet-policy`,
  `hooks-socket-policy`, `hooks-lifecycle-policy`: the five fault/order matrices above.
- `hooks-compile`: the only C++14 build of the eight patched units, and the
  `SWOOLE_RS_HOOKS` refusal check.

A body these gates slice may move into Rust: the PR that moves it ports the
gate's cases for it into a `crates/hooks` test and drops the body from the gate.
