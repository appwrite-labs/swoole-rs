# Clients and resolver boundary (M22)

Feature value 1048576. Upstream targets:
`src/network/{address,client,dns}.cc`, `ext-src/swoole_client.cc`,
`swoole_client_async.cc`, `swoole_client_coro.cc`, `swoole_name_resolver.cc`
and `include/swoole_client.h` (deleted; `native/clients/swoole_client.h` is the owned copy). `include/swoole.h` and
Stream are unchanged. Follows the
[policy/effect split](abi-v1.md#policyeffect-split); data crosses as `repr(C)`
scalars, opaque tokens, borrowed spans and caller-owned output with checked
capacity.

## Address parsing

- Rust makes regex/grammar/family/port/path decisions. C-string/NUL handling,
  errno, narrowing and exception order match upstream. The type is published
  before failure, as upstream. Unix socket length/path rules are kept.
- Loopback is the exact string `127.0.0.1`, not `127.0.0.0/8`.
- Numeric IPv4/IPv6 parsing runs on copied bytes up to the first NUL. The caller
  picks the platform grammar: Apple accepts leading zeros in IPv4 and in IPv6
  dotted tails, Linux rejects them. Scoped IPv6, DNS, unsupported-family errno,
  sockaddr writes and `inet_ntop` stay native.

## Resolver

Rust selects codec, hosts file, resolv.conf, query, result and error handling on
bounded snapshots, keeping tokenization/locale, comments, alias and first-result
order, family coercion and invalid-port fallback. c-ares, addrinfo/hostent
allocation, channel/socket maps, timers, cancellation and coroutine callbacks are
native. `SwooleG.name_resolvers` stays the single registry; M07 owns the DNS LRU.

## Clients

`network::Client`'s state and every operation are Rust's
(`crates/clients/src/client.rs`): construction and destruction, connect (sync,
async, datagram, DNS wait), send/recv, the SOCKS5 and HTTP proxy handshakes, TLS,
close, shutdown, the watermarks, and the reactor, timer and DNS completions. The
C++ `Client` keeps the members of C++ types (callbacks, proxies, TLS context,
protocol, input buffer, addresses) and hands Rust one static table over them,
`swr_v1_clients_native`; Rust reads the socket's record directly. SOCKS5 rewriting
happens before HTTP proxy rewriting. A callback may reenter, close, delete, throw
or bail out, so no Rust borrow of the state spans one; after `onClose` only the
saved Socket's object-null guard permits further access. A thrown exception ends
the Rust operation and C++ rethrows it. The destructor clears `Socket::object`
before deferred free and deletes a pending connect timer; timer cancellation
precedes `onConnect`.

Persistent connections: the private `long_connections` map of heap queues in
`ext-src/swoole_client.cc` is one Rust-owned pool with binary keys and FIFO
queues of opaque Client tokens (take/push/remove-if-empty). It is created lazily
at the original allocation point and uses a deterministic hasher (no
`RandomState` entropy). Allocation is fallible; native throws `bad_alloc` after
return. Dropping the pool frees only Rust containers, never Client tokens.
Upstream never reclaimed residual queues at teardown; Rust does.

Original inventory: 104 raw-client, 2 resolver, 89 HTTP coroutine-client and 18
cross-family resolver PHPTs.
