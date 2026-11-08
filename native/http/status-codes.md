# Public status-code array

The native initializer for `swoole::http_server::list_of_status_code` has been
removed. A Rust `static mut [i32; 128]` now defines that C++ data symbol, keeping
every initialized entry, the sentinel and the trailing zero slots. The header's
`extern int[128]` declaration is unchanged.

- Symbol: `_ZN6swoole11http_server19list_of_status_codeE` (Itanium ABI; Mach-O adds
  its prefix), set with `export_name` and kept with `used`.
- Native compilation rejects other C++ ABI families and any int32 size/alignment
  mismatch. Only the tested macOS and Linux targets are known to work.
- Rust never creates a reference to the array. External C++ users keep the original
  unsynchronized sequential read/write contract. There is no accessor or mirror, and
  status lookup does not depend on the array's contents.

Tests: `crates/http/src/pin_helpers.rs`
(`public_status_code_array_matches_the_pin_and_is_independent_of_lookup`). Export visibility in the production PHP
extension still needs to be checked.
