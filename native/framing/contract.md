# Packet framing and MIME (M18)

The native header defines the SendData, RecvData, PacketLength and Protocol records.
The coroutine socket embeds Protocol and checks its size and offsets at compile time.
The two length callbacks and the package callback are fields of that record.
[Record definitions](swoole_protocol.h) · [Coroutine socket layout](../hooks/swoole_rs_co_socket.h)

## Packet framing

- `swoole_rs_framing::recv::recv` runs one whole length, EOF or split-by-EOF
  receive. The caller holds the `swoole::String` and the `Protocol`: the loop reads
  the String head (`length`, `size`, `offset`) and its bytes, sets `length` and
  `offset`, and reads the `Protocol`'s settings through `Effects`, again after every
  effect. Each effect (receive and read-error handling, length and package callbacks,
  extend/reduce/clear/split, `SSL_pending`, logs) goes through its generic
  `Effects::effect`, which refreshes the socket's removed/recv_wait/ssl flags after it.
- The production adapter supplies the socket, buffer and callbacks to the Rust receive
  loop. Client and port reads call the same adapter with their own protocol settings.
  [Socket adapter](../../crates/clients/src/framing_recv.rs)
- The framing fuzz target exposes `swr_v1_framing_recv` to the oracle’s C++ candidate.
  Its callback reports each effect, and `THROWN` stops further effects.
  [Oracle header](../../crates/oracle/shim/swoole_rs_framing.h) · [Candidate caller](../../crates/oracle/candidate/src/protocol/base.cc) · [Fuzz adapter](../../fuzz/src/framing_native.rs)
- Kept from upstream: callback removal/failure order, recursive entry, captured
  dynamic header size, partial packets, sticky EOF retry and always growing to the
  next page.
- `swoole_rs_framing::unpack` handles all sixteen formats, native 32-bit int and
  64-bit size_t, unaligned loads, signed values and endianness. `default_length`
  keeps the original `header_len` assignment, including the dynamic-size quirk.
  Invalid-format diagnostics are logged after the decode.

## MIME

- The PHP MIME functions share a process-wide table initialized from the pinned
  catalog. `get` copies the MIME type into an owned byte vector, so a later update
  or deletion leaves the result valid. Unknown suffixes return
  `application/octet-stream`. Enumeration visits the types in suffix order.
  [MIME table](../../crates/framing/src/mime.rs)
- Lookup is case-sensitive on mutation, uses the last dot, and handles no-dot,
  trailing-dot and embedded-NUL input. Case mapping is ASCII, as C `tolower` is in
  the "C" locale PHP starts in.

## Deliberate differences from upstream

Rust's own checks:

- A format wider than the reported packet bytes is rejected. A declared dynamic
  size smaller than its format can trigger this.
- An overflowing signed total length is rejected.
- A receive request larger than the buffer's remaining allocation is rejected.
- MIME suffixes lowercase only ASCII letters. Upstream's `tolower` also maps
  bytes 0x80-0xff after a script's `setlocale(LC_CTYPE, ...)` to a single-byte
  locale. PHP 8.2's `strtolower` made the same change.

Tests: `tests/modules/framing/`.
