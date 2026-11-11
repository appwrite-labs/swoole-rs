# Base64 tests

ABI: [`docs/core-abi-v1.md`](../../../docs/core-abi-v1.md). Upstream pin:
`76c657d88be39b7af6bf85e46bcbac0da7387760` (Swoole v6.2.5).

`crates/base64` replaces both low-level algorithms with allocation-free Rust.
The overlay deletes `src/core/base64.cc` and `include/swoole_base64.h`: no
compiled C++ calls `swoole::base64_encode` or `base64_decode` any more.

The decoder intentionally accepts incomplete quartets, stops at the first `=`,
and preserves provisional writes before invalid data returns zero. It does not
skip whitespace. Normal completion writes a terminating NUL. Untouched bytes stay
untouched, including on failure. Both take slices, and writes that would fall
past the output slice are skipped; the crate forbids `unsafe_code`.

**Zero-length encode is outside the compared domain** and is excluded from the
C++ oracle. The Rust definition writes a NUL and returns zero.

The encoder processes full three-byte groups without padding decisions; one tail
step supplies padding. The decoder classifies each byte with an immutable
256-byte table generated from the canonical alphabet.

## Differential tests

```sh
cargo test -p swoole-rs-base64
```

`crates/base64/tests/upstream.rs` compares `encode` and `decode` with the pinned
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

Upstream PHPTs for this module run in the merge tier's PHPT gate (the `phpt-*` jobs), which runs every pinned PHPT on upstream and the Cargo build. To attribute a gate regression to this module, see *Attributing a regression* in [corpus/upstream/README.md](../../../corpus/upstream/README.md).

The `HttpProxy::get_auth_str` path these phases check has no PHPT, Rust or fuzz
coverage.
