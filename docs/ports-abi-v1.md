# ListenPort and PHP Port boundary (M25)

Feature value 8388608. Upstream targets:
`src/server/port.cc`, `ext-src/swoole_server_port.cc`. Follows the
[policy/effect split](abi-v1.md#policyeffect-split).

Rust owns callback-free policy: native defaults, protocol selection,
listener/option/setup policy and PHP Port settings. TLS setup (context creation,
SNI registration and selection, SNI wildcard matching with libc case folding,
per-socket TLS), socket import and `close` are whole Rust operations
(`swr_v1_ports_run`, `crates/ports/src/lifecycle.rs`) over the same op call as
the receive callbacks. Sockets, socket options, OpenSSL calls, DTLS sessions,
Zend conversion, object/property custody, allocation, logging and callbacks stay
native, as do the callback, SNI, session and connection containers. Return values, errno, warnings, partial
publication and cleanup order match upstream.

The receive callbacks (`readable_callback_*`: raw, length, EOF, Redis and HTTP)
are whole Rust operations (`swr_v1_ports_on_read`, `crates/ports/src/read.rs`).
Rust runs each callback's control flow, including HTTP pipelining, the HTTP/2
preface, WebSocket handover, multipart uploads, chunked bodies and size limits,
It reads and writes the receive buffer's and the request's fields in place through
`SwrPortsShared`, the head of the context (layouts pinned by `static_assert`s),
and asks native code for every other fact or effect through one `call(context, op,
arg)`: `recv`, canned replies, logs and dispatch. The request's own operations
(parsing, multipart) are `swoole_rs_http`'s Rust methods. Native and unchanged:
`destroy_http_request`, worker generation and suspended-response handling.

