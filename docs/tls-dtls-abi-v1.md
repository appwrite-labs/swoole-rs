# TLS/DTLS boundary (M21)

Feature value 262144 (`--module tls`). Upstream targets: `src/protocol/ssl.cc`,
`src/protocol/dtls.cc` and `include/swoole_dtls.h`. OpenSSL 3 keeps all cryptography and the TLS/DTLS
engine; Rust binds it through `openssl-sys`.

Rust owns `SSLContext` configuration/string slots and creates/configures `SSL_CTX`,
registers the password, ALPN, verify and DTLS cookie callbacks, initializes OpenSSL,
and owns the ex_data indexes and error text. Native compatibility views retain
addressable fields and the live password binding.

DTLS Session owns its datagram queue and canonical socket/listened/peek fields in
Rust. Its native wrapper retains the shared `SSLContext`; queue cleanup returns
before that shared context is destroyed. BIO read/control/listen operate over the
actual queue and flags. Native code performs socket writes, diagnostics and
OpenSSL method allocation/registration/free after Rust returns. Rust owns atomic
method-table publication; a losing allocation and detached table are freed natively.

SDK consumers must rebuild against the selected headers. `Session::rxqueue` is
opaque; std::deque layout/API compatibility is not claimed. Public socket,
listened and peek aliases refer to stable canonical fields. Value copies hold
independent datagram allocations while the SSLContext remains shared natively.

No panic crosses the ABI. Returned strings: `swr_v1_tls_error` is valid until the
thread's next call.

Upstream behavior kept:

- `SSLContext::create` fails and frees the context on any certificate, key, CA
  (with `verify_peer`), ALPN, client CA or cipher failure.
- Protocol method is chosen before zero normalization; DTLS keeps TLS 1.2.
- The verify callback returns 1; verification happens later in callers.
- DTLS: non-peek short reads drop the whole datagram; empty reads retry;
  `PENDING` is the queue total; DTLS listen return values are library-specific
  (0 pending/true, negative/false, positive listened/true).
