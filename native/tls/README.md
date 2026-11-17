# TLS/DTLS boundary

Feature bit 262144 (`tls`). `corebase::tls` defines `swoole_ssl_init`/`destroy`/`get_error`
(`swoole_ssl_init`, `swr_v1_tls_destroy`, `swr_v1_tls_error`), and `dtls::Session` is a Rust
record. No C or C++ reads `SSLContext`; [swoole_ssl.h](swoole_ssl.h) keeps the OpenSSL
switches C++ still reads. The TLS constants stay in Rust (`crates/corebase/src/tls/constants.rs`);
the `tls-constants` gate compares them with the pinned `swoole_ssl.h`.
Tests: `cargo test -p swoole-rs-corebase tls::`, upstream's `protocol/ssl.cpp` TESTs ported to Rust
(`ssl.*` in `hooks-system-rust`, tests/modules/hooks/README.md) and the ssl/dtls PHPT families. Requires OpenSSL 3 (rejected at compile
time otherwise).

Socket writes, close/free operations and process-pool listener creation call this
module directly. The reactor installs its close/default write handlers when it is
created; no startup registration is needed for core socket operations. External
hooks/client reactor initialization and the sleep callback remain registered.

Corebase's Miri job now covers the TLS tests that do not require unsupported foreign
calls. OpenSSL, access(2) and unsupported socket operations remain explicit Miri
exclusions and run in ordinary and ASan tests. The former TLS crate was entirely
excluded from Miri.

## SSLContext (`crates/corebase/src/tls/context.rs`, `owned.rs`)

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
an active session. Rebuilding a context creates a fresh snapshot. Original-test
adapters edit configuration through scoped loans; no raw field view survives a
returned loan. The password callback reads the
configuration's passphrase only while `create` loads the certificate and key files,
then goes: upstream leaves it installed, but nothing reads a key through the context
afterwards. `owner_assign` copies configuration and takes a context reference.
Warnings keep upstream's text and the method name
upstream's `__SW_FUNC__` printed. Upstream's renegotiation `swoole_debug` exists only
in `SW_DEBUG` builds and is not reproduced.

## DTLS (`crates/corebase/src/tls/dtls.rs`)

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
