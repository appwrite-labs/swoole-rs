# TLS/DTLS boundary

Feature bit 262144 (`tls`). Patches `src/protocol/ssl.cc` and
`src/protocol/dtls.cc`. Declarations are in [swoole_rs_tls.h](swoole_rs_tls.h).
Tests: `crates/tls` unit tests, upstream `core-tests/src/protocol/ssl.cpp` on both engines
(`tls-native-originals`) and the ssl/dtls PHPT families. Requires OpenSSL 3 (rejected at compile
time otherwise).

## SSLContext (`crates/tls/src/context.rs`)

Rust makes every OpenSSL call that creates and configures the `SSL_CTX`: method
and protocol options, certificate/key/chain, CA paths, client CA list, ciphers,
DH parameters, ECDH curves, ALPN and session id. It owns the password, ALPN,
verify and DTLS cookie callbacks, OpenSSL initialization, the two SSL ex_data
indexes and `swoole_ssl_get_error`'s text.

Native code keeps the `SSLContext` fields (Zend glue, ports and clients read and
write them) and passes a `swr_v1_tls_config` view per call: each string as
`c_str()` plus `length()`, the scalars, a `warn` logger and an accessor for the
current passphrase (the password callback reads it live, like upstream's `this`).
Each `SSLContext` method is a one-line call; `create` writes back the normalized
protocol mask. Warnings keep upstream's text and the method name upstream's
`__SW_FUNC__` printed. The info callback stays native: it updates
`network::Socket` bitfields.

## DTLS BIO decisions

Copied scalars, no state:

- `swr_v1_tls_dtls_new` prepares stable scalar fields and a Rust-owned datagram queue. Native `Session` retains its shared SSLContext and clears the queue before that context is destroyed.
- `dtls_append` copies a payload; `dtls_append_buffer` takes one compatible native Buffer on success. Reads preserve datagram boundaries, short-read consumption, zero-length packets, retry and peek behavior.
- `dtls_control` reads the actual queue and canonical flags, returning only native shutdown/diagnostic effects. Peer bytes copy the canonical socket address union. `dtls_listen` publishes success in the stable listened field.
- `dtls_methods`/`publish`/`take` own method-table publication. OpenSSL method allocation, callback registration and detached table destruction execute in native code after Rust returns.
- Native `Session` scalar aliases remain addressable. Its queue is opaque; direct std::deque layout/API compatibility is not claimed. Value copies use independent queue allocations.

## Native correction: BIO callback-control slot

Pinned `dtls.cc` installs OpenSSL's public dispatcher as the method's
callback-control leaf. The overlay leaves that slot unset so OpenSSL handles the
unsupported operation itself ([BIO_meth_new](https://docs.openssl.org/3.0/man3/BIO_meth_new/)).
This is a behavior change from upstream, not parity. The operation is never invoked
by tests.

## Known issues

- The DTLS cookie secret is the SSL pointer repeated over all 32 bytes; upstream
  fills only the aligned part of its stack buffer.
- `verify_02` (expired client certificate) emits an extra native
  `Socket::ssl_shutdown` warning with the Rust engine and passes only on the
  official runner's retry (WARNED); native passes. The cause is in the Socket code.
- On macOS, separately compiled protocol TUs and the prebuilt DSO Socket use
  different ex_data indices (1 vs 0) under two-level binding, so module tests
  compile the pinned `network/socket.cc` with the protocol TUs.
- BoringSSL and LibreSSL are unsupported.
