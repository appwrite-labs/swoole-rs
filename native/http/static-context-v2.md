# StaticHandler context (`swr_v2_http_context_*`)

`StaticHandler` (the only one since #752; HTTP/1 and HTTP/2 both use it) holds two
pointers: the external Server and one unique opaque Rust context. It has no native
strings, containers or stat object. The class layout differs from upstream's; SDKs
that use it must be rebuilt.

## What the context owns

- Original and current URL. Both share one byte vector until a rewrite publishes a
  new value.
- Directory, index, filename, boundary, multipart content and end bytes.
- Stat observations, status/content accounting and path state.
- The directory owner ([directory-v2.md](directory-v2.md)) and the task owner
  ([static-tasks-v2.md](static-tasks-v2.md)), held by value. Empty ones allocate
  nothing, and neither handle escapes the context.

Accessors:

- `context_text` copies bytes into separate native scratch.
- `context_publish` accepts copied external observations.
- `context_format` builds content and final-boundary bytes in Rust storage, and
  native IO reads them back through copied views.
- `context_observe` returns a fully initialized DTO plus directory/task counts.

Stat fields are valid only after a successful stat. A failed stat does not read
uninitialized fields.

The filename buffer has `PATH_MAX` capacity and is initialized in Rust. Its
logical/base length is separate from the full first-NUL filename. The index helper
appends without updating the logical length, which keeps the pinned
repeated-append slash behavior. Native filesystem and send calls use the full C
prefix. Successful realpath bytes are copied into this buffer before the native
scratch goes away.

## Effect protocol

Native runs every filesystem, regex, calendar, RNG, MIME, buffer, allocation and IO
operation after Rust returns. No Rust frame or borrow spans an effect.

- **Serve/path**: Rust builds and updates the configuration, stat, canonical and
  MIME facts, publishes directory and canonical bytes, advances path selection and
  applies the terminal status. Each `try_serve` resets the phase/URL-length scratch
  but keeps the handler's last/base-length fields. The Server location container
  and its iteration stay native.
- **Rewrite**: `try_serve` first calls `swr_v1_server_static_rewrite`, which applies
  the server's Rust-owned rules (`RewriteRules`) to the context's current URL. A
  literal prefix match always stops selection; a regex result equal to the current
  URL continues to the next rule.
- **Directory and index**: Native keeps the `DIR*` and exposes each `readdir` name
  only until the next effect, then closes before Rust sorts and dedups. Rust returns
  a scalar cursor into the configured index vector, and native passes back the one
  freshly read name. The configured count is read again after the scan. Autoindex
  is read only if the first matched name is empty or no name matches. Empty scans
  can be reopened; cached non-empty directories skip a second open. Page
  header/entry/footer order lives in Rust, and the directory count is captured
  after the native header append.
- **Ranges** (`static_context/range.rs`): Rust owns the parse cursor, pending
  part, last length, finalization and accounting, reusing `range_next`,
  `range_finish`, header formatting and task storage. Native handles three effects:
  paired MIME/boundary-RNG header observations (keeping the compiler's argument
  evaluation order), final-boundary readiness, and the If-Range calendar result.
  `range_begin` resets per-call scratch but keeps tasks committed earlier. The
  range text is read fresh on each step. EOVERFLOW from header formatting is
  applied by native, and the next step publishes accounting and pushes the task.
  The boundary is built in Rust from a 24-byte RNG fill into 39-byte native stack
  scratch, and the RNG is skipped once the boundary is cached.

## Native scratch lifetimes

- Tasks are copied into one local native snapshot at the original `get_tasks()`
  point, before MIME, header and send. The snapshot lives for the whole send
  sequence and may outlive the context.
- Multipart content is copied into caller-scoped scratch that stays alive through
  header encoding and send.
- The filename uses `PATH_MAX` stack scratch at the sendfile allocation. The final
  multipart bytes use 64-byte stack scratch (38-byte boundary plus 8). End-part
  scratch lives for the whole function, not just the case block.
- Native MIME reference identity is preserved, including on the single-task and
  HEAD paths.

Known difference: allocation, publication and destruction order differ from the
legacy handler.

Tests: `tests/modules/http/context_gate.py`.
