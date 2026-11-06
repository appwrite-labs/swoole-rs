# Buffer queue (`swr_v1_buffer_typed_*`)

Every build uses this queue: one Rust `VecDeque` with its byte ownership, a Rust
front cursor and generation, and ordered sendfile-token and CLOSE entries.
Exports: `create`, `push`, `front`, `consume` (cursor) and `pop` (detach). There is
no native descriptor queue or DATA offset mirror.

- Queued bytes stay counted until the DATA entry is detached. A partial send moves
  only the Rust cursor. Generation and offset are checked before publishing.
- Native callback and token disposal run after Rust returns. Rust stores native
  function addresses but never calls them.
- CLOSE markers and sendfile ownership allocate nothing.

## Layout

- The C++ Buffer holds only the Rust pointer; its methods are Rust
  (`crates/buffer/src/cpp.rs`). `BufferChunk` is just its `Type` enum.
- C/Rust front and detach layouts are checked by `typed.cc` and the crate's tests.
- Creation uses one fallible `Box` and a raw admission/publication adapter.
  Teardown pairs `Box::into_raw` with `Box::from_raw`.

Tests: `tests/modules/buffer/test_typed.py`, the crate's `cpp` tests.
