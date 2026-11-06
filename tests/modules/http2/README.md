# HTTP/2 policy conversion

Rust (`crates/http2`) owns the settings/frame codecs, role-specific SETTINGS
decisions, body chunking, window actions, frame header and control payload
integers, and next-effect selection for
several server methods. `crates/phphttp2/src/server.rs` drives the server's
frames and responses over the C++ sessions, streams and contexts, which keep
the stores, HPACK, socket, compression, coroutine and destructor effects.
Rust also owns the process-wide default settings.
`Swoole\Coroutine\Http2\Client` is Rust (`crates/phphttp2`, #827); its PHPTs
cover it.
The overlay has no native fallback: patched builds define `SW_USE_RUST_HTTP2`.

## Tests

The codecs, settings, frame lengths and header lookups have Rust unit tests
(`cargo test -p swoole-rs-http2`). Upstream's own `http2` GoogleTests
(`core-tests/src/protocol/http2.cpp`) run on both engines in the
`http2-native-originals` phase. The server's frames, responses and request header
parser are Rust too (`crates/phphttp2/src/server.rs`, #757); its PHPTs cover them.

## Original PHPTs

Upstream PHPTs for this module run in the merge tier's PHPT gate (the `phpt-*` jobs), which runs every pinned PHPT on upstream and the all-modules hybrid. To attribute a gate regression to this module, see *Attributing a regression* in [corpus/upstream/README.md](../../../corpus/upstream/README.md).

## Not covered

Full PHP client/server runtime, real HPACK, callback/reentry/destructor
lifetimes, Zend bailout/longjmp, embedded versus external nghttp2, TLS/ALPN, h2c,
PHP thread modes and performance need separate gates.
