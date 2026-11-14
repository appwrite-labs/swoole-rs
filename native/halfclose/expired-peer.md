# TLS shutdown warning after a rejected client

With the repair enabled, `ssl_shutdown` tries a real close_notify write after a
rejected client's peer has already closed the transport. The write fails with
`SSL_ERROR_SYSCALL` and errno EPIPE. On OpenSSL 3.0.22 the error queue is empty;
on macOS OpenSSL 3.6.5 it holds a system EPIPE entry. It is a terminal failure,
not an incomplete WANT_READ/WANT_WRITE. Upstream never attempts this write
because it premarks both shutdown bits, so the upstream test sees no warning.

`ssl_get_error_reason` used to return reason 0/null for the empty queue. It now
substitutes `no OpenSSL reason available`. The fatal warning and failure/retry
state are unchanged; a failed write is not treated as a delivered alert.

`crates/tls/src/socket.rs`'s `closed_peer` test reproduces terminal EPIPE with a completed TLS
handshake over a nonblocking socketpair and a closed peer, with the non-null
reason checks.

## Compatibility limit

With the repair, `verify_02` and `verify_03` print an extra shutdown warning
(reason 32 plus the fallback text): repair-enabled builds pass 2/4 ordinary
certificate originals, repair-disabled builds pass 4/4. The raw warnings do not
record `sslerr` or errno, so reason 32 must not be read as either.

No correction follows yet: restoring the premarked bits would discard the
alert-delivery fix, and hiding the warning would not make shutdown succeed.
Certificate rejection, failed alert delivery and graceful application shutdown
need to be distinguished before changing either.
