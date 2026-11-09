# ListenPort tests

Contract: [native/ports/contract.md](../../../native/ports/contract.md).

`crates/ports` holds the port policy and receive callbacks (`read.rs`),
unit-tested with `cargo test -p swoole-rs-ports`. `Swoole\Server\Port` is
`crates/phpserver`; `set()`'s options are `port_settings.rs`, unit-tested there.

## Running checks

`tools/dev.py` registers most of these as `ports-*` phases. Placeholders:
`PINNED_REFERENCE` is `target/upstream/src`, `SELECTED_ARCHIVE` the Rust FFI
archive.

Native differential gates (compile the pinned definitions against the hybrid build's
Rust ABI, compare output):

```text
python3 tests/modules/ports/gate.py --source PINNED_REFERENCE --archive SELECTED_ARCHIVE --output REPORT [--sanitize] [--rust-asan]
python3 tests/modules/ports/callback_property.py --source PINNED_REFERENCE --archive SELECTED_ARCHIVE --output REPORT [--sanitize] [--php-config PATH]
```

- `gate.py`: event enum/catalog against the pinned definitions.
- `callback_property.py`: the pinned `getCallback` body against the Rust getter
  (`RUST_GET_CALLBACK`, over `swr_v1_ports_callback_property`), using the real PHP
  binary's `zend_tolower_map` (read via FFI).

PHP runtime checks:

```text
python3 tests/modules/ports/configuration.py --engine ... --build-manifest target/ext/build-manifest.json [--case NAME] --output REPORT
python3 tests/modules/ports/php.py --engine ... --output REPORT
```

- `configuration.py`: setting conversion, reentry, ignored SSL, a throwing
  conversion (reported uncaught, then fatal, as upstream does: #317) and the six
  fatal rejections, compared across engines. Before running it checks that the
  one engine with the ports feature bit is the extension the build manifest
  names (pin and SHA-256) and that it exports `swr_v1_ports_protocol`,
  `_protocol_name`, `_event`, `_callback_property` and `_callback_hook`.
- `php.py`: event registration/lookup, unknown-event warnings, Stringable
  reentry and callback replacement lifetime across engines.
- Upstream PHPTs for this module run in the merge tier's PHPT gate (the `phpt-*` jobs), which runs every pinned PHPT on upstream and the all-modules hybrid. To attribute a gate regression to this module, see *Attributing a regression* in [corpus/upstream/README.md](../../../corpus/upstream/README.md).

## Known failures

- On macOS the original `heartbeat_3` PHPT (upstream TODO) and the Linux-only
  length big-packet PHPT skip, so the strict Mac originals gate stays false.

`ports-sni-originals` runs the two unchanged pinned HTTP-server SNI PHPTs on
the loaded production extension through PHP's official runner. Candidate loops
require the selected extension to pass every case; integrate/compare loops run
both native and selected engines. Missing SSL support remains a failure of this
required gate, never a passing capability exclusion.
