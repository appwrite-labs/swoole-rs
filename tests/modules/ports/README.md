# ListenPort tests

Contract: [native/ports/contract.md](../../../native/ports/contract.md).

`crates/ports` holds the port policy and receive callbacks (`read.rs`),
unit-tested with `cargo test -p swoole-rs-ports`. `Swoole\Server\Port` is
`crates/phpserver`; `set()`'s options are `port_settings.rs`, unit-tested there.

`core-tests/src/server/port.cpp`'s TESTs are `crates/ports/src/methods.rs` tests, same
inputs and expected values (`cargo test -p swoole-rs-ports --all-features server_port`):

| Upstream TEST | Rust test |
| --- | --- |
| `server_port.import` | `methods::tests::server_port_import` (Linux: macOS answers `SO_ACCEPTCONN` with `ENOPROTOOPT`) |
| `server_port.create` | `methods::tests::server_port_create` |
| `server_port.dgram` | `methods::tests::server_port_dgram` |

The `Server` is the tests' head over a `ServerConfig` with `enable_reuse_port`, and
`add_port` is the `create_socket` it calls. `swoole_get_last_error()` is set by the log
host (crates/utilities' in production), so the tests install `read.rs`'s test host, which
sets it the same way. `cert_file` is written into its slot, as upstream assigns the string,
without the setter's file check.

`core-tests/src/server/server.cpp`'s `server.ssl_matches_wildcard_name` starts no server, so
it is `lifecycle::tests::server_ssl_matches_wildcard_name`, over the Rust
`ListenPort::ssl_matches_wildcard_name`: upstream's 44 live `EXPECT`s with the same subject,
certificate name and expected result, in its order (its commented-out ones stay out).
server.cpp's other TLS TESTs start a server: tests/modules/http/README.md maps them.

`core-tests/src/server/server.cpp`'s `server.protocols` reads only its port's
`get_protocols()` and `clear_protocol()`, so it is `methods::tests::server_protocols`
over a port of its own, with the same flags and expected names (`cargo test -p
swoole-rs-ports --all-features server_protocols`). The rest of server.cpp is
`tests/modules/http/server` (see the HTTP module's tests README).

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
- Upstream PHPTs for this module run in the merge tier's PHPT gate (the `phpt-*` jobs), which runs every pinned PHPT on upstream and the Cargo build. To attribute a gate regression to this module, see *Attributing a regression* in [corpus/upstream/README.md](../../../corpus/upstream/README.md).

## Known failures

- On macOS the original `heartbeat_3` PHPT (upstream TODO) and the Linux-only
  length big-packet PHPT skip, so the strict Mac originals gate stays false.

`ports-sni-originals` runs the two unchanged pinned HTTP-server SNI PHPTs on
the loaded production extension through PHP's official runner. Candidate loops
require the selected extension to pass every case; integrate/compare loops run
both native and selected engines. Missing SSL support remains a failure of this
required gate, never a passing capability exclusion.
