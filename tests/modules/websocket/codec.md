# WebSocket codec and dispatch policy seam

The module-local ABI v1 extension is
`native/websocket/swoole_rs_websocket_codec.h`. The original three kernel exports
are unchanged. The opt-in WebSocket overlay replaces native frame encoding,
metadata extraction, decoding, close packing and dispatch policy, plus PHP-facing
outgoing compression/control normalization and incoming control
visibility/automatic-pong selection. Native code still owns String allocations,
fragment storage, connection status, callbacks, socket I/O, handshake, PHP
objects and compression.

The exports allocate nothing, retain no pointers and invoke no callbacks. No
Rust reference, C++ layout, Zend value or exception crosses the seam. All size
fields are native `size_t`; supported hosts remain 64-bit macOS and Linux.
Native allocation/exceptions complete before a Rust output borrow begins.

- `encoded_size(length, flags)` returns actual storage bytes or zero on overflow.
  Header-only output advertises the full length but omits its payload. Masked
  header-only output includes the fixed upstream `258E` key; the native adapter
  then returns false exactly as before, retaining those appended bytes.
- `encode` writes into caller-owned storage and returns bytes written. Invalid
  input/overflow/insufficient capacity returns zero without writes. Input and
  output are disjoint. Null input is allowed for empty/header-only encoding.
  Header opcode truncation and FIN/RSV1/MASK mapping preserve the existing kernel.
- `control_invalid` is upstream's encoder rule since v6.2.4: close, ping and
  pong (opcodes 8-10) must be final and at most 125 bytes. `encode` rejects such
  frames before writing; the native caller logs the warning.
- `encode_close` packs the signed native code modulo 65536, forces FIN and uses
  the same encoder, so reasons over 123 bytes are rejected.
- `decode` accepts an exclusive mutable buffer and disjoint aligned output.
  It returns the expected positive total length, zero for incomplete header,
  minus one for invalid pointers/signed-size overflow, or
  `SWR_WS_INVALID_CONTROL` (-2) for a complete compressed (RSV1) close, ping or
  pong frame, whose payload stays masked as upstream rejects it first. An incomplete body
  returns its expected length without changing input. Only a complete nonempty
  masked payload is XORed; trailing bytes are untouched. Metadata is initialized
  on valid arguments, including incomplete frames. Zero length permits null.
  Header availability preserves the native decoder's uint32 conversion. The
  copied key is meaningful only for complete nonempty masked frames.
- `pack_policy` returns normalized outgoing flags in its low byte and requests
  native zlib compression in bit 8. Control opcodes force FIN and clear
  COMPRESS, preserving a supplied RSV1: the pinned unparenthesized
  `sw_unset_bit(flags, FLAG_COMPRESS | FLAG_RSV1)` macro clears only COMPRESS. Continuations and unfinished messages retain flags but never
  compress. Empty payloads never compress. No-zlib builds retain their original
  native warning/failure path before this call.
- `message_policy` uses close/ping/pong enable bits (1/2/4) to choose PHP delivery,
  discard, or automatic pong plus discard. The native layer executes the reply
  and destroys the PHP payload after the call, outside Rust.
- `decide` consumes a complete decoded-frame value plus a borrowed snapshot of
  native fragment/closing state. It emits an action, error, byte offset/length
  and dispatch flags. It retains no state. Invalid ranges return minus one
  without modifying output. Native code performs the resulting append/dispatch,
  cleanup, close echo and status changes; no callback runs inside Rust.

Policy intentionally retains orphan/overlap rejection, initial fragment flags,
aggregate size boundaries, permissive fragmented control frames, FIN messages
arriving during active fragmentation, unknown-opcode warning/success behavior,
wire-size (not payload-size) ping/pong and close limits, and the native control
payload offset even for non-minimal extended-length headers. Fragment length
addition is checked: impossible overflowing totals are rejected rather than
wrapped. Existing signed total-length rejection remains unchanged.

Run the real ABI differential against unchanged pinned C++ codec bodies:

```sh
cargo test -p swoole-rs-websocket
cargo build --release -p swoole-rs-ffi
python3 -m pytest tests/modules/websocket/test_codec.py \
  --archive target/release/libswoole_rs_ffi.a --sanitize   # reads target/upstream/src
```

The oracle takes exact function bodies and frame/flag definitions from pinned
Git objects, with a vector String adapter and logging disabled. PHP policy
branches are compiled from exact pinned source snippets and compared exhaustively
across all 256 opcode bytes, 256 flag/settings bytes and four payload lengths
(327,680 cases). The bit-operation macros are read verbatim from pinned
`swoole.h`; Zend and zlib are not called. The constructed dispatch-policy cases
are separate from the differential. Connection and fragment lifecycle are covered
by the PHP and network gates in [README.md](README.md). C++ ASan/UBSan excludes
alignment because the original codec has unaligned integer loads.
