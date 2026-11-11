# PHP HTTP binding policy

Feature bit 2097152 (`httpbindings`).
crates/httpbindings is policy over borrowed slices, with no `unsafe`
(`forbid(unsafe_code)`): each call returns a decision and retains no
context, buffer, Zend value or pointer. crates/phphttp (the PHP HTTP classes,
whose context it allocates in `src/state.rs`) performs every effect (Zend
mutation, allocation, encoding, date/time, warnings, callbacks, IO, codecs)
after the call returns and re-observes fields afterwards. Cursors are plain
caller-local values.

## Request headers and Response header keys

Request header classification returns append/cookie/upgrade/form/multipart/
compression/chunked decisions; crates/phphttp publishes the websocket flag before
a failed session lookup, as upstream. Known-key indices 0..7 = host, user-agent, accept, content-type,
content-length, authorization, connection, accept-encoding; 8 = unknown (merge).
Pinned llhttp method numbers 0/3/4/28 are compile-asserted at the caller.

Token-list helper (`token_list_fast`, then `token_list_contains`):

- The fast path is the native bounded comparison using `sizeof(pointer)-1`, not
  the value's `strlen`; on 64-bit, `websock` matches `websocket` through it.
- Only when it is undecided does crates/phphttp check the value fits the native
  scratch buffer, then scan: comma-separated tokens before the first NUL, each
  trimmed of libc `isspace` and compared with libc case semantics.

Quote trimming keeps the original shrinking bound: three opening quotes then `a`
trim to one quote then `a`. Response header-key case formatting keeps the
leading/consecutive-hyphen state machine, bounded binary/NUL behavior, and treats
negative lengths as a no-op.

## Payload options, compression and multipart

- Request options: native iteration, truth/long/string conversion and assignment
  keep their order (including conversion reentry). Keys use libc case comparison.
  Capability bits exclude branches compiled out of the original.
- Compression preference keeps case-sensitive substring matching (including
  embedded NUL, matches inside tokens and q=0); it is not an RFC parser. The
  caller truncates `size_t` to `uint32_t` where `swoole_strnpos` did. Methods
  0..4 are checked against the pinned enum. No match clears
  `accept_compression` but keeps the old method. Encoding labels are
  NUL-terminated Rust statics exposed through the unchanged
  `get_content_encoding`.
- Multipart leading CR/LF is skipped only at the original first-body point,
  after native append and before the native parser call. Completion prefers the
  chunked buffer over ordinary body data, even when the chunked buffer is empty.

## Response default headers and codecs

The Response known-header classifier keeps the pinned flag values and libc case
semantics (static-asserted at the caller): it maps each known header name to its
original native flag, and unknown headers keep the custom path. Header emission
is crates/phphttp (`src/header.rs`).

Codec selection happens only after the original acceptance/minimum-length checks.
Declined compression stays a silent false; unknown/unavailable methods take the
native warning path. Zlib negative/zero/maximum level rules and Brotli's field
clamp (min first) are Rust scalar policy. Zstd keeps its captured local level,
native max-then-min query order and local clamp without writing the Context field.
Codec implementations, allocation, warnings and buffer formulas stay native.

