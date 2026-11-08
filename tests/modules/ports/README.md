# M25 ListenPort configuration

Contracts: [native/ports/contract.md](../../../native/ports/contract.md) and
[docs/ports-abi-v1.md](../../../docs/ports-abi-v1.md). Feature bit
8388608 (`ports`).

Rust owns protocol priority/labels, integer normalization, event lookup,
`listen` and `close_socket` (`methods.rs`), socket-creation decisions, `init_protocol`,
`get_connection_num` summation, the EOF/length/stream setters and the
protocol defaults over the port's Rust-owned `protocol`, callback getter/hook selection,
class-property declarations and the receive
callbacks (`read.rs`, unit-tested with `cargo test -p swoole-rs-ports`). The PHP
class `Swoole\Server\Port` (registration, objects, every method) is
`crates/phpserver`; `set()`'s options are `port_settings.rs`, unit-tested there, and
reach the SNI contexts and `ssl_init` through `native/ports/swoole_rs_phpserver.h`.
`callback_property` compares the original getter with the Rust one. Native
code keeps allocation, Zend storage, syscalls, callbacks,
warnings, cleanup, crypto, the HTTP request parser and request destruction.

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
python3 tests/modules/ports/configuration.py --engine ... --build-manifest MANIFEST --archive SELECTED_ARCHIVE --source-ref REF [--case NAME] --output REPORT
python3 tests/modules/ports/php.py --engine ... --output REPORT
```

- `configuration.py`: setting conversion, reentry, ignored SSL, a throwing
  conversion (reported uncaught, then fatal, as upstream does: #317) and the six
  fatal rejections, compared across engines. The build-manifest options verify,
  before running, that the candidate DSO, archive, M25 Rust source, header and
  both patches match the manifest and that the DSO exports every
  `swr_v1_ports_*` symbol.
- `php.py`: event registration/lookup, unknown-event warnings, Stringable
  reentry and callback replacement lifetime across engines.
- Upstream PHPTs for this module run in the merge tier's PHPT gate (the `phpt-*` jobs), which runs every pinned PHPT on upstream and the all-modules hybrid. To attribute a gate regression to this module, see *Attributing a regression* in [corpus/upstream/README.md](../../../corpus/upstream/README.md).

## Known failures

- `configuration.py` fails in both engines: a throwing package-length
  Stringable makes both warn and fatally reject the length type instead of
  propagating the exception.
- On macOS the original `heartbeat_3` PHPT (upstream TODO) and the Linux-only
  length big-packet PHPT skip, so the strict Mac originals gate stays false.

Open: broader Server originals, getSocket/platform/real TLS coverage,
exceptions/bailout/teardown, production sanitizer runs and benchmarks.

`ports-sni-originals` runs the two unchanged pinned HTTP-server SNI PHPTs on
the loaded production extension through PHP's official runner. Candidate loops
require the selected extension to pass every case; integrate/compare loops run
both native and selected engines. Missing SSL support remains a failure of this
required gate, never a passing capability exclusion.
