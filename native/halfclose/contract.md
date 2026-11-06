# M15 native half-close correction

An opt-in Native correction enabled by `--fix-halfclose`; it is not a Rust
conversion. It adds no public ABI or layout, Rust feature bit, connection field,
timer, allocator or second output queue. Without the flag the pinned behavior is
unchanged.

| Patch | Upstream files |
| --- | --- |
| 0001-drain-before-close | `src/server/master.cc`; `BaseFactory::end` runs in `crates/server/src/base_factory.rs`, and its queued-close op in `master.cc` stops reading under the same `SW_HALFCLOSE_DRAIN` |
| 20261103T195726-drain-before-close-in-rust-send-to-connection | `src/server/master.cc`: `send_to_connection` runs in `crates/server/src/send.rs`, so 0001's `master.cc` hunks give way to `SW_HALFCLOSE_DRAIN`, which selects the same pause, resume and queued-close behaviour there. It also defines `SW_HALFCLOSE_PREQUEUE` for the reactor-thread ops that moved to `master.cc` when `reactor_thread.cc` left the build: the read, write and close paths run in `crates/server/src/conn_io.rs`, and the define selects the synchronous prequeue there and, through its registration with Rust, the TLS repairs in [tls.md](tls.md) |

## Queued close

When `BaseFactory::end` or `Server::send_to_connection` appends `TYPE_CLOSE`
behind pending output, reads stop and the existing queue/reactor drain and
destroy the connection. Removing read interest does not cancel an event the
reactor already returned, so the read dispatcher also ignores application reads
once close is queued (TLS WANT_READ still reaches the shutdown retry). Without
this, a cached EOF could run the close handler and free an undelivered
FILE+CLOSE queue.

## Related corrections

- [Synchronous prequeue](prequeue.md): non-TLS PROCESS responses survive an
  early client FIN.
- [TLS shutdown](tls.md): close_notify must actually be written before shutdown
  counts as complete.
- [Expired-peer diagnostic](expired-peer.md): non-null reason text when OpenSSL
  has no queued error.

### Pause/resume

Two upstream transitions contradicted an explicit `pause()`:

- The write handler restored READ after draining output, even when the
  application had removed READ.
- The resume handler returned early for any registered socket. A paused socket
  with pending output is registered for WRITE, so `resume()` did nothing.

Now pause removes READ, drain removes WRITE, and resume adds READ while keeping
any WRITE interest. Resume is a no-op when READ is already enabled or the
connection is `closed`, `peer_closed`, `close_queued` or `close_notified`, so it
cannot reopen a terminal connection.

### Interrupted async sendfile

A zero-progress sendfile that failed with EINTR was classified as a fatal file
error, popping the file chunk so the close marker could be reached after only
the HTTP header. A negative result with errno EINTR now returns to the write loop
with the same chunk and offset. EAGAIN and fatal handling are unchanged.

## Open issues

- Suspended-coroutine HTTP responses lose their output on early FIN (see
  [prequeue.md](prequeue.md)) and on worker termination
  ([worker-terminal.md](worker-terminal.md)).
- Unread extra request data after `Connection: close` (reset/timeout) is not
  handled.
- With the repair enabled, `verify_02` and `verify_03` emit an extra TLS
  shutdown warning ([expired-peer.md](expired-peer.md)).

Tests are indexed in [tests/modules/halfclose/README.md](../../tests/modules/halfclose/README.md).
