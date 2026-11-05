# Queued TLS shutdown

Changes `src/network/socket.cc` (patch 0003) and `src/server/reactor_thread.cc`.
`Socket::ssl_shutdown` runs in `crates/tls/src/upstream.rs`; the halfclose patch
20261104T005648 replaces 0003's method hunks with `SW_HALFCLOSE_TLS_SHUTDOWN`,
which socket.cc registers with Rust (`swr_v1_socket_set_tls_shutdown_repaired`)
to select the repaired shutdown.
No public layout, signature, Rust ABI or second queue.

## Socket::ssl_shutdown

- Upstream premarks both shutdown bits before calling OpenSSL, so the alert is
  never written. The overlay removes that premature publication.
- Every negative result goes through `SSL_get_error`, even with an empty error
  queue.
- A zero return is a successful one-way alert; the peer is not awaited.
- WANT_READ/WANT_WRITE keep the readiness flags and leave `ssl_closed_` false so
  the owner can retry. Fatal results are reported.
- The unfinished-handshake guard and quiet shutdown are unchanged.

`SSL_SENT_SHUTDOWN` alone does not prove delivery: on Linux a method-only fix
still lost alerts after the 4 MiB payload drained because `SSL_shutdown`
returned WANT_WRITE (3).

## Reactor ownership until shutdown completes

For a handshake-ready stream socket with incomplete graceful shutdown, close
keeps (or creates) the CLOSE marker and keeps reactor interest — adding
readiness for a removed socket or updating an existing registration — until
local TLS shutdown completes. This includes an empty application output queue.
Read readiness in that phase retries shutdown instead of calling the protocol
handler. Registration failure falls through to final cleanup. Forced, reset,
peer-closed and unfinished-handshake paths tear down immediately. Coroutine
socket shutdown callers keep their own semantics.

## Test

`cargo test -p swoole-rs-tls shutdown` runs both shutdowns over real TLS 1.2
peers. Paired BIOs check payload-before-alert, one-way shutdown, peer-first,
quiet mode, repeated close/free, unfinished handshake and nonblocking errors; a
bounded BIO checks alert retry once capacity frees; five injected result/error
pairs check empty-queue classification. Peer-death and non-null-reason checks
from [expired-peer.md](expired-peer.md) complete the old gate's checks.

## Open

- A Linux run of prequeue case `0-0-file-fin` once delivered the header and none
  of the 4 MiB body; the cause is unknown and immediate-close acceptance waits on
  it.
- The macOS bad-client warning failure is unresolved.
- Peer-first quiet close, renegotiation, DTLS and thread mode are untested.
