# Packet framing and MIME (M18)

`src/protocol/base.cc`, `src/protocol/mime_type.cc` and `include/swoole_protocol.h`
have left the build. `swoole_protocol.h` here keeps the SendData, RecvData,
PacketLength and Protocol layouts and the functions C++ or the compiled core-tests call.
Exports are declared in `swoole_rs_framing.h`. Feature bit 4096; graduated.

## Packet framing

- `swoole_rs_framing::recv::recv` runs one whole length, EOF or split-by-EOF
  receive. It reads and writes the `swoole::String` head (`length`, `size`,
  `offset`, `str`) and reads the `Protocol`'s leading fields in place. Each effect
  (receive and read-error handling, length and package callbacks,
  extend/reduce/clear/split, `SSL_pending`, logs) goes through its generic
  `Effects::effect`, which refreshes the socket's removed/recv_wait/ssl flags after it.
- `crates/ports/src/framing_recv.rs` is those effects over the real socket, String and
  callbacks. It defines `default_length_func`, `swoole_unpack`, `swoole_hton64` and
  `swoole_ntoh64` under their upstream symbols (the hybrid archive's
  `upstream-symbols`), and `Protocol::recv_with_length_protocol` and
  `recv_with_eof_protocol` as plain Rust: `swoole_init` registers them for
  crates/clients (`FRAMING`), which ports depends on. Every protocol callback in the hybrid
  is Rust or PHP, so no C++ exception reaches it.
- `swr_v1_framing_recv` is `recv` for the oracle's native candidate from
  `0001-base.patch`: one `call(context, op, arg)` per effect, and `THROWN` stops
  all further effects.
- Kept from upstream: callback removal/failure order, recursive entry, captured
  dynamic header size, partial packets, sticky EOF retry and always growing to the
  next page.
- `swr_v1_framing_unpack` handles all sixteen formats, native 32-bit int and
  64-bit size_t, unaligned loads, signed values and endianness. The default
  length helper keeps the original `header_len` assignment, including the
  dynamic-size quirk. Invalid-format diagnostics are logged after the decode.
- Borrowed pointers must cover the given length; null is accepted for length 0.
  Output pointers must be live, aligned and disjoint from the input and from each
  other. State and snapshot pointers must not alias mutable state.

## MIME

- The table (`crates/framing/src/mime.rs`) starts as the pinned 362-entry
  catalog and is the one process-wide store PHP's `swoole_mime_type_*` functions
  change. A `get` result stays valid until that suffix is set again or deleted,
  as an `unordered_map` value does. `list` yields the types in suffix order
  (upstream's is hash order).
- Lookup is case-sensitive on mutation, uses the last dot, and handles no-dot,
  trailing-dot and embedded-NUL input. Case mapping uses host libc `tolower`.
- `swr_v1_framing_mime_get` and `swr_v1_framing_mime_exists` look up an invalid
  extent as the empty suffix.

## Deliberate differences from upstream

These differ only where upstream has undefined or unsafe behavior:

- A format wider than the reported packet bytes is rejected instead of read past
  the end. A declared dynamic size smaller than its format can trigger this.
- An overflowing signed total length is rejected (signed overflow is UB natively).
- A receive request larger than the buffer's remaining allocation is rejected.
- For C `tolower`, signed-char 0xff keeps EOF behavior; other negative signed-char
  values are converted to unsigned (the original call is UB for them).

Tests: `tests/modules/framing/`.
