# TLS/DTLS boundary

Feature bit 262144 (`tls`). `crates/tls` defines `swoole_ssl_init`/`destroy`/`get_error`
under their upstream symbols, and `dtls::Session` is a Rust
record. No C or C++ reads `SSLContext`; [swoole_ssl.h](swoole_ssl.h) keeps the OpenSSL
switches C++ still reads. The TLS constants stay in Rust (`crates/tls/src/constants.rs`);
the `tls-constants` gate compares them with the pinned `swoole_ssl.h`.
Tests: `crates/tls` unit tests, upstream's `protocol/ssl.cpp` TESTs ported to Rust
(`ssl.*` in `hooks-system-rust`, tests/modules/hooks/README.md) and the ssl/dtls PHPT families. Requires OpenSSL 3 (rejected at compile
time otherwise).

## SSLContext (`crates/tls/src/context.rs`, `owned.rs`)

Rust makes every OpenSSL call that creates and configures the `SSL_CTX`: method
and protocol options, certificate/key/chain, CA paths, client CA list, ciphers,
DH parameters, ECDH curves, ALPN and session id. It owns the password, ALPN,
verify, DTLS cookie and info callbacks, OpenSSL initialization, the two SSL
ex_data indexes and `swoole_ssl_get_error`'s text.

`TlsContext` holds a thread-local `Rc` builder. Configuration takes a checked
exclusive loan that owns the configuration until it returns it to the builder.
Field and hostname readers receive independent owned copies; a second loan is
rejected. Ports publish independent immutable `TlsSettings` for concurrent TCP,
SNI and DTLS handshakes. Clients and coroutine sockets freeze configuration before
creating a session; accepted sockets retain their listener's snapshot. Handshake
retries and verification read the snapshot, so later builder edits cannot change
an active session. Rebuilding a context creates a fresh snapshot. The raw owner
exports remain for the original-test adapters. OpenSSL's password callback reads the slot its context
retains; `owner_assign` copies configuration and retains that password.
Warnings keep upstream's text and the method name
upstream's `__SW_FUNC__` printed. Upstream's renegotiation `swoole_debug` exists only
in `SW_DEBUG` builds and is not reproduced.

## DTLS (`crates/tls/src/dtls.rs`)

- `dtls_open` makes a session over a socket: its descriptor and peer, `listened` and `peek_mode`, a datagram queue and a reference to the port's `SSLContext`. The session's BIO owns it, so it goes when the socket's SSL is freed, and `DtlsSession::of` finds it through the socket; crates/server accepts, feeds and closes it.
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
