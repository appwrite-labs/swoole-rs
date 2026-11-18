# Rust bridge ABI v1

Cargo builds `swoole.so` through `tools/build_ext.py`. The extension links the
Rust module crates through `crates/ffi`. Its [build script](../crates/ext/build.rs)
compiles the native glue and selects bundled C and C++ sources from
`native/swoole-src/thirdparty` for the enabled features and PHP version.
Native test fixtures link the Zend-free `libswoole_rs_ffi.a`.

## Base contract (`native/include/swoole_rs.h`)

- `uint32_t swr_v1_abi_version(void)` returns 1.
- `uint64_t swr_v1_feature_bits(void)` returns the mask of compiled modules
  (values in [migration modules](migration-modules.md)).

Both are allocation-free, safe on any thread and after fork, and cannot panic
or throw. Module startup (crates/phpswoole) publishes them as the
`SWOOLE_RS_ABI_VERSION` and `SWOOLE_RS_FEATURE_BITS` constants and a phpinfo row.

## Rules for all module contracts

- No Zend values, Rust references, exceptions or panics cross the ABI. The [corebase header](../native/corebase/swoole.h) checks the layout of `swoole::Global`.
- Buffers are released by the side that allocated them. Borrowed pointers are
  valid only for the call. Writable ranges must not alias each other or Rust
  state unless a contract says otherwise.
- Errors are fixed-width integer codes. Lengths must fit the target's
  pointer-size limits (`isize::MAX` for slices).
- An owned handle needs an explicit release function, thread affinity, fork
  behavior and cancellation rules.
- Follow each module contract for callback timing and ownership. PHP-facing
  code uses the [Zend boundary](zend.md) for calls that may execute user code
  or trigger a bailout.

Target platforms are 64-bit Linux and macOS; both sides are compiled for the
same target.

## WebSocket kernel (feature value 1)

The WebSocket codec has no C ABI of its own any more: crates/websocket's safe codec
(`mask`, `encode`, `decode`, `pack_close_frame`) is called from Rust, through
crates/ports' `frame_api` where it takes upstream's records.

## Other module contracts

- Buffer queue (value 2): [buffer-abi-v1.md](buffer-abi-v1.md).
- Timer queue (value 4): [timer-abi-v1.md](timer-abi-v1.md).
- Base64, CRC32, error messages (values 8, 16, 32): [core-abi-v1.md](core-abi-v1.md).
- Other modules keep their headers and contracts under `native/<module>/`.
