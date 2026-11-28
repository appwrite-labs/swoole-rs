# Synchronous PROCESS responses after early FIN

In PROCESS mode the reactor can see FIN while a synchronous worker is still
handling the request. Upstream sets `peer_closed` immediately, so both worker and
reactor drop the response, although the client only closed its write half.

## Correction

Applies to non-TLS PROCESS servers with `enable_coroutine=false`, Native
fixed-worker dispatch and no custom dispatch callback. A clean EOF leaves the
write side usable:

- Worker IPC order carries the request, then the close notification; the worker
  finishes its callback before processing the close.
- Response data precedes `TYPE_CLOSE` in the returning stream, and the write
  queue drains before teardown.
- The atomic close-callback guard prevents a second callback when the response
  itself closes the connection.
- `close_notified` is the terminal read state: stale read events are ignored,
  EOF cannot schedule another receive-backpressure timer, and draining the last
  output removes write polling instead of restoring reads.

Unordered/custom dispatch, TLS, thread mode, coroutine-enabled execution, reset,
forced close and read errors keep their upstream classification.

## Cached read after close is queued

A worker can enqueue FILE+CLOSE before an already-returned EOF event is
dispatched; upstream then ran the protocol handler, which closed the connection
and freed the pending file (observed: zero send error, file offset 0 of 4 MiB,
both chunks still queued). The queued-close guard now ignores application reads
once close is queued; TLS WANT_READ still reaches the shutdown retry.

`crates/server/src/conn_io.rs`'s `prequeue_reads_stop_at_a_queued_or_notified_close`
drives the read dispatcher through ordinary reads, queued plain reads, TLS retry
routing and terminal notification.
