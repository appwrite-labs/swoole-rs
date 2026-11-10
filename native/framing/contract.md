# Packet framing and MIME (M18)

`src/protocol/base.cc`, `src/protocol/mime_type.cc` and `include/swoole_protocol.h`
have left the build. `swoole_protocol.h` here keeps the SendData, RecvData,
PacketLength and Protocol layouts and the functions C++ or the compiled core-tests call.
`crates/framing` forbids `unsafe_code` and exports nothing to C; the hybrid reaches
it through crates/ports' upstream symbols below. Feature bit 4096; graduated.

## Packet framing

- `swoole_rs_framing::recv::recv` runs one whole length, EOF or split-by-EOF
  receive. The caller holds the `swoole::String` and the `Protocol`: the loop reads
  the String head (`length`, `size`, `offset`) and its bytes, sets `length` and
  `offset`, and reads the `Protocol`'s settings through `Effects`, again after every
  effect. Each effect (receive and read-error handling, length and package callbacks,
  extend/reduce/clear/split, `SSL_pending`, logs) goes through its generic
  `Effects::effect`, which refreshes the socket's removed/recv_wait/ssl flags after it.
- `crates/ports/src/framing_recv.rs` is those effects over the real socket, String and
  callbacks. It defines `default_length_func` under its upstream symbol (the hybrid
  archive's `upstream-symbols`), and `swoole_unpack`, `swoole_hton64`, `swoole_ntoh64`,
  `Protocol::recv_with_length_protocol` and `recv_with_eof_protocol` as plain Rust: `swoole_init` registers them for
  crates/clients (`FRAMING`), which ports depends on. Every protocol callback in the hybrid
  is Rust or PHP, so no C++ exception reaches it.
- `swr_v1_framing_recv` (`crates/oracle/shim/swoole_rs_framing.h`) is `recv` for the
  oracle's native candidate from `0001-base.patch`, which the `framing_stream` fuzz
  target runs; `fuzz/src/framing_native.rs` defines it. One `call(context, op, arg)`
  per effect, and `THROWN` stops all further effects.
- Kept from upstream: callback removal/failure order, recursive entry, captured
  dynamic header size, partial packets, sticky EOF retry and always growing to the
  next page.
- `swoole_rs_framing::unpack` handles all sixteen formats, native 32-bit int and
  64-bit size_t, unaligned loads, signed values and endianness. `default_length`
  keeps the original `header_len` assignment, including the dynamic-size quirk.
  Invalid-format diagnostics are logged after the decode.

## MIME

- The table (`crates/framing/src/mime.rs`) starts as the pinned 362-entry
  catalog and is the one process-wide store PHP's `swoole_mime_type_*` functions
  change. A `get` result stays valid until that suffix is set again or deleted,
  as an `unordered_map` value does. `list` yields the types in suffix order
  (upstream's is hash order).
- Lookup is case-sensitive on mutation, uses the last dot, and handles no-dot,
  trailing-dot and embedded-NUL input. Case mapping is ASCII, as C `tolower` is in
  the "C" locale PHP starts in.

## Deliberate differences from upstream

These differ only where upstream has undefined or unsafe behavior:

- A format wider than the reported packet bytes is rejected instead of read past
  the end. A declared dynamic size smaller than its format can trigger this.
- An overflowing signed total length is rejected (signed overflow is UB natively).
- A receive request larger than the buffer's remaining allocation is rejected.
- MIME suffixes lowercase only ASCII letters. Upstream's `tolower` also maps
  bytes 0x80-0xff after a script's `setlocale(LC_CTYPE, ...)` to a single-byte
  locale, and is UB for negative signed `char` values other than EOF. PHP 8.2's
  `strtolower` made the same change.

Tests: `tests/modules/framing/`.
