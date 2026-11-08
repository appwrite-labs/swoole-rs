# Core primitives ABI v1: Base64, CRC32, error messages

Reference: Swoole v6.2.4, `c7e514509451beafd4d70ce5b78943d51e76381c`. Feature
values: base64 8, CRC32 16, error messages 32.

The primitive exports allocate nothing, retain no pointer, invoke no callback,
have no mutable global state and cannot panic or throw. They are safe on
independent inputs from any thread and after fork. Byte alignment is 1. Lengths
and output sizes must fit `isize::MAX`; invalid pointer/length combinations make
base64 and CRC32 return 0 without dereferencing. Pointer validity and buffer
extents are caller preconditions.

## M09 Base64 (`native/include/swoole_rs_base64.h`, crate `swoole-rs-base64`)

The header is generated from the crate by cbindgen (`crates/base64/cbindgen.toml`);
run `tools/headers.sh` after changing an export. The PR tier fails if it is stale.

Signatures mirror the pinned low-level functions. Null input is allowed only for
length 0; output must be non-null.

- **Encode** needs `4 * ceil(length / 3) + 1` output bytes and returns the
  encoded length excluding the trailing NUL, with the upstream alphabet and
  padding. Input and output may overlap: the encoder keeps upstream's exact
  read/store order, including rereads after earlier stores. Disjoint input uses
  a faster path (48-byte NEON blocks on little-endian AArch64). Empty input
  writes a NUL and returns 0; upstream underflows and reads out of bounds there.
  See `native/base64/overlap-contract.md`.
- **Decode** needs `length + 1` output bytes and disjoint buffers. Keeps the
  pinned byte classifier, stops at the first `=`, accepts incomplete groups,
  writes a NUL on normal completion and returns 0 on invalid data while leaving
  the partial writes upstream makes. No pinned production code calls the decoder.

## M10 CRC32 (`native/include/swoole_rs_crc32.h`, crate `swoole-rs-crc32`)

IEEE CRC32: reflected polynomial `0xedb88320`, initial value and final XOR
`0xffffffff`. Empty input may be null and returns 0. The table is a compile-time
constant (upstream lazily fills a mutable table, which races). The overlay
deletes `src/core/crc32.cc` and defines the `swoole_crc32` symbol in Rust.

## M11 error messages (`native/include/swoole_rs_error.h`, crate `swoole-rs-error`)

For each case in pinned `src/core/error.cc`, returns the exact NUL-terminated
message for its `swoole_error.h` value. The pointer is immutable static storage.
Unknown codes and system errno values return null.

Additional module-local exports (`native/error/swoole_rs_error_state.h`,
`native/error/swoole_rs_error_resolver.h`, [contract](../native/error/contract.md)):

- The resolver classifies codes and formats unknown ones; it may call only the
  synchronous, non-throwing native system formatter and retains nothing. Native
  exceptions are raised after it returns.
- Unknown-code formatting uses one mutable 32-byte buffer with upstream's pointer
  lifetime; callers serialize format-and-use. Known strings may be read
  concurrently.
- The ignored-error set is a Rust `HashSet` behind an opaque handle. Allocation
  failure leaves it unchanged and returns -2; native raises `bad_alloc`. It is
  copied on fork; writes need external exclusion, reads may be concurrent; each
  process destroys its own handle.
