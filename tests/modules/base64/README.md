# M09 base64 conversion

ABI: [`docs/core-abi-v1.md`](../../../docs/core-abi-v1.md) and
`native/include/swoole_rs_base64.h`. Upstream pin:
`c7e514509451beafd4d70ce5b78943d51e76381c` (Swoole v6.2.4).

`crates/base64` replaces both low-level algorithms with allocation-free Rust.
The overlay deletes `src/core/base64.cc` and `include/swoole_base64.h`: no
compiled C++ calls `swoole::base64_encode` or `base64_decode` any more.

The decoder intentionally accepts incomplete quartets, stops at the first `=`,
and preserves provisional writes before invalid data returns zero. It does not
skip whitespace. Normal completion writes a terminating NUL. Untouched bytes stay
untouched, including on failure. Rust uses `MaybeUninit<u8>` output storage, so C
callers may pass uninitialized writable storage. Only the FFI boundary constructs
slices; checked size/address arithmetic rejects null and unrepresentable ranges.
Allocation validity and disjointness remain caller preconditions.

**Zero-length encode is outside the compared domain** and is excluded from the
C++ oracle. The Rust definition writes a NUL and returns zero; null input
is permitted for zero length.

The encoder processes full three-byte groups without padding decisions; one tail
step supplies padding. The decoder classifies each byte with an immutable
256-byte table generated from the canonical alphabet.

## Caller audit

Production low-level encoder calls in the pinned source:

| Caller | Input and output ownership |
| --- | --- |
| `src/protocol/http.cc`, `HttpProxy::get_auth_str` | Native stack buffers, formatted `username:password` |
| `src/core/string.cc`, `String::append_random_bytes` | Native random input and separately allocated encoded output; zero requested length is not guarded here |
| `ext-src/swoole_http_client_coro.cc`, `Client::set_basic_auth` | Native `std::string` `username:password`, separate Zend allocation |
| `ext-src/swoole_websocket_server.cc` | 20-byte SHA1 digest, separate native handshake output |

No production code calls the low-level `swoole::base64_decode`; only its original
core test does. The `swoole_stdext.cc` aliases, the embedded PHP library and the
client handshake's `php_base64_encode` use PHP's own base64, not this code.

## Differential tests

```sh
cargo test -p swoole-rs-base64
```

`crates/base64/tests/upstream.rs` compares the Rust exports with the pinned
upstream C++, which `crates/oracle` compiles from unmodified copies under its
`upstream/` directory. Return counts and every output byte must match, with three
sentinels, 17-byte red zones and unaligned input: all one- and two-byte inputs,
all strings through length five over eleven byte classes, every byte in every
position through two quartets, and proptest cases for binary encode, base64-like
decode and truncated valid encodings.

The two original core tests (`core-tests/src/protocol/base64.cpp`) are ported
to Rust in the same file, one test each, with the same inputs and expected
values:

| Upstream TEST | Rust test |
| --- | --- |
| `base64.encode` | `base64_encode` (a deterministic fixture replaces `swoole_random_bytes`) |
| `base64.decode` | `base64_decode` |

Rust's own bounds checks replace the previous ASan/UBSan build. The Rust codec has
no mutable state, so the earlier concurrency and fork runs are not carried over.

## PHP consumers

`consumers.py --extension EXTENSION --manifest BUILD_MANIFEST --output REPORT`
(registered in `tools/dev.py` as `base64-consumers-*`) checks the real coroutine
HTTP client's `setBasicAuth` output against Python Base64 for 13 credential
pairs (empty, binary/NUL, padding boundaries, up to 8 KiB), 12 HttpProxy
authorization cases, and 32 WebSocket server handshake responses against Python
SHA1/Base64, all on loopback ephemeral ports. The extension's SHA-256 must match
the manifest. Linux builds with native curl hooks need `--php-extension curl`.
The proxy path keeps its native `%.*s` embedded-NUL truncation and 255-byte
bound; the checks expect that behavior.

Upstream PHPTs for this module run in the merge tier's PHPT gate (the `phpt-*` jobs), which runs every pinned PHPT on upstream and the all-modules hybrid. To attribute a gate regression to this module, see *Attributing a regression* in [corpus/upstream/README.md](../../../corpus/upstream/README.md).

Gate audit (#471/#472): `base64-consumers-*` is **kept**. The codec itself is
covered by the `cargo test -p swoole-rs-base64` differential, the `base64_encode`
/`base64_decode` fuzz targets and the two ported core tests above;
`Client::set_basic_auth` by `swoole_http_client_coro/set_basic_auth.phpt` and the
handshake accept by the `swoole_websocket_server` PHPTs. But the `HttpProxy::
get_auth_str` proxy-auth path (its `%.*s` embedded-NUL truncation and 255-byte
bound) has no PHPT, Rust or fuzz coverage, and all three are C++-caller
integration checks that cannot move to a Rust unit test — so the phase stays.

## Other probes

## Open gates

Linux runs of the positive-only and preparation profiles, native x86 C++/PHP
integration, MemorySanitizer/Miri, matched optimized performance and default
activation remain open.
