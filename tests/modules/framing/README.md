# Framing and MIME tests

Packet framing and the MIME table (`crates/framing`); the ABI is in
[native/framing/contract.md](../../../native/framing/contract.md).

## Gates

- `cargo test -p swoole-rs-framing` (framing-unit) runs the receive loop, unpack,
  length and MIME tests. The crate forbids `unsafe_code` and exports nothing to C,
  so it has no ABI gate; the value checks the old one made are Rust tests.
- The `framing_unpack`, `framing_length` and `framing_stream` fuzz targets
  (`fuzz/`) compare against the pinned base.cc.
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

`core-tests/src/protocol/base.cpp` no longer builds either. Its server TESTs are
cases in [protocol.rs](protocol.rs), built into the http server case binary
(`tests/modules/http/server`, the merge tier's http-server-rust phase), whose
`src/http1/case.rs` builds the Server and its port's framing. The rest are unit tests
(`cargo test -p swoole-rs-ports -p swoole-rs-protocols-ffi`):

| Upstream TEST | Rust test |
| --- | --- |
| `protocol.length` | `protocol.length` (http-server-rust) |
| `protocol.length_2` | `protocol.length_2` (http-server-rust) |
| `protocol.length_3` | `protocol.length_3` (http-server-rust) |
| `protocol.eof` | `protocol.eof` (http-server-rust) |
| `protocol.socks5_strerror` | `swoole-rs-protocols-ffi` `owned_proxy::tests::socks5_strerror` |
| `protocol.socks5_dns_tunnel_target_host_max_length` | `swoole-rs-protocols-ffi` `owned_proxy::tests::socks5_dns_tunnel_target_host_max_length` |
| `protocol.swap_byte_order` | `swoole-rs-ports` `framing_recv::tests::swap_byte_order` |
| `protocol.unpack` | `swoole-rs-ports` `framing_recv::tests::unpack_formats` |
| `protocol.hton64` | `swoole-rs-ports` `framing_recv::tests::hton64` |

## Known failures

- `If_Modified_Since.phpt` returns 200 instead of 304 on macOS in both engines.
- Under the 128-byte allocation ceiling, both engines throw for a 1MiB filename
  prefix, because the reference copies the whole filename too.
