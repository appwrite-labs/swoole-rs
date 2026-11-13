# ListenPort policy ABI (ports)

Feature bit 8388608 (`ports`). Declarations are in
[swoole_rs_ports.h](swoole_rs_ports.h); crates/phpserver (`Swoole\Server\Port`) and
crates/server call them. Tests: `tests/modules/ports/README.md`.

The `swr_v1_ports_*` protocol, event and property exports are pure calls over
copied scalars and borrowed bytes; no pointer is retained. Returned strings and spans point at static Rust storage. Protocol labels
and option keys are NUL-terminated (length excludes NUL); class-property spans are
not. Supported hosts: 64-bit macOS and Linux.

## Protocol selection and configuration

crates/ports' `protocol(flags, length_type)` selects the existing protocol setup
branch and `protocol_name(flags)` its static label: plain Rust functions, with no
native symbol. Datagram/DTLS
distinctions and the priority EOF > length > HTTP > MQTT > Redis are preserved,
including for contradictory flags.

`ListenPort::protocol` and `onRead` live in the Rust-owned port settings
(`PortConfig`). `init_protocol`, `set_length_protocol`, `set_stream_protocol` and the
inline `set_eof_protocol` are Rust functions over a port's `PortConfig`
(`crates/ports/src/methods.rs`), with no native symbol.
`swr_v1_port_config_new` gives the constructor's protocol defaults.

## Event catalog and callbacks

One immutable 12-event catalog.

- `swr_v1_ports_event(bytes, len, native_casefold)` returns `{index, property}`
  or index -1 with a null/empty span. `on()` uses host-libc `strncasecmp` with the
  original `swoole_strcaseeq` bound (locale-sensitive).
- `swr_v1_ports_callback_property(bytes, len)` (getter) uses Zend ASCII folding in
  Rust and returns a 16-byte static span, or null/zero. Reusing the locale-sensitive
  mode here would change Turkish-locale behavior.
- `swr_v1_ports_callback_hook(index)` returns a binding ID: 0 none,
  1 Connect, 2 Packet, 3 Close, 4 BufferFull, 5 BufferEmpty.

## Sockets: `ListenPort::create_socket`, `listen` and `close_socket`

All three are Rust (`crates/ports/src/methods.rs`), over the Rust-owned settings and
socket record. Only the server calls them, through the table `swoole_init` registers
(`swoole_rs_ports::register_methods`, `swoole_rs_server::port_config::PortMethods`), with
`get_protocols`, `init_protocol`, `destroy_http_request`, `ssl_context_init`,
`ssl_create`, `import`, `close` and the DTLS session lookups: ports depends on the
server, so the server can't name them. `init_protocol` keeps its upstream symbol for
tests/modules/http/server/src/http1/case.rs. Options are those the platform defines:
`TCP_DEFER_ACCEPT`, `TCP_KEEPIDLE`/`TCP_KEEPINTVL`/`TCP_KEEPCNT` and
`TCP_USER_TIMEOUT` on Linux, `TCP_FASTOPEN` and `SO_KEEPALIVE` on both supported
hosts (`SO_ACCEPTFILTER` is BSD-only). `host` is a Rust string slot in the
settings, which C++ reads through a `RustStringView`.
`create_socket` reads the server's `enable_reuse_port` (Linux) and `is_dtls()`
(kqueue hosts). On failure the port keeps pointing at the freed socket, as
upstream's does. Known: on macOS, querying `SO_ACCEPTCONN` returns `ENOPROTOOPT`
for both native and Rust, so `import` refuses every descriptor there.

## TLS, SNI, import and close

`crates/ports/src/lifecycle.rs` is `ssl_context_init`, `ssl_context_create`,
`ssl_init`, `ssl_create`, `ssl_add_sni_cert`, the SNI callback, `import` and
`close`, over the port's Rust-owned settings, its contexts' Rust TLS owners, the
socket record and OpenSSL. The port's `ssl_context` is a `PortConfig` field holding one reference
to its Rust owner; the SNI contexts and the DTLS sessions by fd, each with its
reference, live in the Rust TLS state (`PortConfig::tls`). `swr_v1_server_delete` releases
both. SNI contexts are matched in insertion order; upstream iterated an
`std::unordered_map`, whose order is unspecified.

## `Port::set`

`Swoole\Server\Port::set()` is Rust (`crates/phpserver/src/port_settings.rs`): every
option's lookup, conversion, bound and order, written into the Rust-owned
`PortConfig` and, on an SSL port, the context's Rust TLS owner. An over-long
`package_eof` is rejected rather than copied. `set()` calls `dup_ssl_context`
(`lifecycle::context_dup`), `ssl_add_sni_cert` (`sni_add`) and `ssl_init` in
crates/ports directly.

## Class properties: `swr_v1_ports_class_property(index)`

Returns all 19 original Port property declarations (name, visibility, scalar
type/default) in declaration order, then `SWR_PORTS_PROPERTY_END`. The 12 private
names reuse the event catalog. Public: host, port, type, sock, ssl, setting,
connections.
