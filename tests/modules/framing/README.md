# Framing and MIME tests

Packet framing and the MIME table (`crates/framing`); the ABI is in
[native/framing/contract.md](../../../native/framing/contract.md).

## Gates

`tools/dev.py` runs `gate.py` (framing-native) and the versioned
`wrong_eof_setting.phpt` expectation.

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
`fixture.cc` builds the Server and its port's framing. The rest are unit tests
(`cargo test -p swoole-rs-ports -p swoole-rs-protocols`):

| Upstream TEST | Rust test |
| --- | --- |
| `protocol.length` | `protocol.length` (http-server-rust) |
| `protocol.length_2` | `protocol.length_2` (http-server-rust) |
| `protocol.length_3` | `protocol.length_3` (http-server-rust) |
| `protocol.eof` | `protocol.eof` (http-server-rust) |
| `protocol.socks5_strerror` | `swoole-rs-protocols` `owned_proxy::tests::socks5_strerror` |
| `protocol.socks5_dns_tunnel_target_host_max_length` | `swoole-rs-protocols` `owned_proxy::tests::socks5_dns_tunnel_target_host_max_length` |
| `protocol.swap_byte_order` | `swoole-rs-ports` `framing_recv::tests::swap_byte_order` |
| `protocol.unpack` | `swoole-rs-ports` `framing_recv::tests::unpack_formats` |
| `protocol.hton64` | `swoole-rs-ports` `framing_recv::tests::hton64` |

## Known failures

- `If_Modified_Since.phpt` returns 200 instead of 304 on macOS in both engines.
- Under the 128-byte allocation ceiling, both engines throw for a 1MiB filename
  prefix, because the reference copies the whole filename too.
