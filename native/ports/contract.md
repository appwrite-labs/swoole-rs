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

`swr_v1_ports_protocol_plan(SwrPortsProtocolInput)` (input 28 bytes, action 8 bytes,
plan 52 bytes, up to 6 initialized actions) chooses the EOF clamp, protocol
header/offset publications and callback binding IDs/order. Native code applies the
writes through fixed native address tables. Binding IDs:

- read: 1 EOF, 2 length, 3 HTTP, 4 Redis, 5 raw
- package: 1 task, 2 HTTP frame, 3 websocket frame
- length: 1 default, 2 HTTP, 3 HTTP2, 4 websocket, 5 MQTT

This replaces the caller's four-store `mqtt::set_protocol` configuration, using the
native MQTT size fact and existing MQTT length callback address.

## Integer normalization

`swr_v1_ports_number(kind, value)` implements five modes used by `Port::set`:
`SWR_PORTS_U16`, `SWR_PORTS_BUFFER` (positive signed32), `SWR_PORTS_U32`,
`SWR_PORTS_I32`, `SWR_PORTS_U8`. Unsupported kind returns 0. Zend conversion and
option lookup order stay native. `max_idle_time` (`SW_MAX` on a double) stays
native: Rust candidates differed from the C++ reference for a signaling NaN.

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

## Listener options: `swr_v1_ports_listen_step`

Drives admission/order for listen, listening publication, TCP defer/accept-filter/
fastopen, keepalive, user timeout and default high-watermark publication. State is
a stack-local phase (4 bytes, initialized to 0); input 44 bytes; action 12 bytes.
Each call borrows a live, non-aliasing state/input pair; native snapshots are
refreshed after each action and option values are re-read at their original points.

Capability bits: 1 `TCP_DEFER_ACCEPT`, 2 `SO_ACCEPTFILTER`, 4 `TCP_FASTOPEN`,
8 `SO_KEEPALIVE`, 16 `TCP_KEEPIDLE`, 32 `TCP_USER_TIMEOUT`.

Actions: 0 complete; 1/2 listen/publish; 3..10 the original ordered option effects;
11 publish the watermark. Null inputs or invalid phase return -1. Native code keeps
option constants, `<0` vs nonzero warning predicates, errno effects, accept-filter
bytes and the integer `*0.8` watermark arithmetic. Rust does no floating arithmetic.

## Socket creation: `swr_v1_ports_socket_step`

Controls `create_socket` decisions: existing socket/reuse, creation, DTLS reuse,
address, bind/name and final publication. State 4 bytes, input 12 bytes, action
8 bytes. Capability bit 1 = compiled Linux `HAVE_REUSEPORT`; bit 2 = compiled
DTLS+Kqueue. Query actions read the cached Server pointer's `enable_reuse_port`
and current `is_dtls` at their original points.

Native code keeps make_socket/classification/flags, allocation, Address assignment,
syscalls, warnings, last-error and free/close. Cleanup actions set last-error before
native free and return immediately. Existing partial socket-member publication on
failure is preserved. Known: on macOS, querying `SO_ACCEPTCONN` before listen
returns `ENOPROTOOPT` for both native and Rust; import of an already-listening
descriptor is untested.

## Connection count: `swr_v1_ports_connection_count`

`uint64_t swr_v1_ports_connection_count(volatile uint32_t *counters, uint32_t count, uint32_t fallback)`
sums the per-worker counters with relaxed `AtomicU32` loads. Null `counters` selects
`fallback` regardless of `count`; non-null with `count == 0` reads nothing. Cells
must be aligned, initialized and stable for the call with no concurrent non-atomic
writes. This is not a coherent aggregate snapshot and synchronizes no other fields.
Native shared-memory allocation and worker lifecycle stay authoritative.

## Framing setters and defaults

- `swr_v1_ports_framing_settings(SwrPortsFramingInput)` (40-byte input, 32-byte
  output, no pointers). Kind `SWR_PORTS_EOF_SETTINGS` or `SWR_PORTS_LENGTH_SETTINGS`.
  Rust owns EOF capacity/direction and length-field narrowing. Offset overflow keeps
  the original u16 truncation (not clamping). Length width is the result of the pure
  `swoole_type_size` helper (`include/swoole_protocol.h`, `src/core/base.cc`),
  evaluated natively before publication. EOF memcpy stays native.
- `swr_v1_ports_stream_settings()` returns fixed network-u32 framing: type `N`,
  width 4, offset 0, body 4, enabled 1. Native binds the default-length callback in
  the order of the pinned Stream helper; `src/network/stream.cc` is unchanged.
- `swr_v1_ports_defaults(max_length, eof_length)` returns 24 bytes: object id 0,
  network-u32 type/size/body defaults and narrowed fields. Native publishes them in
  original order, copies the EOF literal, then stores the server pointer.

## `Port::set` helpers

- `swr_v1_ports_flag_setting(kind, value, present)` reuses the 52-byte plan with at
  most 2 writes (field IDs 9..19). Owns truth normalization, missing NODELAY = true,
  and the implications EOF-split -> EOF, WebSocket -> HTTP, HTTP2 -> HTTP.
- `swr_v1_ports_buffer_setting(value)` returns `{u32 count, i32 first, i32 last}`:
  the I32-clamped store, followed by `INT_MAX` when nonpositive, in that order.
- Packet preparation returns `SwrPortsPacketSetting {u32 value, u32 status, size_t copy_len}`:
  - `packet_eof(length, capacity)`: status 1 empty (after native u8 publication),
    2 over capacity, 0 bounded copy. Rejecting an over-capacity raw copy is a
    deliberate safety correction over the original undefined copy.
  - `packet_width(width)`: narrows the `swoole_type_size` result; status 1 on zero.
  - `packet_offset(value, limit)`: U16 clamp plus IPC bound; status 1 too large.
  Native field publication precedes the native diagnostic in each case.
- `swr_v1_ports_option(index)` returns a 56-byte `SwrPortsOption` from an immutable
  31-entry catalog: key/alias spans plus action/target/converter/number_kind/policy/
  reserved (0). Out-of-range index returns all-zero (`SWR_PORTS_OPTION_END`). Native
  code interprets one instruction at a time: Zend lookup (alias only when primary is
  missing/null), conversion, Rust scalar plan, then field publication or a native
  special effect. The catalog owns option order, converter selection, missing NODELAY,
  the lazy body-start alias and WebSocket helper position.

## TLS frontend

No crypto behavior moves to Rust.

- `swr_v1_ports_tls_option(group, index)`: group 0 = 2 context file keys,
  group 1 = 12 Port method keys before SNI. Missing/null options skip.
- `swr_v1_ports_tls_value(target, raw)`: compression inversion, boolean arguments,
  raw protocol bits and the U8 verify-depth clamp.
- `swr_v1_ports_sni_plan(stage, has_key, is_array)` returns 24 bytes
  `{key span, u32 status, reserved 0}`. Stage 0 returns the `ssl_sni_certs` key;
  stage 1 status 1 for a non-array mapping; stage 2 status 2 missing string key
  (checked first), 3 item not an array. Other inputs return status 0, no span.

Native iteration, context duplication, file checks, installation, diagnostics and
certificate matching stay native.

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
