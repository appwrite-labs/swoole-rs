# Core primitives ABI v1: Base64, CRC32, error messages

Reference: Swoole v6.2.6, `13d2f33664d60b72aaa9f102ff0ef4085c028513`. Feature
values: base64 8, CRC32 16, error messages 32.

The primitives this page describes (Base64, CRC32 and the error-message
lookup) allocate nothing, retain no pointer, invoke no callback, have no
mutable global state and cannot panic or throw. They are safe on independent
inputs from any thread and after fork. The error exports with a formatter
callback, a per-thread buffer or the ignored-error set keep their own rules in
[the error contract](../native/error/contract.md).

## M09 Base64 (crate `swoole-rs-base64`)

No C ABI: every caller is Rust and uses the safe `encode` and `decode` over
slices. The crate forbids `unsafe_code`.

- **Encode** writes `4 * ceil(length / 3)` characters with the upstream alphabet
  and padding, then a NUL, and returns the count excluding the NUL.
- **Decode** keeps the pinned byte classifier, stops at the first `=`, accepts
  incomplete groups, writes a NUL on normal completion and returns 0 on invalid
  data while leaving the partial writes upstream makes.

## M10 CRC32 (crate `swoole-rs-crc32`)

IEEE CRC32: reflected polynomial `0xedb88320`, initial value and final XOR
`0xffffffff`; the safe `crc32(&[u8])`, in a crate that forbids `unsafe_code`.
The table is a compile-time constant (upstream lazily fills a mutable table,
which races).

## M11 error messages (`native/include/swoole_rs_error.h`, crate `swoole-rs-error`)

For each case in pinned `src/core/error.cc`, returns the exact NUL-terminated
message for its `swoole_error.h` value. The pointer is immutable static storage.
Unknown codes and system errno values return null.

`swr_v1_error_resolve`, `swr_v1_error_strerror`, the per-thread unknown-code
buffer and the ignored-error set are in [the error contract](../native/error/contract.md).
