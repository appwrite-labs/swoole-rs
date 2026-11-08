# CRC32

ABI: `docs/core-abi-v1.md`, `native/include/swoole_rs_crc32.h`. The overlay deletes `src/core/crc32.cc`; `crates/crc32` defines
`swoole_crc32` under its upstream symbol (feature `upstream-symbols`).

The Rust crate implements reflected IEEE CRC32 without allocation or mutable
global state. With `target_arch=aarch64` and `target_feature=crc` it uses the
IEEE `__crc32d/w/h/b` intrinsics (not CRC32C) on 8-byte chunks and 4/2/1-byte
tails; other targets use a compile-time slicing-by-eight table (8 KiB) with
explicit little-endian loads. The target contract selects the implementation;
there is no runtime CPU probe. Unlike the pin's mutable table (whose
`generated` flag is never set) there is no initialization race. The primitive
rejects address-space wrapping before building a slice; that is outside
upstream's defined domain, not a compatibility fix.

Derived from Swoole v6.2.4 (`c7e514509451beafd4d70ce5b78943d51e76381c`),
Apache-2.0; the native source keeps the original notice.

The pin's only production caller is `src/server/reactor_thread.cc`, which
hashes `Address::get_addr()` with the `sockaddr` length (28 bytes for IPv6) to
pick the worker for non-IPv4 datagrams. PHP `crc32()` uses PHP's own
implementation and does not exercise this code.

## Differential

```sh
cargo test -p swoole-rs-crc32
```

`crates/crc32/tests/upstream.rs` compares `swr_v1_crc32` with the pinned
`src/core/crc32.cc` (vendored unchanged in `crates/oracle/upstream/`, symbol
renamed) on the empty input, every one- and two-byte input, every length up to
64 and proptest input up to 4 KiB, each at seven unaligned offsets. The
original `TEST(hash, crc32)` from `core-tests/src/core/hash.cpp` is ported there
as `hash_crc32`, with the same input and expected value.

## Consumers

`consumer.py --extension EXTENSION --manifest IDENTITY --output REPORT` starts a
real Swoole UDP6 server in process mode (`consumer.php`) and checks that 56
binary messages from eight clients reach the independently computed CRC32
worker, and that eight empty datagrams get no callback (pinned recvfrom's
`ret <= 0` branch).

`crc32-consumers-*` runs it. No PHPT dispatches non-IPv4 datagrams across more
than one worker, so this is the only coverage of that path.
