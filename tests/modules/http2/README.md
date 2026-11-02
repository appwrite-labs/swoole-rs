# HTTP/2 policy conversion

Rust (`crates/http2`) owns the settings/frame codecs, role-specific SETTINGS
decisions, body chunking, window actions, frame header and control payload
integers, the client send queue container, and next-effect selection for
several server methods. `crates/phphttp2/src/server.rs` drives the server's
frames and responses over the C++ sessions, streams and contexts, which keep
the stores, HPACK, socket, compression, coroutine and destructor effects.
Rust also owns the process-wide default settings. Public headers are unchanged.
`Swoole\Coroutine\Http2\Client` is Rust (`crates/phphttp2`, #827); its PHPTs
cover it.
The overlay has no native fallback: patched builds define `SW_USE_RUST_HTTP2`.

## Running

From the repository root, run the core gate first. Every other gate reads its
report and reuses its overlay and archive. The overlay is the http2 files of the
archive's hybrid build tree (all modules' patches applied):

```sh
python3 tests/modules/http2/gate.py --source /absolute/pinned/swoole \
  --archive /absolute/hybrid/rust-prefix/lib/libswoole_rs_ffi.a --output target/http2/core.json --sanitize
python3 tests/modules/http2/header_calls.py --source /absolute/pinned/swoole \
  --core-report target/http2/core.json --output target/http2/header.json
```

`header_calls.py` takes the same `--source/--core-report/--output` arguments. Add
`--include /path/to/openssl/include` when the pinned headers need OpenSSL. Each
output's `-artifacts` directory must not exist yet. `tools/dev.py` registers all
of these as `http2-*` phases.

Native code is built with ASan and UBSan, excluding alignment because the pinned
inline codecs use unaligned integer casts. Rust archives are optimized and not
instrumented.

## Gates

`sides.py` builds the header gate as three translation units: the pinned
predicates against the pinned headers, the Rust lookups and the driver against
the hybrid build's tree (from the core report's archive).

- `gate.py`: compiles the original `src/protocol/http2.cc` against the pinned
  headers and the hybrid tree's copy and `core_calls.cpp` against the hybrid
  build's patched headers, runs Rust unit tests, and compares the core codec calls
  (`core_calls.cpp`) with a recording `Socket::send`.
- `header_calls.py`: Rust role/media header lookup against the pinned
  locale-sensitive predicates.

The server's frames, responses and request header parser are Rust too
(`crates/phphttp2/src/server.rs`, #757); its PHPTs cover them, so they have no
body-extraction gates.

## Original PHPTs

Upstream PHPTs for this module run in the merge tier's PHPT gate (the `phpt-*` jobs), which runs every pinned PHPT on upstream and the all-modules hybrid. To attribute a gate regression to this module, see *Attributing a regression* in [corpus/upstream/README.md](../../../corpus/upstream/README.md).

## Not covered

Full PHP client/server runtime, real HPACK, callback/reentry/destructor
lifetimes, Zend bailout/longjmp, embedded versus external nghttp2, TLS/ALPN, h2c,
PHP thread modes and performance need separate gates.
