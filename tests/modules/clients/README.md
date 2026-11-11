# Clients and resolver migration

The client and resolver (`crates/clients`). Rust owns `network::Client` state and operations,
using the reactor's socket record directly, with its TLS context and proxies.
The DNS lookup (`dns.rs`, `cares.rs`), the persistent client pool and the PHP
classes (`crates/phpclients`) are Rust too; no C++ remains (native/clients/contract.md).

## Actual-body differentials

`crates/clients/tests/upstream.rs` runs the Rust code against the pinned C++ in
`swoole-rs-oracle`: the Set-Cookie split, the `Address` methods (`verify_ip`,
`verify_port`, both `assign`s, `is_loopback_addr`), the hosts-file lookup, the DNS
name codec, the DNS address text and the socket DNS lookup
(`dns_lookup_impl_with_socket` over a UDP socket double).
`network/dns.cpp` and `address.cpp` are Rust cases in `hooks-system-rust` (see
[the hooks README](../hooks/README.md)).

## network/client.cpp in Rust

`core-tests/src/network/client.cpp` no longer builds. `client.rs` holds its 43
TESTs, one function each with upstream's name, inputs, ports and expected values:
TEST `client.<name>` is the case `client.<name>` (function `<name>`). The case
binary is the hooks one (`tests/modules/hooks/system`, which includes this file),
so `hooks-system-rust` builds and runs it over
the crates and system.py's static libswoole.a (the pin's thirdparty and native/'s Zend-free sources). A
`Client` is crates/clients' state: made by `swr_v1_clients_new` and given its
callbacks by `swr_v1_clients_listen`; its methods, `enable_ssl_encrypt`, `ssl_verify`
and its deletion among them, are `client::Handle` calls or other
`swr_v1_clients_*` exports, and the `Socket` TLS reads are called under their
upstream symbols.
`set_tls_host_name` assigns a fresh `SSLContext` with the name set.
`SyncClient` and `AsyncClient` are `Client`'s `created`/`connected` bookkeeping.
`swoole_timer_after` takes the Rust closure through crates/timer's
`swr_v1_timer_add_plain`. The `Server` fixtures are std servers forked after
binding (an echo, the 403 proxy reply, a slow reader): the cases assert only on
the client. Excluded as before: `bind` (EACCES, not EINVAL, off a root container),
`async_tcp_socks5_proxy_handshake_fail` (www.baidu.com:80 timing out a SOCKS5
greeting), `async_tcp_ssl_handshake_fail` (external network: its handshake with
www.baidu.com:80 has a 1 s timeout the round trip sometimes exceeds), the nine proxy cases (a local HTTP or SOCKS5 proxy) and
`sync_ssl_sendfile`: each `recv` lands at the buffer's start, and www.baidu.com's
~730 KB page fills the 64 KB buffer with reads no larger than a TLS record, so
the bytes the assertion finds "baidu.com" in are overwritten and the rest were
never written (uninitialised upstream, zeroed here).
`connect_timeout` and `async_connect_timeout` connect to a local listener whose
accept queue is full, not to the address upstream assumes drops the SYN
(19.168.0.99, 192.168.1.199): a network that rejects the route answers
`ENETUNREACH` instead of letting the connect time out.

`http_format_gate.py` compiles the pinned request formatter calls and
`sw_snprintf` next to `swr_v1_clients_http_format` from the hybrid build's archive
and compares them at bounded capacities:

```sh
python3 tests/modules/clients/http_format_gate.py --source /path/to/pinned-git \
  --archive /path/to/libswoole_rs_ffi.a --output target/clients.json [--sanitize] [--rust-sanitized]
```

`--sanitize` adds native ASan/UBSan; `--rust-sanitized` records that the archive
was ASan-built (requires `--sanitize`).

`python3 -m unittest discover -s tests/modules/clients -p 'test_*.py'` checks
that the gates' shared command recorder (`gate.py`) keeps non-UTF-8 diagnostics.

The persistent-pool witness runs in `clients-unit` (`pool::tests`): the same
100,000 operations, 700,000 state checks, fork isolation, token teardown and
allocation-failure prefixes now call the Rust exports directly. Its retired
C++ RAII adapter had no production callers. PHP pool/fork runtime witnesses
remain separate below.

## PHP runtime witnesses

`php_<family>.py` runs `php_<family>.php` against native and selected engines
using private peers on ephemeral ports. Families: `lifecycle`, `http`, `dns`,
`pool`, `pool_fork`, `resolver`, `http_connection`, `http_format`, `coro`,
`async`, `coro_io`. `socket_custody.py` (`clients-php-socket-custody`) runs
`socket_custody.php` the same way: imported socket and stream ownership, binary I/O,
the factory fallback and hooked pair release.
`php_reentrant_close.py` (`clients-php-reentrant-close`, every lane, Cargo build)
closes a persistent `Swoole\Client` from its `package_length_func` while `recv()`
holds the connection: `recv()` fails with 5001, and the key's next connect is fresh.
`php_property_array.py` (`clients-php-property-array`, every lane, Cargo build)
gives `uploadFiles` and `setting` an object whose `__destruct` writes the
property again: `addData` and `set` keep an array it writes, and `addData`
returns false, leaving the value, when it writes anything else.

```sh
python3 tests/modules/clients/php_http.py --build-manifest BUILD_MANIFEST \
  --engine native=/path/native.so --engine hybrid=/path/selected.so \
  --php-extension curl --php-extension sockets --output target/clients-php-http.json
```

`php_attestation.py` refuses to run unless the selected extension matches the
build manifest; the extension hash must also stay unchanged across the run.
`--native-only` (all except `http`
and `dns`) runs native controls and reports `passed: false`. `php_dns.py`
rejects c-ares builds, which bypass the converted codec. `php_async.py` bailout
mode expects exit 43 on PHP 8.2 and 0 on 8.5; other versions are unsupported.
`php_pool_fork.py` needs pcntl.

## Original PHPTs

Upstream PHPTs for this module run in the merge tier's PHPT gate (the `phpt-*` jobs), which runs every pinned PHPT on upstream and the Cargo build. To attribute a gate regression to this module, see *Attributing a regression* in [corpus/upstream/README.md](../../../corpus/upstream/README.md).

## Known gaps and differences

- macOS Rust ASan archives fail to link (`___asan_version_mismatch_check_v8`);
  Rust ASan runs on Linux only.
- `domain_decode_matches_upstream`'s case count differs by platform because plain
  `char` is signed on macOS and unsigned on Linux.
- Some inputs are outside the compared domain and are not compared; Rust stops at
  bounds on them.
- The Rust pool frees its container buffers at teardown; the original leaks
  residual queues. Neither closes the opaque Client tokens.
- Removing temporary hosts-parser containers and IP-text string appends changes
  allocation-failure behaviour.
- `Address::verify_ip`/`assign`: macOS keeps its decimal leading-zero grammar
  and native `inet_pton` for scoped IPv6; Linux is strict.
- Async gate locale coverage is the C locale only.
- The pinned `SHUT_RDWR` branch sets only `shutdown_read`; this is preserved.
- `network::Client` itself (crates/clients/src/client.rs) is covered by its Rust
  unit tests against recording doubles of the C++ half and of the socket, reactor
  and framing symbols it calls, and by the PHPTs.
