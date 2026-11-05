# Output queues and writes

## Socket output progression

`Socket::handle_send`, `handle_sendfile`, `sendfile`, `sendfile_sync` and
`send_async` are Rust (`crates/tls/src/upstream.rs`), over the policies in
`crates/reactor/src/output.rs` and the requests in `file_output.rs`:

- `handle_send` sends the unsent tail of the front data chunk and classifies the
  result with `catch_error`: pop when it all went, store the advanced offset (and
  `send_wait`) after a short write, `send_wait`/`close_wait` or a warning on
  failure.
- `handle_sendfile` corks once when more than `SW_SOCKET_CORK_MIN_SIZE` remain,
  sends a chunk (returning EINTR to the reactor when the halfclose repair, M15,
  is built), classifies the result, records the send time, then uncorks and pops
  once the request is done.
- `sendfile_async` stays C++ only to create the output `Buffer`: the request's
  construction and range check are `swr_v1_socket_file_checked`, and the queue
  frees it with `swr_v1_socket_file_dispose`.

## Writable callback

`Reactor::_writable_callback` uses `flush_workflow` (4-byte state);
`drain_write_buffer` is upstream's code. Requests 10..15 read empty, front-kind, close-wait, send-wait, timer and
removed.

- Rust picks DATA (0), SENDFILE (1) or CLOSE (2); other kinds fall back to DATA.
  The front kind is read only when the original loop would.
- Each send result is consumed before requesting `close_wait`, then
  `send_wait`. Once drained, timer deletion precedes a final empty check (asked
  even with no timer) and write-interest removal.
- The originally captured Buffer pointer is kept, not replaced by a fresh
  `socket->out_buffer`.

## Server Buffer

Socket send/sendfile, reactor writable dispatch,
`BaseFactory::end` and `ReactorThread_onPipeWrite/onWrite` consume the single
typed Rust queue. Rust owns bytes, ordered kind/token custody, current DATA
offset, generation and byte accounting. Native send/sendfile run after copied
front facts return. Partial progress commits against generation and old offset;
pop keeps full queued-byte accounting until detach. Publication stores the
opaque token and disposer together; the Native `unique_ptr` releases custody in
the next pure workflow phase. Sendfile request construction/validation and
disposal stay Native.
