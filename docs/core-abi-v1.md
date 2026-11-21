# Core primitives ABI v1: Base64, CRC32, error messages

Reference: Swoole v6.2.5, `76c657d88be39b7af6bf85e46bcbac0da7387760`. Feature
values: base64 8, CRC32 16, error messages 32.

The primitives allocate nothing, retain no pointer, invoke no callback,
have no mutable global state and cannot panic or throw. They are safe on
independent inputs from any thread and after fork.

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
which races). `crates/utilities` defines `swoole_crc32` under its upstream symbol for
C++ callers; null or empty input returns 0.

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
