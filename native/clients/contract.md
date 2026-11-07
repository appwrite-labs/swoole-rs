# Clients local ABI

Feature bit 1048576 (`clients`). ABI: `docs/clients-resolver-abi-v1.md`,
`swoole_rs_clients.h`. Patched upstream files: `ext-src/swoole_client.cc`,
`ext-src/swoole_client_async.cc`, `ext-src/swoole_client_coro.cc`,
`ext-src/swoole_name_resolver.cc`,
`src/network/client.cc`, `src/network/dns.cc` (`src/network/address.cc` is deleted).

Production exports have the prefix `swr_v1_clients_`. A test-only allocation-budget
symbol is excluded from production artifacts.

| Area | Exports | Rust responsibility |
| --- | --- | --- |
| Address | `url`, `port`, `ip`, `loopback`, `ip_text` | URL grammar, type and port; port validity; numeric bytes, original loopback policy and DNS text |
| Hosts/resolver settings | `hosts_match`, `resolv_line`, `dns_endpoint` | Borrowed line/token spans; locale/`strtol` stay native |
| DNS codec | `domain_encode`, `domain_decode` | Bounded label encoding/decoding with native `char` signedness |
| Client | `new`, `free`, `connect`, `send`, `recv`, `close`, `shutdown`, `on_*`, … | `network::Client`: state and every operation (see below) |
| Persistent clients | `pool_create`, `pool_destroy`, `pool_push`, `pool_state`, `pool_take`, `pool_remove_empty` | Private pool storage (crates/phpclients keeps the connections) |
| HTTP | `cookie`, `http_format` | Set-Cookie spans; bounded output formatting |

## Common rules

Scalar cursors hold no native address or object. Native code observes copied
facts at the original point, calls Rust, then performs the chosen effect. No Rust
frame or borrow spans a callback, Zend bailout, exception, IO, coroutine
suspension or destructor. Native Client/Socket/Zend fields and route function
slots stay authoritative; static callback metadata is not a mutable registry.

Borrowed spans must be initialized readable/writable storage for their stated
length (see each unsafe export). Returned offsets/lengths refer to that
caller-owned storage; no span is retained. Null/oversized inputs are rejected
defensively. Scalar/span operations allocate no Rust storage. Locale facets,
`strtol` and its errno, buffers, strings, zvals, parsers, crypto, descriptors
and callbacks stay native.

## Persistent pool

`Swoole\Client` (crates/phpclients) holds one lazily created pool handle and
calls the pool directly. The former C++ RAII facade had no production callers
and is retired; its FIFO, binary-key, fork, teardown and failure-prefix cases
run directly against the pool exports in `crates/clients/src/pool.rs`.
Rust stores binary keys and FIFO queues of opaque Client tokens using a
deterministic hasher and fallible allocation; there is no C++ map/queue. Only
native code closes or deletes Client tokens; dropping the Rust container never
does. On allocation failure Rust returns a status and native throws `bad_alloc`.
Unlike the original (which leaks residual heap queues at shutdown), Rust frees
its own buffers on destruction, so allocation topology differs.

## Address and raw object

`src/network/address.cc` is deleted: `network::Address`'s methods are inline calls
into these exports in `native/reactor/swoole_socket.h`.

- Numeric parsing stops at the first NUL of the span. Only successful 4/16-byte
  results are written into existing `sockaddr`/`tmp_address` storage.
- Apple accepts decimal leading zeros; Linux is strict. Dotted IPv6 tails are
  normalized in bounded stack storage before `std::net` parsing.
- IPv4 loopback is exactly `127.0.0.1`, not `/8`.
- `Address::assign(type, host, port, resolve)` is one Rust call,
  `clients_address_assign`: port check, then the sockaddr union and length for
  IPv4/IPv6/raw (literal, else native `gethostbyname` through a function pointer
  when resolving) or Unix (length limit, `strlcpy` copy); it returns the
  `SW_ERROR_*` code native sets as the last error. Native sets `type` first.
- `Address::assign(url)` is `clients_url`: the pinned regexes' grammar, the type,
  ASCII port digits (the classic locale's `\d`) and `std::stoi`'s `int` range,
  which native throws as `std::out_of_range`. Native logs the invalid-URL notice
  and assigns the host span.
- `Address::verify_ip` is `clients_address_verify`, parsing into the scratch
  `addr_str` writes.
- Unsupported families and Apple scoped-IPv6 parsing go to the platform's
  `inet_pton` from Rust, errno included.

## DNS (`src/network/dns.cc`)

- `dns_lookup_impl_with_socket` is `clients_dns_query`: query packet, one send and
  one receive through native coroutine-socket function pointers, then the
  answer's A/AAAA addresses as text. Native keeps `resolv.conf` loading before it,
  the socket and its timeout, and maps `ECANCELED` for Rust.
- Both `network::gethostbyname` variants are `clients_gethostbyname`
  (`gethostbyname2_r` with a growing buffer on Linux, `gethostbyname2` under a
  lock elsewhere); `network::getaddrinfo` is `clients_getaddrinfo` into a native
  array of `SW_DNS_HOST_BUFFER_SIZE`.
- `swoole_load_resolv_conf`'s file read is `clients_resolv_conf` (libc `fopen`
  and `fgets`); `get_ip_by_hosts` is `clients_hosts_lookup`, with the native
  locale's space table passed in. The c-ares path stays native.
- `ip_text` writes into 40-byte caller-local storage and returns the length.
  IPv6 is emitted as eight four-digit lowercase groups (as the original DNS
  helper does, not compressed `inet_ntop` text). Native builds the
  `std::string`; the invalid-family assertion stays native.

## Client (`src/network/client.cc`)

`network::Client`'s state (flags, server host and port, connect timer,
watermarks) and operations are Rust's; see
[the boundary](../../docs/clients-resolver-abi-v1.md#clients). The C++ half is
the `swr_v1_clients_native` table over the members of C++ types, and the
warnings, emitted from functions named as upstream's so the log lines match.
`bind` and `sendto` are whole Rust operations; each method passes a lambda for
its one warning, which logs under the method's own name.
Construction checks static `Socket::is_stream(type)` for fd class and the
created socket's type for dispatch; a null Socket keeps the early return.

Tests: `tests/modules/clients/README.md`.
