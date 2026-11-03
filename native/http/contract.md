# HTTP native boundary (M04)

M04 is opt-in behind feature bit 256. The overlays cover `src/protocol/http.cc`,
`src/server/static_handler.cc` and `include/swoole_static_handler.h`, whose
`StaticHandler` is a facade over the Rust static context
([static-context-v2.md](static-context-v2.md)). Other public C++ layouts and
signatures, native storage and reference identity, and the third-party parsers
(llhttp, multipart) are unchanged. v1 declarations are in
`swoole_rs_http.h`; v2 owners are described in the companion files listed at the
end.

## General rules

- No Rust function allocates through a native allocator or calls a native
  callback, Zend function, parser or IO operation. Native allocation, exceptions,
  bailouts, libc calendar/RNG, MIME storage, `std::regex`, IO and PHP work happen
  after Rust returns. No Rust borrow outlives a call.
- Outputs and initialized input ranges must be valid, aligned and disjoint, except
  for the in-place URL decode. Empty input may be null where stated.
- Lengths are bounded by `isize::MAX`. Out-of-range or invalid input returns an
  error before any mutation. Rust does not panic on inputs that meet the contract.
- Byte APIs return 0 = success, -1 = invalid, -2 = overflow, unless a function
  documents its own convention.
- Inputs outside the compared domain are rejected by Rust. They are listed per
  section and are not counted as parity.

## Encoding and lookup

- `swr_v1_http_status(code)` returns immutable, NUL-terminated known status text.
  0 and 200 both give `200 OK`; unknown codes return null.
- `swr_v1_http_method(bytes,len)` does an exact-length, locale-sensitive lookup and
  returns method IDs 1..26, or -1. `swr_v1_http_method_string(id)` returns an
  immutable C string, or null for an invalid ID.
- `swr_v1_http_url_capacity(bytes,len,out)` returns the exact allocation size
  including NUL: length + 2 × escaped bytes + 1, with checked arithmetic. `out` is
  zeroed on error. Native allocates once outside Rust.
- `swr_v1_http_url_encode(input,len,output,capacity,written)` escapes with
  uppercase hex and leaves RFC-unreserved ASCII unchanged (binary-safe). `written`
  excludes the NUL. On error the output bytes are untouched and `written` is
  zeroed. If no byte needs escaping, the input is copied in one pass.
- `swr_v1_http_url_decode(data,len,written)` decodes in place. `data` must be
  writable for len+1 bytes, with the first len initialized. `+` becomes a space,
  valid `%XX` escapes decode, and malformed or truncated escapes are left as-is.
  No allocation.
- `swr_v1_http_boundary(input,len,offset,span)` returns 1 = found, 0 = absent, or
  negative on error; the span is zeroed when absent or on error. It keeps legacy
  parameter skipping, case-insensitive `boundary=`, semicolon termination, quotes
  and trailing whitespace. The C++ wrapper publishes only on success.

The native URL wrapper maps invalid sizes to native exceptions and null allocation
to `std::bad_alloc`. Correction: upstream allocates len×3 and can overflow by one
byte (for example `url_encode("!",1)` under ASan). Rust reserves the exact size
once and has no shrink copy.

## Request framing

Native adapters pass independent PODs, never C++ object layouts. `HttpFrame`
carries offset, content length, method, version, error and URL/line/header
metadata.

- `swr_v1_http_frame`: operation 0 = request line, 1 = header terminator. Returns
  0 = complete, -1 = retry/error, with the same partial fields as legacy code (no
  rollback). Known method prefixes stay case-sensitive and permissive.
- `swr_v1_http_chunk_step`: 1 = next chunk, 2 = zero-length chunk (complete),
  3 = more data needed after a valid numeric line, -1 = incomplete initial tail or
  invalid bounds, -2 = invalid syntax. Native logs 1/2/3, sets `excepted` only for
  -2, and publishes known/content length only for 2. Initialize the cursor from
  the buffer offset, pass `initial = 1` only on the first step, and keep the same
  input and header position across steps. Uses C-locale `strtoul` rules.
- `swr_v1_http_expect` returns 1/0.
- `swr_v1_http_header_value` returns 1 = found, 0 = absent; outputs are zeroed if
  absent or on error. Native builds the `std::string` after return. The original
  case matching, first-match, colon and line-break whitespace rules are kept.

Corrections: request lines that previously formed `input.end()+8` without finding
a version, and chunk lengths that pointed past the allocation, are rejected.

## Header metadata and cookies

`swr_v1_http_header_info` parses into `swr_v1_http_header_state` plus multipart
spans. Flags: known 1, keepalive 2, chunked 4, nobody 8, parsed 16. Initialize the
cursor to request-line length + 2 and pass header-end − 4. After result 1, native
publishes the scalar fields and allocates FormData before resuming. 0 = complete;
negative = invalid bounds. The iterator stops at each multipart allocation, so
field mutations and callback timing before an exception match upstream.

Content-length parsing (whitespace, sign, decimal overflow, unsigned negation) is
done in Rust. `number_errors` bit 1 = overflow, bit 2 = the last numeric error had
no digits. Native sets errno after return. A one-time probe with a fixed literal
detects whether libc sets errno on no-conversion (Darwin sets EINVAL).

`swr_v1_http_cookie_next` advances through a native scratch buffer to the next
token and returns key/value spans plus whether `=` was present. Start the cursor at
0. Capacity is at most `isize::MAX`, with bytes initialized through a reachable
NUL. It keeps semicolon/NUL termination, leading C-space trimming, trailing key
spaces, splitting on the first `=` and skipping empty keys. 1 = native calls the
callback, 0 = done, negative = missing terminator or bad bounds. Native may change
or reuse the scratch between steps, including for reentrant parsing. The native
adapter reserves room for the NUL and rebases the input across growth, which
fixes upstream's out-of-capacity termination. See [cookie-flow.md](cookie-flow.md)
for the full flow entry `swr_v2_http_cookie_flow`.

## CONNECT proxy

`HttpProxyRequest` is a C POD with target, Host, version and auth spans, a signed
32-bit port and an authenticated flag.

- `swr_v1_http_proxy_pack` serializes CONNECT and request headers into a disjoint
  native buffer, without allocating. Capacity 0 returns the full length; otherwise
  capacity includes the terminator and `written` is clamped to capacity − 1, like
  `sw_snprintf`. Bytes after the terminator are untouched. C-string NUL handling
  and narrowing precision to int are kept. -1 = invalid pointers; -2 = length,
  result or capacity above `INT_MAX` (written unchanged). The wrapper returns 0 in
  those cases.
- `swr_v1_http_proxy_handshake` returns 1 = accepted, -2 = rejected status (native
  sets last-error), 0 = other or incomplete, -1 = invalid span. It keeps the pinned
  minimum length, case-insensitive `HTTP/1.0`/`1.1` prefix, C-locale whitespace and
  acceptance by `200` prefix. It does not validate the reason text.
- `swr_v1_http_proxy_auth_enabled` is true only when username and password are
  both non-empty. Upstream leaks its unpublished object if a later string
  assignment throws; that is unchanged.
- `swr_v1_http_proxy_credentials` formats `%.*s:%.*s` from two spans into a
  caller-owned 256-byte staging buffer plus a count excluding the NUL, keeping
  int-precision narrowing, embedded-NUL handling and the 255-byte useful prefix.
  Status 0 = success. Status 1 = the formatted length would overflow int: it
  publishes an empty string with count 0, and native sets EOVERFLOW. Negative =
  invalid metadata, storage unchanged.
- `swr_v2_http_proxy_authorization` is used by `HttpProxy::get_auth_str`. It does
  the credential staging, prefix/NUL policy, Base64 (reusing the M09 crate via a
  Cargo dependency, without M09's bit) and terminator. It needs independent,
  writable 512-byte output and aligned count storage. Native builds the returned
  `std::string` and sets errno. Two empty fields still encode the `:`, so the
  native empty-input Base64 case is never reached.

## Static response headers

`swr_v1_http_static_headers` encodes the response headers for 404, 304, directory
autoindex and file/range responses (variants 0/1/2/3). Range kind 0 = no range
headers, 1 = Content-Range, 2 = Accept-Ranges. Ordering, CRLF, signed
content-length, unsigned range-end wrap and the HEAD/keepalive/early-return
policies match upstream. Native passes the software/page constants. Capacity
follows the `proxy_pack` rules. -1 = invalid metadata; -2 = sizes above the
conservative maximum (a 512-byte allowance plus all text, capped at `INT_MAX`).
The former `stringstream` allocation is gone. Numeric grouping from a non-default
C++ global locale is not reproduced.

## Static paths

`HttpStaticPath` is a zero-initialized POD; seed `last` and `filename_length` from
the handler.

- `swr_v1_http_static_path` returns actions from the `SWR_HTTP_PATH_*` enum. Native
  iterates locations, allocates `dir_path`, runs lstat/realpath and MIME lookup,
  then resumes with the `SWR_HTTP_PATH_*` result bits. URL bytes and the terminator
  are copied before `dir_path` is allocated, but the decoded length is published
  only afterward (same as legacy, including on exception).
- `swr_v1_http_static_location` checks one location entry: full-URL, locale-aware
  prefix match, sticky `last`.
- `swr_v1_http_static_index_path` appends an index name. If the byte at the logical
  length is already `/`, no separator is added and the name overwrites that slash
  (this matters on repeated calls).
- Legacy behavior that is kept: `?` wins over `#` even when `#` comes first;
  locations compare the full untrimmed URL; `..` anywhere in the decoded span
  (including past an embedded NUL) triggers canonicalization; relative
  canonicalization checks the root prefix and does not re-stat, while an absolute
  symlink is canonicalized and re-stated.
- Correction: a root longer than capacity, or an index name that would not fit,
  fails before mutation instead of writing out of bounds.

### Conditional dates

`swr_v1_http_conditional` handles If-Modified-Since (kind 0) and If-Range mismatch
(kind 1). `swr_v1_http_date_format` returns format strings for indexes 0..3, or
null. Actions: 0 = false, 8 = true; 1..4 = strptime with formats 0..3; 5 =
gmtime(file); 6 = mktime(parsed); 7 = mktime(file GMT); negative = invalid (the
wrapper returns false). Native keeps one `tm` across failed strptime attempts and
holds the gmtime pointer through both mktime calls. If-Range calls gmtime even
when every parse fails, as in legacy code. Trailing bytes after a successful
strptime are accepted. Only If-Modified-Since has the 63-byte cap. Correction: a
null gmtime after a successful parse, or signed timezone overflow, returns false.

### Directory index page

`swr_v1_http_index_page` renders one page part: 0 = open, 1 = entry, 2 = close.
Inputs are path, entry, CSS and powered-by spans. Capacity 0 returns the required
size; a non-zero capacity gets the available prefix with no NUL, and `written`
always gets the full size. Rust builds all HTML and applies the `.` / root `..`
filter. Displayed text stops at NUL like `%s`; the filter does not. Markup is not
escaped. Header and entry parts reserve a NUL and grow geometrically; the footer
uses an exact reserve and no NUL. An empty `dir_path` is outside the contract
(upstream calls `back()` on it). Correction: zero buffer capacity now makes
progress instead of looping.

### Rewrite rules

- `swr_v1_http_rewrite_rule` returns the pattern span and regex flag. Only a paired
  leading/trailing `~` (length ≥ 2) is stripped.
- `Server::apply_rewrite_rules` drives the static context's rewrite steps
  (`swr_v2_http_context_rewrite_*`). Native keeps the rule container and
  `std::regex`, including `regex_error` continuation. A literal match publishes and stops even when the output equals the
  input; an unchanged regex result falls through to the next rule. The intermediate
  suffix allocation is gone.

### Range parsing and formatting

- `StaticHandler::parse_range` runs in the static context
  (`swr_v2_http_context_range_*`). Suffix and open ranges, skipped unsatisfiable
  starts, zero suffixes, whitespace, trailing commas and CR follow the pinned
  rules; an unrecognized unit falls back to the full file, bypassing If-Range.
- `swr_v1_http_range_format`: part 0 = the multipart header (`snprintf`
  semantics, boundary/MIME stop at NUL, wrapping `offset+length-1`); part 1 = the
  binary-safe final boundary. Capacity 0 returns the full size; otherwise at most
  capacity − 1 bytes plus NUL. A header longer than `INT_MAX` returns 1 (native
  EOVERFLOW) with the first byte reset to NUL and count 0; with capacity 0 it
  returns `SIZE_MAX`. Header capacity must fit `INT_MAX` (the caller uses 512).

### Response body

`swr_v1_http_static_body` takes a POD `{count, cursor, phase, head}` and returns
the next native action: 0 = done, 1 = allocate/copy filename, 2 = part header,
3 = file task, 4 = ending part, 5 = free task, 6 = read keep-alive, 7 = close. HEAD
is read after the header send, so a callback that changes the method still
affects the body; keep-alive is read only after the sends and frees. A single
zero-length task still allocates, copies and frees. Each multipart task (even zero
length) sends part header then file, followed by the ending part. Send errors are
ignored as before. This only covers the final file/range path; 404, 304 and
autoindex keep their native flow.

### StaticHandler helpers

- `swr_v1_http_static_boundary` returns the static prefix and 24 random characters
  only for an empty boundary.
- `swr_v1_http_static_content_prefix` / `swr_v1_http_static_content_type` produce
  the multipart content type only when there is more than one task; with zero or
  one task the MIME catalog reference is returned directly. Capacity 0 returns the
  exact size without a terminator; otherwise the whole value must fit.
- `swr_v1_http_static_catch` sets 404 only for the terminal matched location.

### Static-response selection

`swr_v1_http_static_select` uses 12 bytes of caller scratch (phase, head,
range_kind as three `uint32_t`).

| Phase | Input | Decision |
| --- | --- | --- |
| 0 | try_serve result | skip, or check status |
| 1 | current status | 404, or build dates |
| 2 | conditional header length | call is_modified only if non-empty |
| 3 | is_modified result | 304, or index selection |
| 4 | try_serve_index_file result | skip, or inspect index facts |
| 5 | has-index 1 / autoindex 2 / directory 4 | autoindex only for exactly 6; otherwise parse ranges, copy tasks |
| 6 | current method | HEAD (tag 3) uses zero length + MIME |
| 7 | task count, status after MIME | one task: 206 → range 1, else range 2; other counts 0 |

Phase 8 is terminal. An unknown phase or invalid phase-5 bits returns INVALID
without changing the scratch.

### Index selection, routing, status

- `swr_v1_http_transport_route` returns 1 = WebSocket, 2 = HTTP2, 0 = error.
  WebSocket status ≥ 2 wins even when both flags are set.
- `swr_v1_http_unknown_status` formats a signed 32-bit decimal followed by
  "Unknown Status", using snprintf prefix/NUL rules. See
  [status-buffer.md](status-buffer.md) and [status-codes.md](status-codes.md).

## Multipart

- `swr_v1_http_multipart_data` takes POD counters/flags. Phase 0 picks append,
  file write or reject, and increments `upload_size` before the limit check.
  Returned flag bits: 1 = excepted, 2 = unavailable, 4 = too_large. Actions:
  0 = success, 1 = append, 2 = write (signed count), 3 = close + clear FILE,
  4 = warn/reject, 5 = reject. Unsigned `size_t` additions wrap as upstream.
- `swr_v1_http_multipart_result` classifies the parser return: -1 = error message,
  log, reject; 0 = short, log, reject; 1 = short, log, excepted-accept; 2 = clear
  input, accept.
- `swr_v1_http_multipart_disposition` returns 1 for a case-insensitive
  `Content-Disposition` name, else 0. `swr_v1_http_multipart_filename` uses the
  cookie iterator over native scratch and returns 1 at the first `filename` key.
  Invalid scratch can be partly mutated before rejection.
- `swr_v1_http_multipart_emit` returns an immutable action array and count for a
  callback kind and the initial FILE-presence flag. Actions: 0 = done, 1 = append
  static marker, 2 = append filename (strlen), 3 = append boundary, 4 = fflush,
  5 = fclose then clear, 6 = append header name, 7 = append header value. Effects
  before an action: 1 sets `multipart_header_parsed`, 2 clears it, 4 commits
  content length then `tried_to_dispatch`. Native reads all dynamic operands when
  it runs each action. Unknown kind or null count returns null.
- `swr_v2_http_multipart_upload` runs after the emit plan in
  `multipart_on_header_value`. It decides disposition/filename admission, temp-file
  selection and FILE-publication/error order. Native does the allocation, tempfile,
  fdopen and warnings, and the warning stays in `multipart_on_header_value`.

## Request multipart lifecycle

`swr_v1_http_request_lifecycle(kind, observed, count)` returns immutable `uint32`
action storage (`SWR_HTTP_REQUEST_*`). Kind 0 uses the boundary result, 1 the
parser-allocation result, 2 returns the destruction prefix, 3 uses fresh FILE
presence after that prefix, 4 chooses destructor cleanup from FormData presence.
Invalid kind, non-boolean observation or null count returns null. FILE presence is
read after the temporary body is deleted, the original buffer restored and the
saved field cleared. Partial-initialization cleanup has the same limits as
upstream.

## Versioned owners

- [directory-v2.md](directory-v2.md): `swr_v2_http_directory_*`
- [static-tasks-v2.md](static-tasks-v2.md): `swr_v2_http_tasks_*`
- [static-context-v2.md](static-context-v2.md): `swr_v2_http_context_*`

Tests: `tests/modules/http/` (for example `gate.py`, `static_gate.py`,
`proxy_gate.py`, `multipart_headers_gate.py`).
