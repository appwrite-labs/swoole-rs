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

Request header classification (`request_header`) returns append/cookie/upgrade/
form/multipart/compression decisions; crates/phphttp publishes the websocket flag
before a failed session lookup, as upstream. Known-key indices 0..7 = host,
user-agent, accept, content-type, content-length, authorization, connection,
accept-encoding; `None` = unknown (merge). Form bodies follow llhttp's method
numbers for DELETE, POST, PUT, PATCH and QUERY, which a crates/phphttp test
checks against crates/http's llhttp method names.

Token-list helper (`token_list_fast`, then `token_list_contains`):

- The fast path is the native bounded comparison using `sizeof(pointer)-1`, not
  the value's `strlen`; on 64-bit, `websock` matches `websocket` through it.
- Only when it is undecided does crates/phphttp check the value fits the native
  scratch buffer, then scan: comma-separated tokens before the first NUL, each
  trimmed of libc `isspace` and compared with libc case semantics.

Quote trimming keeps the original shrinking bound: three opening quotes then `a`
trim to one quote then `a`. Response header-key case formatting (`canonical_case`)
keeps the leading/consecutive-hyphen state machine and changes only ASCII letters.

## Payload options, compression and multipart

- Request options: native iteration, truth/long/string conversion and assignment
  keep their order (including conversion reentry). Keys use libc case comparison.
  Capability bits exclude branches compiled out of the original.
- Compression preference keeps case-sensitive substring matching (including
  embedded NUL, matches inside tokens and q=0); it is not an RFC parser. The
  caller truncates `size_t` to `uint32_t` where `swoole_strnpos` did. Methods
  0..4 are the pinned `HTTP_COMPRESS_*` values. No match clears
  `accept_compression` but keeps the old method. `encoding` gives each method's
  NUL-terminated label, as `get_content_encoding` did.
- Multipart leading CR/LF is skipped only at the original first-body point,
  after the body is appended and before the multipart parser runs. Completion
  prefers the chunked buffer over ordinary body data, even when the chunked
  buffer is empty.

## Response default headers and codecs

The Response known-header classifier (`response_header`) keeps libc case
semantics and maps each known header name to its pinned `swHttpHeaderFlag`
value, the constants crates/phphttp's header builder (`src/header.rs`) tests
the flags with; unknown headers keep the custom path.

Codec selection happens only after the original acceptance/minimum-length checks.
Declined compression stays a silent false; unknown/unavailable methods take the
warning path. Zlib negative/zero/maximum level rules and Brotli's field clamp
(min first) are scalar policy here. Zstd keeps its captured local level, upstream's
max-then-min query order and local clamp without writing the Context field.
The codecs (crates/deflate, crates/inflate), their buffers and warnings are
crates/phphttp's `src/header.rs`.

