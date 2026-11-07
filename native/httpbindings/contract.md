# PHP HTTP binding policy

Feature bit 2097152 (`httpbindings`). ABI: `docs/http-bindings-abi-v1.md`
and `swoole_rs_httpbindings.h`. `ext-src/swoole_http_cookie.cc`,
`swoole_http_request.cc`, `swoole_http_response.cc`, `swoole_http_server.cc`,
`swoole_http_server_coro.cc` and `swoole_websocket_server.cc` are gone.
Public PHP headers, class layouts and stubs/arginfo are unchanged. Rust
allocates and frees `swoole::http::Context`, whose layout is `ContextState`.

Common rule: Rust returns before every native effect (Zend mutation, allocation,
encoding, date/time, warnings, callbacks, IO, codecs, exceptions, bailout) and
retains no context, buffer, Zend value or pointer. Fields are re-observed after
each native effect; there is no second store. Cursors are plain caller-local
values.

## Cookie::toString

`swr_v1_httpbindings_cookie` takes six borrowed spans
(name/value/path/domain/sameSite/priority), expires, encode/secure/httpOnly/
partitioned bits and the last native time difference. A null value span means
absent, distinct from present-empty. A caller-local `u32` cursor selects one
typed native effect per call. Inputs must be disjoint initialized objects;
non-empty spans readable, length <= `isize::MAX`. Malformed cursors/null
objects/spans return -1 with no effect requested. Append text points at current
input storage or Rust static literals and is consumed immediately; warnings are
NUL-terminated static text.

Preserved: warning positions, partial `smart_str` contents, date/encoding/free
order, per-option validation, NUL-terminated `strpbrk` behavior inside binary
`zend_string` values, and expiry bounds. `php_url_encode` (RFC1738),
`php_format_date`, `php_time` and `difftime` stay native; `smart_str` owns every
allocation and `zend_string_free` every temporary. A warning returns after its
callback with no further field access; a bailout leaves no Rust cleanup.

## Request headers and Response header keys

Request header classification returns append/cookie/upgrade/form/multipart/
compression/chunked decisions. Native code still does Zend mutation, parser init,
session lookup, logging and publication (including websocket flag publication
before a failed session lookup). Known-name selection runs after native
`ZVAL_STRINGL`. Known-key indices 0..7 = host, user-agent, accept, content-type,
content-length, authorization, connection, accept-encoding; 8 = unknown (merge).
Pinned llhttp method numbers 0/3/4/28 are compile-asserted at the caller.

Token-list helper (also used by the clients HTTP client; signature unchanged):

- Mode 0 is the native bounded fast comparison using caller-supplied
  `sizeof(pointer)-1`, not the value's `strlen`; on 64-bit, `websock` matches
  `websocket` through this path.
- Only a scan request triggers the native global scratch copy/terminator before
  mode 1. Rust then keeps `strtok_r` comma mutations, NUL termination, trim
  writes, early-match suffix state, aliased value pointers and libc case/space
  semantics. Value must be readable through its NUL (scan) or the compared prefix
  (fast path).

Quote trimming keeps the original shrinking bound: three opening quotes then `a`
trim to one quote then `a`. Response header-key case formatting keeps the
leading/consecutive-hyphen state machine, bounded binary/NUL behavior, and treats
negative lengths as a no-op.

## Coroutine server

The normal server's receive path (`php_swoole_http_server_onReceive`) is
crates/phphttp's: it routes to the TCP, WebSocket and HTTP/2 `onReceive`s and
reads the port callbacks and the received data from ext-src/swoole_server.cc.

Coroutine handler matching uses the native `unordered_map` iteration and
default-callback identity, skipping default entries before any context-path
read. Accept policy consumes native errno constants, connection result and the
signed coroutine ID after native creation/release.

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
original native flag, and unknown headers keep the custom path. The header-emission
tail — the Server / date / upgrade / Connection / type / chunked / compression /
length / final-publication sequence — is upstream C++ again: #372 restored
`build_header`'s tail and deleted the Rust phase machine, so Rust no longer drives
that sequence. What Rust still owns in the response is this classifier and the codec
policy below.

Codec selection happens only after the original acceptance/minimum-length checks.
Declined compression stays a silent false; unknown/unavailable methods take the
native warning path. Zlib negative/zero/maximum level rules and Brotli's field
clamp (min first) are Rust scalar policy. Zstd keeps its captured local level,
native max-then-min query order and local clamp without writing the Context field.
Codec implementations, allocation, warnings and buffer formulas stay native.

Not converted: object setters/reset/refcounts, array/property/GC/destruction,
`getContent`/`getData` string/COW behavior, remaining multipart and Response
methods, HTTP/2 and WebSocket.
