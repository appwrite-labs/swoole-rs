# Listening ports

The server owns each listening port’s settings in `PortConfig`. The ports crate
supplies the socket operations, protocol selection and callback catalog used by
the server and the PHP binding. [Port settings](../../crates/server/src/port_config.rs) · [Port tests](../../tests/modules/ports/README.md)

## Protocol selection

`get_protocols` returns the label `dgram` for a datagram port. For streams it
selects the first enabled protocol in the order EOF, length, HTTP, MQTT and
Redis, with `raw` as the default. HTTP labels also identify enabled HTTP/2 and
WebSocket support. `init_protocol` installs the protocol callbacks and readable
handler in the port settings. It leaves ordinary datagram ports alone and
configures DTLS ports for reads. [Protocol selection](../../crates/ports/src/methods.rs)

## Listening sockets and TLS

At startup, `register_methods` gives the server the eight functions in
`PortMethods`. The socket operations are `import`, `create_socket`, `listen`,
`close_socket` and `close`. The other entries are `get_protocols`,
`init_protocol` and `destroy_http_request`. Import checks the descriptor’s type,
address and `SO_ACCEPTCONN` before adopting it. A failed import leaves the
descriptor with the caller. On macOS the listening-socket probe can return
`ENOPROTOOPT`. [Method table](../../crates/ports/src/lib.rs) · [Socket creation and options](../../crates/ports/src/methods.rs) · [Import and close](../../crates/ports/src/lifecycle.rs)

Linux and macOS support `TCP_FASTOPEN` and `SO_KEEPALIVE`. On Linux the port
also applies configured defer-accept, keepalive timing and retry, and TCP user
timeout options. A socket option failure logs a warning and leaves the port
listening. [Listening socket options](../../crates/ports/src/methods.rs)

The server initializes the port’s TLS context, creates TLS sessions for accepted
connections and releases the port’s context and SNI state at teardown. The first
matching SNI context in insertion order wins. PHP’s `Port::set()` applies
port settings first. On an SSL-enabled port with a context, it also applies TLS
settings. It reaches `context_dup`, `sni_add` and
`ssl_init` through the ports crate’s re-exports. The packet EOF delimiter accepts
one to eight bytes. [TLS ownership and SNI](../../crates/server/src/port_tls.rs) · [PHP port settings](../../crates/phpserver/src/port_settings.rs)

## PHP events and properties

The event catalog pairs 12 event names with their callback properties.
`Port::on()` uses locale-sensitive libc case folding and installs the server
hook for Connect, Packet, Close, BufferFull or BufferEmpty when needed.
`getCallback()` matches event names using Zend’s ASCII case folding.
[Event lookup](../../crates/ports/src/lib.rs) · [Server hooks](../../crates/phpserver/src/dispatch.rs)

During PHP class registration, `class_properties()` yields 19 declarations in
order. The 12 private callback properties default to null. The public properties
are `host` (null), `port` (0), `type` (0), `sock` (−1), `ssl`
(false), `setting` (null) and `connections` (null). [Declaration iterator](../../crates/ports/src/lib.rs) · [PHP class registration](../../crates/phpserver/src/port.rs)
