# ListenPort policy ABI (ports)

Pinned upstream: Swoole 6.2.4, commit c7e514509451beafd4d70ce5b78943d51e76381c.
Feature bit 8388608 (`ports`). Upstream's `src/server/port.cc` and
`ext-src/swoole_server_port.cc` are deleted; their native glue is in
`src/server/master.cc` and `ext-src/swoole_server.cc`. Declarations are in
[swoole_rs_ports.h](swoole_rs_ports.h). Tests: `tests/modules/ports/README.md`.

## General rules

- Every export is a pure call over copied scalars, borrowed initialized bytes, or
  caller-local state. No native object, Zend value, callback address or mutable
  registry enters Rust, and no pointer is retained after return.
- All native effects (Zend conversion, lookups, syscalls, warnings, field
  publication, callbacks, destruction) run after Rust returns. No Rust frame spans
  a Zend call, callback, destructor, exception or bailout.
- Returned strings and spans point at static immutable Rust storage; callers never
  free them. Protocol labels and option keys are NUL-terminated (length excludes
  NUL). Class-property spans are length-taking with no NUL promised.
- Byte inputs must be readable for their length. Null/empty input and lengths above
  `isize::MAX` are rejected.
- The PHP facade requires 64-bit `zend_long`. Supported hosts: macOS and Linux 64-bit.
  Struct sizes below are for those LP64 hosts.

## Protocol selection and configuration

`swr_v1_ports_protocol(flags, length_type)` selects the existing protocol setup
branch; `swr_v1_ports_protocol_name(flags)` returns its static label. Datagram/DTLS
distinctions and the priority EOF > length > HTTP > MQTT > Redis are preserved,
including for contradictory flags.

`ListenPort::protocol` and `onRead` live in the Rust-owned port settings
(`PortConfig`); native code binds them by reference. `init_protocol`,
`set_length_protocol` and `set_stream_protocol` are Rust symbols
(`crates/ports/src/methods.rs`), and so is the body of the inline
`set_eof_protocol` (`swr_v1_ports_set_eof(port, bytes, length, find_from_right)`).
`swr_v1_port_config_new` gives the constructor's protocol defaults; the inline
C++ constructor stores the server.

## Event catalog and callbacks

One immutable 12-event catalog replaces native event metadata. The native callback
array, PHP properties, callable allocation/free and Server handlers stay native.

- `swr_v1_ports_event(bytes, len, native_casefold)` returns `{index, property}`
  or index -1 with a null/empty span. `on()` uses host-libc `strncasecmp` with the
  original `swoole_strcaseeq` bound (locale-sensitive).
- `swr_v1_ports_callback_property(bytes, len)` (getter) uses Zend ASCII folding in
  Rust and returns a 16-byte static span, or null/zero. Reusing the locale-sensitive
  mode here would change Turkish-locale behavior.
- `swr_v1_ports_callback_hook(index)` returns a native binding ID: 0 none,
  1 Connect, 2 Packet, 3 Close, 4 BufferFull, 5 BufferEmpty. Native code preserves
  an installed hook or binds its existing callback address. Callback enum indices
  are frozen by static assertions.

Existing callback publication/free/error prefixes are not repaired.

## Sockets: `ListenPort::create_socket`, `listen` and `close_socket`

All three are Rust (`crates/ports/src/methods.rs`) under their upstream symbols, over the
Rust-owned settings and socket record. Options are those the platform defines:
`TCP_DEFER_ACCEPT`, `TCP_KEEPIDLE`/`TCP_KEEPINTVL`/`TCP_KEEPCNT` and
`TCP_USER_TIMEOUT` on Linux, `TCP_FASTOPEN` and `SO_KEEPALIVE` on both supported
hosts (`SO_ACCEPTFILTER` is BSD-only). `host` is a Rust string slot in the
settings, which native code reads and writes through a `RustStringView`.
`create_socket` asks native code (the third runner `swr_v1_ports_set_native`
registers) for the two facts it does not own: the server's `enable_reuse_port`
(Linux) and `is_dtls()` (kqueue hosts), each at its original point. On failure
the port keeps pointing at the freed socket, as upstream's does. Known: on macOS, querying `SO_ACCEPTCONN` before listen
returns `ENOPROTOOPT` for both native and Rust; import of an already-listening
descriptor is untested.

## `Port::set`

`Swoole\Server\Port::set()` is Rust (`crates/phpserver/src/port_settings.rs`): every
option's lookup, conversion, bound and order, written into the Rust-owned
`PortConfig` and, on an SSL port, the context's Rust TLS owner. An over-long
`package_eof` is rejected rather than copied. Native code keeps the SNI contexts
(`dup_ssl_context`, `ssl_add_sni_cert`) and `ssl_init`, which `set()` reaches through
the port host table (`swoole_rs_phpserver.h`).

## Class properties: `swr_v1_ports_class_property(index)`

Returns all 19 original Port property declarations (name, visibility, scalar
type/default) in declaration order, then `SWR_PORTS_PROPERTY_END`. The 12 private
names reuse the event catalog. Public: host, port, type, sock, ssl, setting,
connections. Native MINIT calls the Zend declaration APIs after each return.

## Not converted

Native defaults beyond the explicit constructor body, SSL/import/close custody,
container storage, object lifecycle, callback property update/cache publication,
and allocation-failure behavior remain native. Full Server behavior, ZTS, optional
platforms and performance are untested.
