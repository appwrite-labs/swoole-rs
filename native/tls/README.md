# TLS/DTLS boundary

Feature bit 262144 (`tls`). `crates/tls` defines `swoole_ssl_init`/`destroy`/`get_error`
and `SSLContext::create` under their upstream symbols, and `dtls::Session` is a Rust
record. [swoole_ssl.h](swoole_ssl.h) keeps the `SSLContext` members C++ still uses; the C declarations are in
[swoole_rs_tls_owner.h](swoole_rs_tls_owner.h).
Tests: `crates/tls` unit tests, upstream's `protocol/ssl.cpp` TESTs ported to Rust
(`ssl.*` in `hooks-system-rust`, tests/modules/hooks/README.md) and the ssl/dtls PHPT families. Requires OpenSSL 3 (rejected at compile
time otherwise).

## SSLContext (`crates/tls/src/context.rs`, `owned.rs`)

Rust makes every OpenSSL call that creates and configures the `SSL_CTX`: method
and protocol options, certificate/key/chain, CA paths, client CA list, ciphers,
DH parameters, ECDH curves, ALPN and session id. It owns the password, ALPN,
verify, DTLS cookie and info callbacks, OpenSSL initialization, the two SSL
ex_data indexes and `swoole_ssl_get_error`'s text.

`SSLContext` is the Rust owner itself, reference counted: `swr_v1_tls_owner_new`
makes one and its first reference, `_clone` adds one and `_free` drops one, so a
port, its SNI contexts and DTLS sessions, a client and a coroutine socket hold
references rather than `std::shared_ptr`s. C++ reads its fields and string slots
through `swr_v1_tls_owner_view`; OpenSSL's password callback reads the passphrase
slot the context retains. A copy (`ListenPort::dup_ssl_context`) is
`swr_v1_tls_owner_assign`. Warnings keep upstream's text and the method name
upstream's `__SW_FUNC__` printed. Upstream's renegotiation `swoole_debug` exists only
in `SW_DEBUG` builds and is not reproduced.

## DTLS (`crates/tls/src/dtls.rs`)

- `swr_v1_tls_dtls_new` makes a session: its socket, `listened` and `peek_mode`, and a datagram queue. crates/ports stores it by fd with a reference to the port's `SSLContext`, released with it; crates/server accepts, feeds and closes it.
- `dtls_append` copies a payload; `dtls_session_receive` reads the socket's next datagram into a new `dtls::Buffer`. Reads preserve datagram boundaries, short-read consumption, zero-length packets, retry and peek behavior.
- Rust builds the one BIO method table (write, read, ctrl, create, destroy) and points each session's BIO at its Rust queue. Control reads the actual queue and flags; peer bytes copy the canonical socket address union.

The table leaves the callback-control slot unset, so OpenSSL handles that unsupported
operation itself ([BIO_meth_new](https://docs.openssl.org/3.0/man3/BIO_meth_new/)).
Upstream installs OpenSSL's public dispatcher there; no test invokes it.

## Known issues

- The DTLS cookie secret is the SSL pointer repeated over all 32 bytes; upstream
  fills only the aligned part of its stack buffer.
- `verify_02` (expired client certificate) emits an extra native
  `Socket::ssl_shutdown` warning with the Rust engine and passes only on the
  official runner's retry (WARNED); native passes. The cause is in the Socket code.
- BoringSSL and LibreSSL are unsupported.
