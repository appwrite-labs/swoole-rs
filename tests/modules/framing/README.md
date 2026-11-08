# Framing and MIME tests

Packet framing and the MIME table (`crates/framing`); the ABI is in
[native/framing/contract.md](../../../native/framing/contract.md).

## Gates

`tools/dev.py` runs `gate.py` (framing-native), `core.py`
(framing-native-originals, Linux) and the versioned `wrong_eof_setting.phpt` expectation.

```sh
mkdir -p target/reports
rustc --edition=2024 --crate-type=staticlib --crate-name swoole_rs_framing \
  -O -Dwarnings crates/framing/src/lib.rs -o target/libframing.a
python3 tests/modules/framing/gate.py --archive target/libframing.a \
  --sanitize --output target/reports/framing-gate.json
```

- `gate.py` runs the ABI, MIME lookup and malformed-input checks in
  `gate.cc`. Add `--rust-asan` with an instrumented archive to also require
  that a one-past read in the invalid caller is detected.
- `core.py --source S --config config.h --archive A --build-root DIR --output R`
  needs Linux with CMake, GTest and PHP. It builds the unchanged pinned native
  GoogleTests for the reference and for the archive's hybrid tree (every
  module's patches, `overlay_hybrid_tree`), and runs each test
  separately. Any error, skip or missing outcome fails. `--inventory-only`
  lists the tests without building.
- Upstream PHPTs for this module run in the merge tier's PHPT gate (the `phpt-*` jobs), which runs every pinned PHPT on upstream and the all-modules hybrid. To attribute a gate regression to this module, see *Attributing a regression* in [corpus/upstream/README.md](../../../corpus/upstream/README.md).

## Ported GoogleTests

`core-tests/src/protocol/mime_type.cpp` no longer builds. Its TESTs are Rust
tests over the table in `crates/framing/src/mime.rs` (`cargo test -p
swoole-rs-framing`), with the same inputs and expected values:

| Upstream TEST | Rust test |
| --- | --- |
| `mime_type.get` | `mime::tests::get` |
| `mime_type.exists` | `mime::tests::exists` |
| `mime_type.set` | `mime::tests::set` |
| `mime_type.add` | `mime::tests::add` |
| `mime_type.del` | `mime::tests::del` |

## Known failures

- `If_Modified_Since.phpt` returns 200 instead of 304 on macOS in both engines.
- Under the 128-byte allocation ceiling, both engines throw for a 1MiB filename
  prefix, because the reference copies the whole filename too.
