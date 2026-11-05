# Wrapper boundary

Feature bit 2048 (`wrappers`). Header: `swoole_rs_wrappers.h`. Patched
upstream files: `src/wrapper/event.cc`, `src/wrapper/http.cc`. Supported targets:
macOS and Linux. No panic, C++ exception or Zend bailout may cross the ABI.

## HTTP request handle

A private Rust handle owns current header/form/input names, multipart form
accumulation and beginning/completed state. Allocate and free it through Rust
exactly once, tied to the native request lifetime. It must not be copied,
accessed concurrently or used after free. Allocation can return null (native parse
then fails without dereferencing it). Mutations are fallible: `Vec` reservation
failure returns -1 and callback failures propagate.

The llhttp and multipart callbacks are Rust (`crates/wrappers/src/http.rs`, #931).
`ContextImpl::parse` binds the handle to its `Context` and a native effect call
(`swr_v1_wrappers_request_bind`, `SWR_WRAPPER_HTTP_*`), then runs llhttp with
`swr_v1_wrappers_http_settings()`. The handle owns the open multipart parser and
frees it (closing its upload file) through that call when the request goes.

Views are borrowed byte spans; copy what you need before the next mutation or
destruction. Input buffers must be valid, live, not overlap mutable handle state,
and be at most `isize::MAX` long; null with zero length is accepted. Context
maps, the llhttp and multipart parsers themselves, server transport, FILE/temp
files, warnings and `strtol` stay native, as effects. No C++ exception crosses
the Rust callbacks: an upload's `files.emplace` failure terminates.

Header classification keeps llhttp method numbers (DELETE = 0, POST = 3, PUT = 4,
PATCH = 28; static-asserted), ASCII case-insensitive content/header matching, and
case-sensitive disposition parameter names with last value wins. Preserved
quirks: filename presence selects input-name even when empty; a file disposition
does not set form-name; the URL query is assigned only when non-empty; the first
non-empty multipart body skips all leading CR/LF once (including all-EOL
buffers). Header formatting matches `sw_snprintf` `%s` (embedded NUL truncates)
and the 1024-byte buffer truncation, including the terminating NUL.

### `swr_v1_wrappers_disposition`

Decodes the whole disposition parameter stream into the Rust request metadata,
replacing the generic native `parse_cookie` callback in the multipart header
caller (generic cookie parsing elsewhere is unchanged). Input must be initialized
and disjoint from request buffers; no borrow survives return. First NUL ends the
stream, `;` splits tokens, host `isspace` trims leading key whitespace, keys are
case-sensitive, the last duplicate wins, quotes and trailing spaces stay literal,
and filename presence wins even when empty. Missing name and filename returns -1.

### Deliberate fixes relative to upstream

- `ContextImpl` `parser`/`mt_parser` are initialized (upstream left `mt_parser`
  and `form_data_buffer` uninitialized and could read/delete them).
- A disposition without both filename and name is rejected instead of
  dereferencing `unordered_map::end()`.
- Multipart dependencies and partial FILEs are freed on request destruction,
  including aborts and repeated multipart header setup (upstream leaked them).
- `fdopen` failure closes the new descriptor and unlinks the new temp path before
  the warning, preserving errno.
- Upload file-map insertion uses `emplace` before publishing FILE ownership; an
  allocation exception closes the stream, unlinks the path and rethrows with
  errno preserved.
- A failed or partial upload write closes the FILE and unlinks the temp path
  before storing the existing ERROR (1) marker; the original `fwrite` errno
  reaches the warning (upstream leaked the temp file).

## Response

- `swr_v1_wrappers_response_next(previous_action, body_length,
  last_send_result, keepalive)` returns the next `u32` action: header send,
  optional body send, late keepalive observation, optional close, return. No
  pointers or state. Keepalive is read only after successful sends; server/session
  fields are re-read by the native send/close expressions.
- `swr_v1_wrappers_content_length(length, out, capacity)` writes the unsigned
  decimal ASCII (no terminator) and returns the byte count; null output or short
  capacity returns `SIZE_MAX` without writing. Callers use the existing 1024-byte
  stack buffer, statically asserted to hold at least one byte per `size_t` bit.

Header construction, map iteration, format buffer and TLS buffer lookups stay
native.

## Listener address

`swr_v1_wrappers_listener_plan` returns 16 bytes: separator index, admission and
default-host flags. The address bytes are borrowed only for the call. The first
colon splits host and port; an empty host is distinct from an embedded NUL.
Server allocation still precedes validation; host allocation, `strtol`,
`add_port` and callbacks stay native.

## Event wrappers

Event add-or-update direction and wait admission are Rust decisions. The native
facade keeps `if_exit` short-circuiting, invalid-mask assertions, callback
installation, `SwooleTG` ownership, syscalls and teardown. Other event functions
forward to the reactor. Timer wrappers belong to the timer module.

`swr_v1_wrappers_event_init(flags, wait_exit_mask, ready)` returns 20 bytes:
`u32` accepted, `u32` wait_exit and three `u32` initializer IDs. Rejected
readiness returns all zero; otherwise the wait flag is normalized and the
initializer order is Socket, System, Client. Native allocates the Reactor and
calls `ready()` before Rust, runs each initializer after, then publishes the
same owner to `SwooleTG` last.
