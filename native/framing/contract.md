# Packet framing and MIME (M18)

The overlay replaces `src/protocol/base.cc` and `src/protocol/mime_type.cc`. The public
Protocol, PacketLength, Socket, String and MIME declarations are unchanged.
`swoole_rs_framing.h` declares ten exports. Feature bit 4096 is the optional
framing feature; the module is opt-in.

## Packet framing

- `swr_v1_framing_recv` runs one whole length, EOF or split-by-EOF receive. It
  reads and writes the `swoole::String` head (`length`, `size`, `offset`, `str`) and
  reads the `Protocol`'s leading fields in place; native `static_assert`s pin both
  layouts. Native code performs each effect (receive and read-error handling,
  length and package callbacks, extend/reduce/clear/split, `SSL_pending`, logs)
  through one `call(context, op, arg)` and refreshes the socket's
  removed/recv_wait/ssl flags after it.
- A native exception (for example String allocation failure) is caught in the
  call, Rust returns at once, and native code rethrows it; nothing unwinds
  through a Rust frame.
- Kept from upstream: callback removal/failure order, recursive entry, captured
  dynamic header size, partial packets, sticky EOF retry and always growing to the
  next page.
- `swr_v1_framing_unpack` handles all sixteen formats, native 32-bit int and
  64-bit size_t, unaligned loads, signed values and endianness. The default
  length helper keeps the original `header_len` assignment, including the
  dynamic-size quirk. Invalid-format diagnostics run natively after Rust returns.
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
- C++'s `StaticHandler` calls `swr_v1_framing_mime_get` and
  `swr_v1_framing_mime_exists` (crates/server's Rust one calls `mime::get` and
  `mime::exists`); an invalid extent looks up the empty suffix.

## Deliberate differences from upstream

These differ only where upstream has undefined or unsafe behavior:

- A format wider than the reported packet bytes is rejected instead of read past
  the end. A declared dynamic size smaller than its format can trigger this.
- An overflowing signed total length is rejected (signed overflow is UB natively).
- A receive request larger than the buffer's remaining allocation is rejected.
- For C `tolower`, signed-char 0xff keeps EOF behavior; other negative signed-char
  values are converted to unsigned (the original call is UB for them).

Tests: `tests/modules/framing/`.
