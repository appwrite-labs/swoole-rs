# ListenPort policy ABI (ports)

Feature bit 8388608 (`ports`). crates/phpserver (`Swoole\Server\Port`) and
crates/server call crates/ports' Rust functions; no C or C++ does, so there is no
header. Tests: `tests/modules/ports/README.md`.

The protocol, event and property functions are pure calls over
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

- `event(name, native_casefold)` returns the event's index and property, or `None`.
  `on()` uses host-libc `strncasecmp` with the original `swoole_strcaseeq` bound
  (locale-sensitive).
- `callback_property(name)` (getter) uses Zend ASCII folding in Rust and returns the
  static property name, or `None`. Reusing the locale-sensitive mode here would change
  Turkish-locale behavior.
- `on()` installs the Server slot a port's Connect, Packet, Close, BufferFull or
  BufferEmpty callback needs (crates/phpserver's `dispatch::port_hook`).

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
`create_socket` receives the server's `enable_reuse_port` (Linux) and reads `is_dtls()`
(kqueue hosts). On failure the port keeps pointing at the freed socket, as
upstream's does. Known: on macOS, querying `SO_ACCEPTCONN` returns `ENOPROTOOPT`
for both native and Rust, so `import` refuses every descriptor there.

## TLS, SNI, import and close

`crates/ports/src/lifecycle.rs` is `ssl_context_init`, `ssl_context_create`,
`ssl_init`, `ssl_create`, `ssl_add_sni_cert`, the SNI callback, `import` and
`close`, over the port's Rust-owned settings, its contexts' Rust TLS owners, the
socket record and OpenSSL. The port's `ssl_context` publishes its Rust owner;
readers take a reference that survives closing the port. The SNI contexts and the DTLS sessions by fd, each with its
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
