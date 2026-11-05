# HTTP/2 boundary (M24)

Feature value 4194304. Upstream targets:
`src/protocol/http2.cc`, `ext-src/swoole_http2_server.cc`,
`ext-src/swoole_http2_client_coro.cc`. Frame/Settings/Session/Stream/Context
layouts and PHP declarations are unchanged. Follows the
[policy/effect split](abi-v1.md#policyeffect-split).

Rust owns settings, frame, header, stream, request/response, window, chunk and
control policy. HPACK/nghttp2, Zend values, native Context helpers, sockets,
crypto, compression and I/O are native effects. `src/protocol/http2.cc` is
deleted: Rust exports its functions under their upstream symbols and owns the
process-wide default settings; `swoole_http2.h` keeps `unpack_setting_data`
(a `std::function` callback) and `send_setting_frame` (`Socket::send`) inline.
Server session/stream maps and client stream map/settings/windows remain. The
empty `server_ips`/`client_ips` maps are unused.

- Settings unpack calls a native callback per item and keeps stop/error
  behavior, rereading bytes and defaults after each mutation. Zero or negative
  signed length returns success. A partial final item is not rejected when the
  original buffer permits the read; there is no `length % 6 == 0` check and no
  eager decode. One original native test passes the whole frame length to the
  payload decoder; it is kept.
- Kept: role-specific settings/HPACK effects, unknown-setting handling, unsigned
  windows, send/chunk/end order, header locale/case/CRLF/trim, PHP cookie
  encoding, nested headers and compression defaults, client RST/GOAWAY, ignored
  PUSH_PROMISE, stream-id and pipeline publication, queue limit/pop/release/
  error order. Incomplete upstream CONTINUATION/push/coroutine-window support is
  left as is.

## Client

`Swoole\Coroutine\Http2\Client`, its send queue included, is Rust
(`crates/phphttp2/src/client.rs`, #827).

Original inventory: 70 PHPTs (44 direct HTTP/2), 5 direct and 5 cross native
tests. HPACK is embedded 1.64.0 or external nghttp2.
