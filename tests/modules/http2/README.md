# HTTP/2 tests

`crates/http2` holds the settings and frame codecs and the HTTP/2 policy;
`crates/phphttp2` holds the server (`server.rs`) and
`Swoole\Coroutine\Http2\Client`. The HTTP/2 PHPTs in the PHPT gate cover both.

`cargo test -p swoole-rs-http2` runs the unit tests and upstream's
`core-tests/src/protocol/http2.cpp` TESTs, ported with the same inputs and
expected values since the hybrid has no `swoole_http2.h` to build them against:

| Upstream TEST | Rust test |
| --- | --- |
| `http2.default_settings` | `http2_default_settings` |
| `http2.pack_setting_frame` | `http2_pack_setting_frame` |
| `http2.get_type` | `http2_get_type` |
| `http2.get_type_color` | `http2_get_type_color` |
| `http2.ssl` | not ported: needs the nghttp2 client |

## HPACK

`crates/http2/src/hpack.rs` replaces the nghttp2 HPACK subset upstream bundles
(`thirdparty/nghttp2/nghttp2_hd*.c`), coding as it does: the same indexing
choices, table size updates, limits and `NGHTTP2_ERR_HEADER_COMP` failures.
`hpack.rs` (`cargo test -p swoole-rs-http2 --test hpack`) decodes every RFC 7541
Appendix C example, whole and in fragments, with the table sizes listed there;
encodes C.4's requests to the same bytes; checks the encoder's indexing and table
size updates and the decoder's errors; and decodes encoded and corrupted blocks
on one connection alongside crates/oracle's nghttp2 inflater, which must agree on
every field and failure. The `http2_hpack` fuzz target runs the same comparison
step by step.

`http_server.get_package_length` is a Rust case in `tests/modules/http/server`
(`http-server-rust`), packing its settings frame with crates/http2's codec.

## Conformance

`h2spec.py` runs h2spec (pinned release and SHA256, strict cases included) against
`h2spec.php`, an `open_http2_protocol` server, over h2c and TLS on each engine. A
case that passes on upstream and not on another engine fails the run; upstream's own
failures are listed in the report, not hidden. The merge tier runs it on upstream
and the Cargo build (`.github/workflows/h2spec.yml`), as does a PR that touches
HTTP/2 code.

```sh
python3 tests/modules/http2/h2spec.py --engine upstream=target/upstream/src/modules/swoole.so \
  --engine cargo=target/ext/swoole.so
```
