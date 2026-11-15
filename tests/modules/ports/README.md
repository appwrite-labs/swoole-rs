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

`tools/dev.py` registers most of these as `ports-*` phases.

### Event catalog and `getCallback` against the pin

`catalog.rs` (the `catalog` test of `swoole-rs-ports`, in the `ports-unit` phase) is the
former native gates `gate.cc` and `callback_property.cc`: the same names against the
same pinned (v6.2.5) definitions, which `crates/oracle` compiles unchanged (`ports`: the
callback enum of `ext-src/php_swoole_server.h`, `server_port_event_map` and the
`getCallback` body of `ext-src/swoole_server_port.cc`, and swoole.h's
`swoole_strcaseeq`). Each C export the gates called is replaced by the safe function
that was its body. The exports, their header (`swoole_rs_ports.h`), the gates' scripts
and the `ports-policy` and `ports-callback-property` phases are gone.

| Gate checks | Rust test (`catalog.rs`) | Export → function |
| --- | --- | --- |
| `gate.cc`: every key, its upper case, every byte at every position of every key, `""`, `unknown` and `connect\0`, matched exactly and folded, index and property against the pinned catalog and `swoole_strcaseeq`: 108,647 checks | `event_catalog` | `swr_v1_ports_event` → `event` |
| `callback_property.cc`: the pinned `getCallback` body against the Rust getter over the keys, their upper case with every byte at every position, four fixed names and 4096 random ones (xorshift32, seed `0x81715`), with every property set and every property null, in the `C`, environment and `tr_TR.UTF-8` locales: 62,520 comparisons per available locale (187,560 with all three) of the returned property and the order of the Zend calls | `get_callback` | `swr_v1_ports_callback_property` → `callback_property` |

The gate's last check passed a null span of no bytes; a slice cannot be null, so the
test passes an empty name. The getter's two sides ran in one model of the Zend calls
(string conversion, `zend_string_tolower`, the silent property read, the copy); the
pinned side still does, in `crates/oracle/shim/ports.cc`, and the Rust side is
`crates/phpserver/src/port.rs`'s `get_callback` in the same terms (`rust_get_callback`,
as `callback_property.py`'s `RUST_GET_CALLBACK` was). `callback_property.py` read PHP's
`zend_tolower_map` through FFI and required it to fold exactly `A`-`Z`; the model folds
exactly those, and nothing checks the PHP binary's table any more. The gates ran their
C++ under ASan/UBSan; the tests don't. A locale the host lacks is reported on stderr,
not passed, and a host with none fails.

PHP runtime checks:

```text
python3 tests/modules/ports/configuration.py --engine ... --build-manifest target/ext/build-manifest.json [--case NAME] --output REPORT
python3 tests/modules/ports/php.py --engine ... --output REPORT
```

- `configuration.py`: setting conversion, reentry, ignored SSL, a throwing
  conversion (reported uncaught, then fatal, as upstream does: #317) and the six
  fatal rejections, compared across engines. Before running it checks that the
  one engine with the ports feature bit is the extension the build manifest
  names (pin and SHA-256).
- `php.py`: reflected property order, visibility and defaults; event registration/lookup, unknown-event warnings, Stringable
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
