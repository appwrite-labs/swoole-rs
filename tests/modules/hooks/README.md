# Hooks

Pin `c7e514509451beafd4d70ce5b78943d51e76381c`.

## Native-facade gates

```sh
rustc --edition 2024 --crate-type staticlib --crate-name swoole_rs_hooks \
  -Copt-level=2 -Cpanic=abort crates/hooks/src/lib.rs -o OWN/libhooks.a
python3 tests/modules/hooks/ordered_gate.py --source PINNED_SOURCE --archive OWN/libhooks.a --sanitize --report OWN/ordered.json
python3 tests/modules/hooks/async_gate.py --source PINNED_SOURCE --archive OWN/libhooks.a --sanitize --report OWN/async.json
python3 tests/modules/hooks/packet_gate.py --source PINNED_SOURCE --archive OWN/libhooks.a --sanitize --report OWN/packet.json
python3 tests/modules/hooks/socket_policy_gate.py --source PINNED_SOURCE --archive OWN/libhooks.a --sanitize --report OWN/socket.json
python3 tests/modules/hooks/lifecycle_gate.py --source PINNED_SOURCE --archive OWN/libhooks.a --sanitize --report OWN/lifecycle.json
python3 tests/modules/hooks/compile_gate.py --source PINNED_SOURCE --config ACTUAL_GENERATED_CONFIG --report OWN/compile.json
```

Each `*_gate.py` extracts the pinned and patched function bodies, compiles both
against a deterministic native facade (`*_fixture.h`) and runs identical
scenarios through each, comparing effects and order. Syscalls, contexts, TLS
and kernel IO are faked; these are policy/order checks, not runtime proof.
`--sanitize` adds ASan/UBSan to C++ only, not the Rust archive.

- `ordered_gate.py` (2021 cases): hook guard short circuit; flock routing,
  retry, cancel and invalid precedence; socket partial reads/writes, barrier
  wait/error/EOF/timer failure and retry policy; recvmsg/sendmsg/sendto/recvfrom
  paths; io_uring submission; system poll
  error-to-read/write mapping; wait/poll and `System::wait_event`;
  TimerController start/destructor; 121 requested/installed hook flag pairs
  including the native/PHP curl conflict; all 19 stream/transport/PDO/function
  families; MongoDB install-only aliases.
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

```sh
python3 tests/modules/hooks/core.py --receipt upstream=NATIVE_CORE_RECEIPT --receipt hybrid=NATIVE_CORE_RECEIPT --output REPORT
```

Upstream PHPTs for this module run in the merge tier's PHPT gate (the `phpt-*` jobs), which runs every pinned PHPT on upstream and the all-modules hybrid. To attribute a gate regression to this module, see *Attributing a regression* in [corpus/upstream/README.md](../../../corpus/upstream/README.md).

`core.py` runs the 100 `TEST()` declarations of the pinned
`core-tests/src/coroutine/{hook,socket,system}.cpp` (plus the `accept` dependency) against completed
`tools/native_core.py` builds, after verifying the original assertion files,
executable and loaded library match the receipt. It runs the unchanged
`base.file_put_contents` test first to create `TEST_TMP_FILE`.

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

`core.py` is not a phase. No CI lane produces the `tools/native_core.py` receipts
it needs, and some of its cases need external network. Its 8,150-line
`originals.json` inventory is replaced by the three pinned hashes above. Only
those hashes and the test names were ever read, and the names now come from the
source.
