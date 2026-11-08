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

`http_server.get_package_length` runs on both engines in
`http-native-originals`; on the hybrid, `tools/core_tests.py` gives it the two
constants and crates/http2's settings codec in place of the header.
