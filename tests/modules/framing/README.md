# M18 framing and MIME

Packet framing and the immutable MIME policy behind the optional framing feature
(bit4096). The native C++ container and the native IO/callback/storage ABI shims
are described in [native/framing/contract.md](../../../native/framing/contract.md).
Default activation is not claimed.

## Gates

`tools/dev.py` runs `gate.py` (framing-native), `trace.py` (framing-traces), `core.py`
(framing-native-originals, Linux) and the versioned `wrong_eof_setting.phpt` expectation.

```sh
mkdir -p target/reports
python3 tests/modules/framing/trace.py --archive target/hybrid/BUILD/src/../rust-prefix/lib/libswoole_rs_ffi.a \
  --output target/reports/framing-trace.json
rustc --edition=2024 --crate-type=staticlib --crate-name swoole_rs_framing \
  -O -Dwarnings crates/framing/src/lib.rs -o target/libframing.a
python3 tests/modules/framing/gate.py --archive target/libframing.a \
  --sanitize --output target/reports/framing-gate.json
```

- `trace.py` compiles the exact pinned C++ bodies and the same files from the
  hybrid build's patched tree (linked to its archive)
  against instrumented IO/String boundaries and compares the traces line by
  line. The traces cover partial packets, split residuals, callback
  failure/removal ordering, reentry, TLS pending, allocation exceptions and
  dynamic-header offset/body/header_len ordering.
- `gate.py` runs the ABI, catalog, locale-suffix and malformed-input checks in
  `gate.cc`. Add `--rust-asan` with an instrumented archive to also require
  that a one-past read in the invalid caller is detected.
- `core.py --source S --config config.h --archive A --build-root DIR --output R`
  needs Linux with CMake, GTest and PHP. It builds the unchanged pinned native
  GoogleTests for the reference and for the archive's hybrid tree (every
  module's patches, `overlay_hybrid_tree`), and runs each test
  separately. Any error, skip or missing outcome fails. `--inventory-only`
  lists the tests without building.
- Upstream PHPTs for this module run in the merge tier's PHPT gate (the `phpt-*` jobs), which runs every pinned PHPT on upstream and the all-modules hybrid. To attribute a gate regression to this module, see *Attributing a regression* in [corpus/upstream/README.md](../../../corpus/upstream/README.md).

## Known failures

- PHP originals: macOS has 43 pass, 2 skip and 2 fail; Linux has 45 pass,
  1 skip and 1 fail. Both engines get the same results.
- On macOS, `If_Modified_Since.phpt` returns 200 instead of 304 in both engines.
- The SMTP online test skips. macOS also skips the Linux-only large-packet test.
- Under the 128-byte allocation ceiling, both engines throw for a 1MiB filename
  prefix, because the reference copies the whole filename too. This is not a
  migration regression. The uncapped control matches on both platforms.
- Still open: optimized measurements, broader platform/feature/combined
  activation, and the remaining original failures.
