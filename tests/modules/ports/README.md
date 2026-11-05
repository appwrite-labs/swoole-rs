# M25 ListenPort configuration

Contracts: [native/ports/contract.md](../../../native/ports/contract.md) and
[docs/ports-abi-v1.md](../../../docs/ports-abi-v1.md). Feature bit
8388608 (`ports`).

Rust owns protocol priority/labels, integer normalization, event lookup,
listener option sequencing, socket-creation decisions, the init_protocol plan,
`get_connection_num` summation, EOF/length/stream setter normalization,
constructor defaults, callback getter/hook selection, flag/buffer/packet/option
plans, TLS/SNI frontend metadata, class-property declarations and the receive
callbacks (`read.rs`, unit-tested with `cargo test -p swoole-rs-ports`). The PHP
class `Swoole\Server\Port` (registration, objects, every method) is
`crates/phpserver`; its `set()` reaches the ListenPort/TLS block that stays C++
(`port_apply_settings`) through `native/ports/swoole_rs_phpserver.h`. The
`flags`, `options`, `packet_settings` and `tls_settings` gates extract that block;
`callback_property` compares the original getter with the Rust one's body. Native
code keeps allocation, Zend storage and conversions, syscalls, callbacks,
warnings, cleanup, crypto, the HTTP request parser and request destruction.

## Running checks

`tools/dev.py` registers most of these as `ports-*` phases. Placeholders:
`PINNED_REFERENCE` is `target/upstream/src`, `SELECTED_ARCHIVE` the Rust FFI
archive, `CONFIGURED_CORE` a configured upstream/hybrid `src` tree.

Native differential gates (extract exact original bodies from the pin and selected
ones from the hybrid build's tree, compile both against scripted native effects,
compare output):

```text
python3 tests/modules/ports/gate.py --source PINNED_REFERENCE --archive SELECTED_ARCHIVE --output REPORT [--sanitize] [--rust-asan]
python3 tests/modules/ports/listen.py --source PINNED_REFERENCE --archive SELECTED_ARCHIVE --output REPORT [--sanitize]
python3 tests/modules/ports/socket_workflow.py --source PINNED_REFERENCE --archive SELECTED_ARCHIVE --core CONFIGURED_CORE --output REPORT [--sanitize]
```

- `gate.py`: integer macros, event enum/catalog and protocol plan against the
  pinned definitions.
- `listen.py`: `ListenPort::listen` bodies, 1536 traces must match.
- `socket_workflow.py`: `create_socket` bodies, 1280 traces must match. Reads
  configured Makefile `INCLUDES`/`CPPFLAGS` as literal argv; shell expressions,
  unknown variables and cycles are rejected and no Make command runs.

Exact-body model gates share the `count.py` runner (`main()`), which extracts the
named bodies (original from the pin, selected from the hybrid build's tree), compiles
each side's witness translation unit and links them to the archive. With `--core`
(a configured pinned tree) it syntax checks the full original `port.cc` /
`swoole_server_port.cc` there and the selected ones in the hybrid build's tree:

```text
python3 tests/modules/ports/NAME.py --source PINNED_REFERENCE --archive SELECTED_ARCHIVE --output REPORT [--core CONFIGURED_CORE] [--sanitize] [--rust-asan] [--openssl-root DIR] [--php-config PATH] [--sdk-origin ORIGINAL_SRC] [--include DIR]
```

| NAME | Checks |
| --- | --- |
| `count` | `get_connection_num` over shared counters |
| `settings` | EOF/length/stream setters: EOF capacity/direction, NUL bytes, every length-type byte, u16 offset truncation |
| `defaults` | `ListenPort` constructor defaults |
| `callback_property` | `getCallback` lowering using the real PHP binary's `zend_tolower_map` (read via FFI; needs `--php-config`) |
| `callback_hooks` | post-registration hook binding over all slot masks and event indices |
| `class_properties` | MINIT property declarations against inert Zend sinks |
| `flags` | non-TLS `set` prefix: flag settings and buffer clamp/fallback order |
| `packet_settings` | same prefix plus binary EOF, width and offset plans |
| `options` | same prefix through the ordinary option catalog |
| `tls_settings` | TLS option order against successful native models |
| `sni_settings` | SNI mapping admission against successful native models |

`--sdk-origin` relocates in-build include paths when compiling a private copy of
a configured SDK. On macOS pass a matching `--php-config` and `--openssl-root`.

Linux sanitizer run (builds an ASan Rust archive and reruns `gate.py`,
`listen.py` and `socket_workflow.py`, then checks that a deliberately invalid
ABI caller is caught):

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

Unit tests (no native build):

```text
cd tests/modules/ports && python3 -m unittest test_configured_flags test_count_checkpoints
```

## Known failures

- `configuration.py` fails in both engines: a throwing package-length
  Stringable makes both warn and fatally reject the length type instead of
  propagating the exception. The conversion block is still native.
- On macOS the original `heartbeat_3` PHPT (upstream TODO) and the Linux-only
  length big-packet PHPT skip, so the strict Mac originals gate stays false.
- A Rust float conversion for `max_idle_time` failed on signaling NaN with Apple
  Clang and was dropped; that conversion stays native.

Open: broader Server originals, getSocket/platform/real TLS coverage,
exceptions/bailout/teardown, production sanitizer runs and benchmarks.

`ports-sni-originals` runs the two unchanged pinned HTTP-server SNI PHPTs on
the loaded production extension through PHP's official runner. Candidate loops
require the selected extension to pass every case; integrate/compare loops run
both native and selected engines. Missing SSL support remains a failure of this
required gate, never a passing capability exclusion.
