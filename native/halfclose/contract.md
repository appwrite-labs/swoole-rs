# M15 half-close correction

An opt-in correction enabled by `--fix-halfclose`; it is not a conversion of upstream
code. It adds no public ABI or layout, feature bit, connection field,
timer, allocator or second output queue. Without the flag the pinned behavior is
unchanged.

`--fix-halfclose` builds the Rust archive with crates/ffi's `halfclose` feature; it
patches no upstream file and compiles no native code. crates/server's feature runs the
drain and the synchronous prequeue (`send.rs`, `base_factory.rs`, `conn_io.rs`), and
crates/tls's the TLS repairs in [tls.md](tls.md).

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

- Suspended-coroutine HTTP responses lose their output on early FIN and on
  worker termination ([http-response-lifetime.md](../../docs/http-response-lifetime.md)).
- Unread extra request data after `Connection: close` (reset/timeout) is not
  handled.
- With the repair enabled, `verify_02` and `verify_03` emit an extra TLS
  shutdown warning ([expired-peer.md](expired-peer.md)).

Tests are indexed in [tests/modules/halfclose/README.md](../../tests/modules/halfclose/README.md).
