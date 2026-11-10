# Output queues and writes

## Socket output progression

`Socket::handle_send`, `handle_sendfile`, `sendfile`, `sendfile_sync` and
`send_async` are Rust (`crates/tls/src/upstream.rs`), over `crates/reactor/src/output.rs`
and the sendfile requests in `file_output.rs`:

- `handle_send` sends the unsent tail of the front data chunk and classifies the
  result with `catch_error`: pop when it all went, store the advanced offset (and
  `send_wait`) after a short write, `send_wait`/`close_wait` or a warning on
  failure.
- `handle_sendfile` corks once when more than `SW_SOCKET_CORK_MIN_SIZE` remain,
  sends a chunk (returning EINTR to the reactor when the halfclose repair, M15,
  is built), classifies the result, records the send time, then uncorks and pops
  once the request is done.
- `sendfile_async` checks the request, makes the output `Buffer` if there is
  none and queues the request, which the queue drops when it pops or drains it.

Server sends, `BaseFactory::end` and the reactor thread's pipe writes use the same
typed Rust queue (`swoole_rs_buffer`).

## Writable callback

`Reactor::_writable_callback` and `drain_write_buffer` are `swr_v1_reactor_flush` and
`swr_v1_reactor_drain` (`crates/reactor/src/flush.rs`), over the `FlushEffects` trait
that `crates/tls/src/reactor_write.rs`'s `Flush` implements.

- DATA (0), SENDFILE (1) or CLOSE (2) pick the send; other kinds fall back to
  DATA. The front kind is read only when the original loop would.
- Each send result is consumed before `close_wait`, then `send_wait`. Once
  drained, timer deletion precedes a final empty check (asked even with no timer)
  and write-interest removal.
