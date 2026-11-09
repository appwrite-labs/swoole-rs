# Clients local ABI

Feature bit 1048576 (`clients`). ABI:
`swoole_rs_clients.h`. Upstream's client, address, DNS and name-resolver sources
have left the build; the resolver list and `NameResolver::Context` are
`crates/corebase/src/name_resolver.rs`, and `native/clients/swoole_client.h` is the
owned `network::Client` header.

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
calls the pool directly; its tests are in `crates/clients/src/pool.rs`.
Rust stores binary keys and FIFO queues of opaque Client tokens using a
deterministic hasher and fallible allocation; there is no C++ map/queue. Only
native code closes or deletes Client tokens; dropping the Rust container never
does. On allocation failure Rust returns a status and native throws `bad_alloc`.

## Address and raw object

`network::Address`'s methods are inline calls into these exports in `native/reactor/swoole_socket.h`.

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

## DNS

No C++ remains. The query is here (`dns.rs`, `cares.rs`); its coroutine side is
`crates/hooks/src/resolve.rs`: `swoole_name_resolver_lookup` (the Rust list,
`swr_v1_core_name_resolver_resolve`, then `System::gethostbyname` in a coroutine or
the blocking lookup), `System::gethostbyname`/`getaddrinfo` over the async pool,
`coroutine::dns_lookup` (`swr_v1_hook_dns_lookup`) and the pool's DNS handlers.
Upstream's `std::string` DNS API (`swoole_set_dns_server`, `swoole_get_dns_server`,
`swoole_load_resolv_conf`, `swoole_set_hosts_path`, `get_ip_by_hosts`) is gone:
callers use the `swr_v1_clients_*` exports. Resolvers are C functions added and
removed with `swr_v1_core_name_resolver_add`/`_remove`.

- `dns_lookup_impl_with_socket` is `clients_dns_lookup`: the DNS server, read from
  `resolv.conf` (`SW_DNS_RESOLV_CONF`) while none is set
  (`SW_ERROR_DNSLOOKUP_NO_SERVER` otherwise), the request id, `clients_dns_query`
  (query packet, one send and one receive, then the answer's A/AAAA addresses as
  text), the invalid-domain warning and the last error. crates/hooks opens and
  deletes the coroutine UDP socket, sets its timeout, and maps `ECANCELED`.
- Both `network::gethostbyname` variants are `clients_gethostbyname`
  (`gethostbyname2_r` with a growing buffer on Linux, `gethostbyname2` under a
  lock elsewhere); `network::getaddrinfo` is `clients_getaddrinfo` into a native
  array of `SW_DNS_HOST_BUFFER_SIZE`.
- `swoole_load_resolv_conf` is `clients_load_resolv_conf` (libc `fopen` and
  `fgets`, and the `fopen` warning); `get_ip_by_hosts` is `clients_hosts_lookup`,
  given a 256-entry space table (the classic locale's for upstream's
  `std::istringstream`).
- With c-ares (`cares_lookup`), `cares.rs` registers the `SW_FD_CARES` handlers and
  the library cleanup, wraps and registers c-ares' descriptors, parks and resumes
  the coroutine, defers the resumes, warns and sets the last error itself.
  Upstream's `SW_TRACE_CARES` trace lines (trace builds only) are not kept.
- `ip_text` writes into 40-byte caller-local storage and returns the length.
  IPv6 is emitted as eight four-digit lowercase groups (as the original DNS
  helper does, not compressed `inet_ntop` text). Native builds the
  `std::string`; the invalid-family assertion stays native.

## Client (`native/clients/network_client.cc`)

`network::Client`'s state (flags, server host and port, connect timer,
watermarks), operations, callbacks and reactor handlers are Rust's
(`crates/clients/src/client.rs`). The callbacks are one `(listener, object)` pair and an
event mask (`swr_v1_clients_listen`); a PHP bailout in one longjmps out, so no C++
exception crosses Rust. The socket's `object` is the Rust state, which the handlers and
`onPackage` read. Warnings log under upstream's function names so the lines match.
The socket IO, sendfile, reactor writes, framed reads and the socket's release call
crates/tls's, crates/reactor's and crates/ports' definitions by their upstream symbols
(those crates sit above this one), and the input buffer is a Rust-owned `String`.
The C++ half is the `swr_v1_clients_native` table over the members of C++ types.
It is Zend-free so core-tests' `SyncClient` and the Rust client cases (through its `swoole_client_*` C calls) link it. The last error is corebase's, which Rust sets directly.
`bind` and `sendto` are whole Rust operations; each method passes a lambda for
its one warning, which logs under the method's own name.
Construction checks static `Socket::is_stream(type)` for fd class and the
created socket's type for dispatch; a null Socket keeps the early return.

Tests: `tests/modules/clients/README.md`.
