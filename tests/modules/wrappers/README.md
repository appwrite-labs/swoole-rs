# Wrapper validation

The wrapper area is `src/wrapper/event.cc` and `src/wrapper/http.cc` (timer.cc
belongs to the timer module). Rust owns HTTP request/multipart field names and
byte accumulation, completion policy, URL/listener splitting, header
classification, response-header serialization, Content-Length digits, response
send sequencing, event direction/wait admission and event-init ordering. Native
code keeps the public `Context` containers, C++ resource lifetimes, callbacks,
transport, llhttp/multipart parsers, FILE/tmpfile handling and warnings. See
[native/wrappers/contract.md](../../../native/wrappers/contract.md).

`overlay.py` applies `migration/patches/wrappers/` to the pinned `event.cc`/`http.cc`
for the gates; edit the patches with `tools/patches.py`.

## Registered gates

- `gate.py --archive FFI_ARCHIVE --output R [--sanitize] [--rust-asan]`
  (`wrappers-native`): ABI consumer (`gate.cc`) with 20,000 seeded snprintf
  differentials, request/multipart/header policy, null/empty/binary/truncation
  inputs, duplicate parameters, threads, fork and destruction. `--rust-asan`
  also requires an instrumented archive to detect a deliberate one-past read.
- `listener.py` and `response.py`, both
  `--source PINNED --archive A --output R [--core CONFIGURED_SRC] [--include DIR] [--sanitize] [--rust-asan]`
  (`wrappers-listener`, `wrappers-response`): extract the pinned and patched
  method bodies, compile them into a model driver and compare them (listener:
  8,213 address-prefix/ABI cases; response: 5,760 `Context::end` traces plus
  16,392 Content-Length decimal comparisons). With `--core`, all four
  original/patched event/http translation units are syntax-checked against the
  configured SDK.
- Upstream PHPTs for this module run in the merge tier's PHPT gate (the `phpt-*` jobs), which runs every pinned PHPT on upstream and the all-modules hybrid. To attribute a gate regression to this module, see *Attributing a regression* in [corpus/upstream/README.md](../../../corpus/upstream/README.md).

## Other gates

- `core.py --source S --config CONFIG_H --archive A --build-root DIR --output R [--library-only] [--openssl-root P]`:
  builds the upstream tree and the archive's hybrid tree with CMake and runs the
  unchanged reactor/timer GTests and the four HTTP wrapper tests (parser1,
  parser2, upload, max_request_size) one process each. `--library-only` builds
  the libraries without running tests (used by the next three scripts).
- `native.py --core-root DIR --output R [--openssl-root P]`: loopback
  Server/Context/SyncClient application against both native libraries,
  comparing response bytes, public maps, form/file content and file cleanup
  over five scenarios. Each run verifies the loaded library path and hash.
- `benchmark.py --prepare DIR --core-root CORE [--openssl-root P]`, then
  `benchmark.py --prepared DIR [--output R]`: event add/update consumer.
  Preparation verifies loaded images without reading the clock; `--prepared`
  times five alternating trials. Run timings with no other workload.
- `python3 -m unittest tests/modules/wrappers/test_model_checkpoints.py`:
  Python-only test of the `response.py` runner's partial-report checkpoints.

Example:

```sh
python3 tests/modules/wrappers/response.py --source target/upstream/src \
  --archive target/.../libswoole_rs_ffi.a --core target/.../src \
  --include /dependency/include --sanitize --output target/reports/response.json
```

## Known failures

- `core.py` on macOS: the upstream HTTP test unit uses Linux `stat.st_mtim`
  and does not compile; only `--library-only` works there.
- On Linux Docker bind mounts, libtool's install copy can fail with
  `error deallocating ... Invalid argument`; a PATH `cp` shim passing
  `--sparse=never --reflink=never` works around it.
