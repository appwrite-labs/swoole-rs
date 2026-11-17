# Clients local ABI

The clients C header declares one function, `swr_v1_clients_address_verify(int family, SwrText host)`.
The caller supplies a non-null, NUL-terminated string that stays live and unchanged for the call.
The function returns whether the string is an IP literal of the given family. The inline C++
`network::Address::verify_ip` passes `std::string::c_str()`. The export shares thread-local
address scratch space with `address_string`. Rust callers use `address::verify_ip`, which
parses into local storage. [C declaration](swoole_rs_clients.h) · [C++ caller](../reactor/swoole_socket.h) · [Rust implementation](../../crates/clients/src/address.rs)

## Address and DNS

- Numeric parsing reads through the first NUL. macOS accepts decimal IPv4 octets with
  leading zeros. Linux uses strict octets. IPv4 loopback matches exactly `127.0.0.1`.
  Unsupported families and platform-specific forms such as scoped IPv6 on macOS go through
  `inet_pton`. The DNS text formatter writes all eight IPv6 groups with four lowercase
  hexadecimal digits each. [Address grammar and formatting](../../crates/clients/src/ip.rs) · [Platform parsing](../../crates/corebase/src/net_address.rs)
- `address::assign` validates the port and fills the socket address and its length. For
  Internet addresses it accepts a literal or, when resolution is enabled, looks up the
  host. Unix socket paths use the address storage’s length limit. Callers receive the
  Swoole error code on failure. URL parsing returns the socket type, host range and port,
  with a separate error for ports outside the signed integer range. [Address assignment](../../crates/clients/src/address.rs) · [URL parsing](../../crates/clients/src/lib.rs)
- The DNS configuration holds the server and hosts-file path. Without a configured server,
  the UDP resolver reads the first nameserver from `resolv.conf` in chunks of up to 99 bytes.
  The query sends one packet, reads the reply and decodes A or AAAA records. Coroutine hooks
  supply the UDP socket and timeout, and use c-ares when available. The thread-pool resolver
  handles blocking lookups. [DNS configuration and wire query](../../crates/clients/src/dns.rs) · [Coroutine lookup](../../crates/hooks/src/resolve.rs) · [Pool handlers](../../crates/clients/src/resolver.rs)

## Client lifetime and operations

`client::create` returns an `Owner` on success. Rust callers pass its `Handle` to operations
including `connect`, `send`, `recv`, `shutdown` and `close`. The client owns its input buffer,
TLS context and proxy state. Reactor handlers recover the client from the socket’s object
field. Async callers register a listener and event mask with `client::listen`. PHP connects
those notifications to its registered callbacks. [Client operations](../../crates/clients/src/client.rs) · [PHP callbacks](../../crates/phpclients/src/async_net.rs)

Each PHP thread keeps a lazily created pool of persistent connections because its sockets
belong to that thread’s reactor. The pool queues connections in FIFO order under binary
server IDs. The PHP client takes and returns connections through this pool. [PHP pool ownership](../../crates/phpclients/src/net.rs) · [Queue implementation](../../crates/clients/src/pool.rs)

The native client cases exercise the Rust operations directly. The PHP fixtures cover
callback reentry, close, socket ownership and persistent connections. [Fixture inventory](../../tests/modules/clients/README.md)
