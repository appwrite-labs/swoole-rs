# PHP HTTP binding correctness

The ABI contract is in [docs/http-bindings-abi-v1.md](../../../docs/http-bindings-abi-v1.md).

`Swoole\Http\Cookie` and the `Set-Cookie` line are Rust (`crates/httpbindings`,
`php` feature); the pinned `swoole_http_cookie` and `swoole_http_response` PHPTs
cover them.

## Native differential gates

The response header and trailer and zlib body compression are Rust too
(`crates/phphttp/src/header.rs`, `crates/deflate`), covered by the same PHPT
families.

The request parser (llhttp and multipart callbacks, header semantics, body and
form handling) is Rust over the zend crates (`crates/phphttp/src/parse.rs`);
the pinned `swoole_http_server`, `swoole_http_server_coro` and
`swoole_http2_server` PHPTs cover it.

The `start` settings order (aliases, null/false fallthrough, the compression
switch, refusals and reentrant rewrites) is Rust's cursor
(`crates/httpbindings/src/settings.rs`); its unit tests compare it with
upstream's `start` body across every subset of the twelve keys.

The coroutine server's accept loop is Rust (`crates/httpbindings/src/server.rs`);
its unit tests walk upstream's branches, and the pinned `swoole_http_server_coro`
PHPTs cover it with its handler routing.

Preserved native quirks (asserted by independent witnesses, not fixed): the
pointer-size token fast path, shrinking quote-trim bound, leading-hyphen key
formatting, case-sensitive substring and `q=0` encoding acceptance, uint32
length truncation, a successful `end` return after a failed trailer send, and
`parse_files=false` still publishing multipart files. The native
`LONG_MIN` helper overflow and signed-subtraction overflow in range validation
have no defined reference result and are excluded.

## Known gaps

- Mac's 501-position Request diagnostic has 67 original expectation failures in
  each engine (outcomes match across engines).
- Not covered: actual Zend GC/bailout/allocator faults, optional Brotli/Zstd
  execution, queue custody, cancellation, TLS, HTTP2, forced IO faults and
  complete response lifetime.
