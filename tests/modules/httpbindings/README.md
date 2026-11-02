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

Each `*_gate.py` extracts the actual pinned and patched function bodies from
`ext-src/swoole_http_*.cc`, compiles both against shared native effect facades
(`*_oracle.h`) and the Rust archive, and compares outputs, effect order, partial
state and fault exits. Native code always runs under ASan/UBSan.

```sh
rustc --edition 2024 --crate-type staticlib -C opt-level=2 \
  crates/httpbindings/src/lib.rs -o /private/target/libhttpbindings.a
python3 tests/modules/httpbindings/settings_gate.py --source /readonly/pinned/git \
  --archive /private/target/libhttpbindings.a --output /private/target/settings.json
```

All gates take `--source`, `--output` and either `--archive PATH` or
`--rust-sanitize` (Linux: build an ASan Rust archive privately; std stays
uninstrumented). `server_gate.py` needs `--archive` from a hybrid build, as does `emission_gate.py`
(with `--rust-sanitize` it links a private ASan archive instead): their candidate side,
including the `swoole.h`/`swoole_util.h` helpers they inline, comes from that
build's fully patched tree, the upstream side from the pin.

| Gate | Checks | Extra flags |
|---|---|---|
| `server_gate.py` | coroutine handler routing, whole accept loop | |
| `settings_gate.py` | coroutine server startup settings body: alias order, null/false fallthrough, level clamp | `--without-compression` |
| `emission_gate.py` | `HttpContext::write`/`end` chunk/trailer packetization and split sends | `--without-compression`, `--without-zlib` |
| `file_gate.py` | `send_file` packetization and signed range validation | `--without-compression` |

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
