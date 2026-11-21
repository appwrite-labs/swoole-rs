# PHP HTTP binding tests

Contract: [native/httpbindings/contract.md](../../../native/httpbindings/contract.md).

The PHP HTTP classes are Rust: `crates/phphttp` (cookies, request parser,
response headers and trailers, server and coroutine server, WebSocket) over the
policy in `crates/httpbindings` and the zlib bodies in `crates/deflate`. Their
unit tests cover the pure policy; for example `settings.rs` compares the `start`
settings order with upstream's across every subset of its keys, and `server.rs`
walks the coroutine accept loop's branches. The pinned `swoole_http_server`,
`swoole_http_server_coro`, `swoole_http_cookie`, `swoole_http_response`,
`swoole_http2_server` and `swoole_websocket_server` PHPTs in the PHPT gate cover
the PHP behaviour.
