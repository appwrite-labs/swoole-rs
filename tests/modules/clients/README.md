# Clients and resolver migration

The client and resolver (`crates/clients`, patches in
`migration/patches/clients/`). Rust owns `network::Client` state and operations,
using the reactor's socket record directly. C++ retains Protocol, TLS/proxy
objects, callbacks and Zend boundaries through the installed native table.
The persistent client pool is also Rust-owned.

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
so `hooks-system-rust` builds and runs it over the hybrid's libswoole.so. A
`Client` is made, given its callbacks and deleted through network_client.cc's
`swoole_client_*` C calls (the ones crates/phpclients makes); its methods are the
`swr_v1_clients_*` exports they forward to, and `enable_ssl_encrypt`, `ssl_verify`
and the `Socket` TLS reads are called under their upstream symbols.
`set_tls_host_name` assigns a fresh `SSLContext` owner with the name set.
`SyncClient` and `AsyncClient` are `Client`'s `created`/`connected` bookkeeping.
`swoole_timer_after` gets a libstdc++ `std::function` laid out over the Rust
closure. The `Server` fixtures are std servers forked after binding (an echo, the
403 proxy reply, a slow reader): the cases assert only on the client. Excluded
as before: `bind` (EACCES, not EINVAL, off a root container),
`async_tcp_socks5_proxy_handshake_fail` (www.baidu.com:80 timing out a SOCKS5
greeting) and the nine proxy cases (a local HTTP or SOCKS5 proxy).

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
`async`, `coro_io`.

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

Upstream PHPTs for this module run in the merge tier's PHPT gate (the `phpt-*` jobs), which runs every pinned PHPT on upstream and the all-modules hybrid. To attribute a gate regression to this module, see *Attributing a regression* in [corpus/upstream/README.md](../../../corpus/upstream/README.md).

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
  unit tests against a recording double of the C++ half, and by the PHPTs.
