# CRC32

The CRC32 crate computes reflected IEEE CRC32 with a compile-time slicing-by-eight
table and explicit little-endian loads. The Rust server calls `crc32` directly
from `DgramPacket::sender` in `crates/server/src/reactor_entry.rs`. For non-IPv4
datagrams it hashes the sender's host text using the socket address length
(28 bytes for IPv6) to select the worker.

The C++ hash-originals fixture imports the `swoole_crc32` wrapper from corebase.
See the [utilities contract](../../../native/utilities/contract.md).

## Differential

```sh
cargo test -p swoole-rs-crc32
```

`crates/crc32/tests/upstream.rs` compares `crc32` with the pinned
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
