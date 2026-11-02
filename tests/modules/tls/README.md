# TLS validation

Rust creates and configures `SSLContext`'s `SSL_CTX` and owns its callbacks
(`crates/tls/src/context.rs`), and decides the DTLS BIO read/control/listen
results. Native code keeps the configuration fields, the info callback, the DTLS
Session and BIO method table.

## differential.py

Compiles the complete pinned `src/protocol/ssl.cc` and `dtls.cc` against the
pinned headers and links them to the pinned native core library; compiles the
hybrid build's fully patched copies against its patched headers (the tree of
`--archive`) and links them to a native core library built from that same tree.
Then it compares their output. The patched driver must contain definitions and
call instructions for every `swr_v1_tls_*` function it uses, the reference none.
Loader output, both native libraries and the loaded libssl/libcrypto are checked
to be unchanged across the run. Drivers reach DTLS state only through public
calls (`Session::append`, the BIO callbacks), never `Session`'s queue or peek mode.

```sh
python3 tests/modules/tls/differential.py --reference PINNED_GIT \
  --source CONFIGURED_HEADER_TREE --library NATIVE_LIBSWOOLE \
  --hybrid-library HYBRID_LIBSWOOLE --archive HYBRID_FFI_ARCHIVE \
  --openssl OPENSSL_PREFIX --output R [--sanitize] \
  [--gtest-prefix PREFIX] [--peers | --allocation | --crypto-allocation | --benchmark-check]
```

- default (`driver.cc`): 256 context masks x 2 compression flags, encrypted
  cert/key/passphrase setup and native setup failures (missing cert/key/CA,
  invalid cipher/curve, wrong passphrase, DH and curve variants), 40 BIO
  length/request/peek cases and the four expected unknown-command warnings.
  Only the logger's timestamp/PID prefix is normalized.
- `--gtest-prefix`: also builds unchanged upstream
  `core-tests/src/protocol/ssl.cpp` with `original_main.cc` and runs
  `ssl.destroy`, `ssl.get_error`, `ssl.password` one process each.
- `--peers` (`peers.cc`): in-memory TLS 1.2/1.3 peers (trust, hostname, ALPN,
  password callback, DHE/ECDHE, data, two-sided shutdown), DTLS over an
  AF_UNIX datagram pair with cookie generation/verification and
  `Session.listen`, native re-init cycles, four native threads, info callback
  events and fork.
- `--allocation` (`allocation.cc`): replaceable C++ `operator new` failures
  through the DTLS `BIO_ctrl` callback's native warning (SSLContext's warnings
  are formatted in Rust). Each side's trace is checked against its own recorded
  profile (`ALLOCATION_PROFILES` in `differential.py`): upstream's warning makes
  three C++ allocations, the hybrid's (whose logger is utilities' Rust) one.
- `--crypto-allocation --crypto-operation client-ca|dhparam|curve|dtls-init`
  (`crypto_allocation.cc`): OpenSSL allocator-hook single-failure sweep.
- `--benchmark-check` (`benchmark.cc`): prepares six fixed optimized
  workloads and verifies their checksums without timing anything.
- `--sanitize` adds ASan/UBSan to the native TUs and driver only (not Rust, the
  native DSO or OpenSSL); leak detection is off.

## gate.py (registered as `tls-native`)

```sh
python3 tests/modules/tls/gate.py --source PINNED_GIT --config SELECTED_CONFIG_H \
  --archive SELECTED_FFI --output R --jobs N [--gtest-prefix PREFIX] \
  [--native-report REPORT] [--allocation-native-report REPORT]
```

Runs `differential.py` against the supplied FFI archive in three children:
sanitized setup with original GTests, sanitized peers, and ordinary
allocation. It first checks the archive exports ABI 1 with TLS feature bit
262144, then builds `lib-swoole` from the archive's hybrid tree with CMake
(cached in `target/native-core/hybrid-*`, keyed by the archive, the tree's
`config.h` and its patch state). The pinned native library comes from an existing successful build report
(the coroutine `core.py` report, a `tools/native_core.py` report, or a corebase
outer report wrapping one) whose library and compiled config must match.
Without `--native-report` it tries `coroutine-native-originals.json` next to
the output. The allocation child needs an uninstrumented native library; if
the main control has sanitizer symbols, pass `--allocation-native-report`.

## php.py

`--dh-witness` adds `php-dh.phpt` (DHE server with the pinned DH params at
security level 0); `--dh-witness-only` runs just that.

## Unit tests

```sh
python3 -m unittest tests/modules/tls/test_gate.py tests/modules/tls/test_php.py
```

Python-only checks of `gate.py` control-report acceptance, `differential.py`
argument guards and timeout handling, and `php.py` export parsing.

## Known gaps and failures

- Upstream `ssl.password` uses an uninitialized `SSLContext`; it passes, but on
  undefined input.
- `verify_02` has been observed WARNED (flaky) in the PHP cohort.
- An empty curve list is accepted by OpenSSL 3.6 and rejected by 3.0; the
  curve fixture probes OpenSSL directly rather than hard-coding either.
- A native library built without DTLS/listen would fail the caller check;
  there is no capability handling for that yet.
- Not covered: real IP sockets, packet loss, PHP cancellation, legacy/no-ECDH
  OpenSSL configurations, PHP coverage of the curve/info/session/password
  conversions, macOS PHP runs and performance.
