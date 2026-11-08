# TLS/DTLS boundary (M21)

Feature value 262144. Upstream targets: `src/protocol/ssl.cc`,
`src/protocol/dtls.cc` and `include/swoole_dtls.h`. OpenSSL 3 keeps all cryptography and the TLS/DTLS
engine; Rust binds it through `openssl-sys`.

Rust owns `SSLContext` configuration/string slots and creates/configures `SSL_CTX`,
registers the password, ALPN, verify and DTLS cookie callbacks, initializes OpenSSL,
and owns the ex_data indexes and error text. Native compatibility views retain
addressable fields and the live password binding.

A DTLS session is a Rust record: its socket, listened and peek fields and its
datagram queue. crates/ports stores it by fd with a holder of the port's shared
`SSLContext`, released with it. Rust builds the BIO method table, whose
write/read/control operate over the socket and the actual queue and flags, and runs
`Session::init`/`listen`. `ssl.cc` and `dtls.cc` have left the build and
`swoole_dtls.h` the tree; no C++ names a session.

No panic crosses the ABI. Returned strings: `swoole_ssl_get_error` is valid until the
thread's next call.

Upstream behavior kept:

- `SSLContext::create` fails and frees the context on any certificate, key, CA
  (with `verify_peer`), ALPN, client CA or cipher failure.
- Protocol method is chosen before zero normalization; DTLS keeps TLS 1.2.
- The verify callback returns 1; verification happens later in callers.
- DTLS: non-peek short reads drop the whole datagram; empty reads retry;
  `PENDING` is the queue total; DTLS listen return values are library-specific
  (0 pending/true, negative/false, positive listened/true).
