# Hooks

Pin `c7e514509451beafd4d70ce5b78943d51e76381c`.

## Native-facade gates

```sh
python3 tests/modules/hooks/packet_gate.py --source PINNED_SOURCE --archive BUILD/rust-prefix/lib/libswoole_rs_ffi.a --sanitize --report OWN/packet.json
python3 tests/modules/hooks/compile_gate.py --source PINNED_SOURCE --archive BUILD/rust-prefix/lib/libswoole_rs_ffi.a --report OWN/compile.json
```

`packet_gate.py` extracts the pinned function bodies and
the patched ones from the hybrid build `--archive` comes from (`hybrid_tree`), compiles both
against a deterministic native facade (`packet_fixture.h`) and runs identical
scenarios through each, comparing effects and order. Syscalls, contexts, TLS
and kernel IO are faked; these are policy/order checks, not runtime proof.
`--sanitize` adds ASan/UBSan to C++ only, not the Rust archive.

`packet_gate.py` (`hooks-packet-policy`, 240 cases) covers the length/EOF/raw
framing paths. Gates whose bodies moved to Rust became `ported_*` tests in
`crates/hooks`.

`compile_gate.py` (`hooks-compile`) compiles all eight original and patched translation units as
C++14: the pinned ones, then the pin with every file the hybrid build patched
(`overlay_hybrid_tree`) under that build's `config.h`. io_uring units are
reported UNSUPPORTED when the config does not enable `SW_USE_IOURING`.

## Original tests

`core-tests/src/coroutine/system.cpp` is Rust: `system/` holds its 20 TESTs, one
function each (`coroutine_system.<name>`, upstream's names, inputs and expected
values), linked against the hybrid tree's `libswoole.so`.
`hooks-system-rust` (`system.py`, Linux, integrate lane) builds that library from
the hybrid tree and runs each case in its own process,
excluding `getaddrinfo` (external DNS and network) and `getaddrinfo_timeout` (a
lookup that misses the resolver cache), as the GoogleTest run did. Where
upstream's `System` signature carries C++ library types the cases call what the
hybrid's callers call instead: `swr_v1_system_read_file`, `_getaddrinfo` and
`_wait_signal`, and `swr_v1_hook_socket_poll` with `System`'s poll operations.

`core-tests/src/coroutine/hook.cpp` is Rust too: `system/src/hook.rs` holds its 26
TESTs, one function each (`coroutine_hook.<name>`, upstream's names, inputs and
expected values), calling the C ABI `swoole_coroutine_*` exports; the same
`hooks-system-rust` phase runs them. Upstream's test helpers are written out over
libc (`File`, `String`, `create_socket_pair`, `file_get_contents`,
`Address::assign`), `System::gethostbyname` is `swr_v1_system_gethostbyname`,
and `exists` reads the socket's `sock_fd` through the hybrid's record layout.
Before each case the binary writes `TEST_TMP_FILE` as core-tests' run did and
removes the `/tmp/123.txt` that `ftruncate` leaves behind with mode 0.
`sendmsg_and_recvmsg` stays excluded (below).

`core-tests/src/coroutine/socket.cpp` is Rust: `system/src/socket.rs` holds its 54
TESTs, one function each (`coroutine_socket.<name>`, upstream's names, inputs, ports
and expected values), run by `hooks-system-rust`. A `Socket` is made and deleted
through `swr_v1_runtime_socket` (its constructors are inline in the hybrid header);
its out-of-line members are called under their upstream symbols, its inline ones
through the `swr_v1_hook_co_socket_*` exports the header calls, and its fields
through the record layout (`swoole_rs_co_socket.h`). `enable_ssl_encrypt` and the
`set_ssl_*` setters write the `SSLContext` owner (`swr_v1_tls_owner_new`,
`_set_path` and its string slots and fields); `TimeoutSetter` and
`TimeoutController` are `swr_v1_hook_timeout_scope` and
`swr_v1_hook_co_socket_timed_out` (the `rust_socket_scopes.selected_timeout_restore_and_copy`
witness, whose `TimeoutSetter` left the hybrid header, is a Rust case there too); `pop_read_buffer`/`pop_write_buffer` are
`swr_v1_hook_co_socket_pop_buffer`; `ssl_handshake` and `ssl_verify` are exported
under their upstream symbols for this. The `Server` fixture `recv_success` and
`recv_fail` fork is a std echo (or close-on-receive) server: those cases assert only
on the client. Excluded as before: `tcp6` (IPv6) and the four proxy cases (a local
HTTP or SOCKS5 proxy).

`core-tests/src/network/dns.cpp` and `network/address.cpp` are Rust too:
`system/src/dns.rs` holds dns.cpp's 15 TESTs and `system/src/address.rs`
address.cpp's 12, one function each (`dns.<name>`, `address.<name>`, upstream's
names, inputs and expected values), run by `hooks-system-rust`. `dns_lookup`,
`get_ip_by_hosts`, `swoole_set_dns_server`/`_get_dns_server`/`_load_resolv_conf`,
`swoole_set_hosts_path` and `swoole_name_resolver_add`/`_each` are called under
their upstream symbols, passing libstdc++'s `std::string`, `std::vector` and
`std::function` by layout (a returned `std::string` is read only outside a
coroutine); lookups go through `swoole_name_resolver_lookup_run`. The DNS
requests drive the resolver record (`swr_v1_core_resolver_*`) as
`network::gethostbyname`/`getaddrinfo` do, and `Address` is the header's inline
calls to `swr_v1_clients_address_*` and `swr_v1_clients_url`. Excluded as in the
GoogleTest run: `dns.lookup1`, `dns.lookup_ipv6` and `dns.bad_family` (external
DNS and network) and `dns.cancel` (a lookup that misses the resolver cache).
`address.record` is the former repo-owned `address_record.cc` check, with the
same assertions: the record's fields and value copies, and the per-thread scratch
`get_addr` and `verify_ip` share.
The repo-owned C++ witnesses of copies and `std::bad_alloc` containment, which only
C++ can throw, are the same binary's cases with bodies in `system/fixture.cc`
(compiled into the hybrid's lib-swoole with `--fixture`), unchanged but for plain
assertion macros in place of GoogleTest's: `rust_socket_scopes.protocol_snapshot_and_copy_restore`
and `.error_text_publication_and_cache_exception` (formerly `socket_scopes.cpp`),
the five `rust_dns_request.*` cases (formerly `dns_request_owner.cpp`) and
`rust_coroutine_lifetime.cancellation_copy_failure_preserves_wait` (formerly
tests/modules/coroutine/lifetime.cpp). The fixture also holds the allocation fault
oracle they share. coroutine/base.cpp and file.cpp run here too
(tests/modules/coroutine/README.md).

`core-tests/src/network/socket.cpp` is Rust: `system/src/network_socket.rs` holds
its 26 TESTs, one function each (`socket.<name>`, upstream's names, inputs, paths,
ports and expected values), run by `hooks-system-rust`; with reactor/base.cpp already Rust, the
`reactor-native-originals` lane that compiled it is gone. `network::Socket` is
Rust's record (`swr_v1_socket`): its
out-of-line members are called under their upstream symbols and its inline ones
through the `swr_v1_socket_*` calls the header makes; `Address` is address.rs's.
`ERR_put_error` is OpenSSL 3's expansion (`ERR_new`, `ERR_set_debug`,
`ERR_set_error`); `sw_tg_buffer()` reads become a 64 KiB buffer; upstream's mutex
handoff between the two threads is a channel. Excluded as in the GoogleTest run:
`socket.connect_sync` and `socket.fail` (external DNS and network) and
`socket.make_server_socket` (the runner's kernel returns `EPROTONOSUPPORT` for an
unprivileged raw socket).

| Upstream TEST | Rust case |
| --- | --- |
| `connect_sync` | `socket.connect_sync` |
| `fail` | `socket.fail` |
| `ssl_fail` | `socket.ssl_fail` |
| `sendto` | `socket.sendto` |
| `sendto_ipv4` | `socket.sendto_ipv4` |
| `sendto_ipv6` | `socket.sendto_ipv6` |
| `recv` | `socket.recv` |
| `recvfrom_sync` | `socket.recvfrom_sync` |
| `send_async_1` | `socket.send_async_1` |
| `send_async_2` | `socket.send_async_2` |
| `sendfile_sync` | `socket.sendfile_sync` |
| `sendfile` | `socket.sendfile` |
| `peek` | `socket.peek` |
| `sendto_sync` | `socket.sendto_sync` |
| `clean` | `socket.clean` |
| `check_liveness` | `socket.check_liveness` |
| `sync` | `socket.sync` |
| `dup` | `socket.dup` |
| `convert_to_type` | `socket.convert_to_type` |
| `get_domain_and_type` | `socket.get_domain_and_type` |
| `make_socket` | `socket.make_socket` |
| `make_server_socket` | `socket.make_server_socket` |
| `make_server_socket_rebind_after_time_wait` | `socket.make_server_socket_rebind_after_time_wait` |
| `ssl_get_error_reason` | `socket.ssl_get_error_reason` |
| `catch_error` | `socket.catch_error` |
| `misc` | `socket.misc` |

`core-tests/src/memory/lru_cache.cpp` no longer builds (`include/swoole_lru_cache.h`
is deleted). Its TESTs are Rust tests over the DNS cache's `Lru` in
`crates/hooks/src/dns_cache.rs`, with the same inputs and expected values:
`lru_cache.basic` is `lru_cache_basic`, `lru_cache.memory_free` is
`lru_cache_memory_free` and `lru_cache.lru_kick` is `lru_cache_lru_kick`.

Upstream PHPTs for this module run in the merge tier's PHPT gate (the `phpt-*` jobs), which runs every pinned PHPT on upstream and the all-modules hybrid. To attribute a gate regression to this module, see *Attributing a regression* in [corpus/upstream/README.md](../../../corpus/upstream/README.md).

## Known failures

- `hooks-system-rust`: 18 of system.cpp's 20 cases pass; the two `getaddrinfo`
  network cases are excluded. socket.cpp: 49 of 54 pass (Linux aarch64, docker).
  `coroutine_hook.sendmsg_and_recvmsg` is excluded:
  it segfaulted in ~15% of GoogleTest runs on both engines (its child reads the
  finished parent's stack). hook.cpp's other 25 cases pass (Linux aarch64,
  docker); `gethostbyname`, `getaddrinfo` and `socket` need external DNS and
  network.
- macOS (clang): `packet_gate.py` stops at a narrowing error in the selected
  `recv_packet` initializer. It passes on Linux, which is where CI runs it.

`core-tests/src/os/timer.cpp` and `protocol/ssl.cpp` are Rust: `system/src/timer.rs`
and `ssl.rs`, one function per TEST (`timer.<name>`, `ssl.<name>`), run by
`hooks-system-rust`; the `timer-native-originals` and `tls-native-originals` lanes,
the last two that compiled core-tests files on their own, are gone. The timer cases
call crates/timer under its upstream symbols; the fixture wraps their
`std::function` callbacks and reads `SwooleTG.timer`. Upstream's shared-memory
counters become `Cell`s (the callbacks run in the case's process), and its
`uint64_t` elapsed-time subtractions wrap as they do in C++. The ssl cases call
`swoole_ssl_*` and `SSLContext::create` under their upstream symbols and build the
context through the `swr_v1_tls_owner_*` and `swr_v1_core_string_set` calls
native/tls/swoole_ssl.h makes; `ERR_put_error` is network_socket.rs's.

| Upstream TEST | Rust case |
| --- | --- |
| `timer.sys` | `timer.sys` |
| `timer.async` | `timer.async` |
| `timer.exists` | `timer.exists` |
| `timer.clear` | `timer.clear` |
| `timer.get` | `timer.get` |
| `timer.delay` | `timer.delay` |
| `timer.error` | `timer.error` |
| `timer.reinit` | `timer.reinit` |
| `timer.realtime_add` | `timer.realtime_add` |
| `ssl.destroy` | `ssl.destroy` |
| `ssl.get_error` | `ssl.get_error` |
| `ssl.password` | `ssl.password` |
| `ssl.failed_create` | `ssl.failed_create` |

`core-tests/src/os/process_pool.cpp` and `os/pipe.cpp` are Rust too: `system/src/process_pool.rs`
and `pipe.rs`, one function per TEST (`process_pool.<name>`, `pipe.<name>`), run by
`hooks-system-rust`; `process-worker-retirement`, the last phase that built core-tests
itself, and `tools/core_tests.py` are gone. The cases call the Rust `ProcessPool`,
`Worker` and `SocketPair` methods under their upstream symbols over zeroed mirrors of
the records Rust owns, which `process_pool` checks against the fixture's `sizeof`s and
offsets. The pool callbacks are `extern "C"` functions, as upstream's are captureless
lambdas; a failed assertion in one aborts the process it runs in (the master or a
worker), as GoogleTest's does only in the master. core-tests' counters are
`sw_mem_pool()` memory, so workers' increments reach the master. `tcp_raw` calls the
raw-bytes `swr_v1_process_pool_ipc_send_sync` and `_listener`, as core-tests' adapted
copy did; `msgqueue_2` discards its child's status, as `spawn_exec_and_wait`'s caller
does. `SyncClient` becomes `std::net::TcpStream` and `UnixStream`. The Server
worker-retirement witness (formerly `tests/modules/process/worker_retirement.cc`, built
in C++14 and C++17 against both engines) is a fixture function now, run once as
`rust_worker_retirement.abort_and_reset` on the hybrid.

| Upstream TEST | Rust case |
| --- | --- |
| `pipe.unixsock` | `pipe.unixsock` |
| `pipe.base` | `pipe.base` |
| `process_pool.tcp` | `process_pool.tcp` |
| `process_pool.unix_sock` | `process_pool.unix_sock` |
| `process_pool.tcp_raw` | `process_pool.tcp_raw` |
| `process_pool.msgqueue` | `process_pool.msgqueue` |
| `process_pool.msgqueue_2` | `process_pool.msgqueue_2` |
| `process_pool.message_protocol` | `process_pool.message_protocol` |
| `process_pool.message_protocol_with_timer` | `process_pool.message_protocol_with_timer` |
| `process_pool.stream_protocol` | `process_pool.stream_protocol` |
| `process_pool.stream_protocol_with_msgq` | `process_pool.stream_protocol_with_msgq` |
| `process_pool.shutdown` | `process_pool.shutdown` |
| `process_pool.reload` | `process_pool.reload` |
| `process_pool.async` | `process_pool.async` |
| `process_pool.async_mb` | `process_pool.async_mb` |
| `process_pool.mb1` | `process_pool.mb1` |
| `process_pool.mb2` | `process_pool.mb2` |
| `process_pool.socket` | `process_pool.socket` |
| `process_pool.listen` | `process_pool.listen` |
| `process_pool.listen_rejects_truncated_stream_packet` | `process_pool.listen_rejects_truncated_stream_packet` |
| `process_pool.listen_rejects_truncated_task_packet` | `process_pool.listen_rejects_truncated_task_packet` |
| `process_pool.listen_unixsock` | `process_pool.listen_unixsock` |
| `process_pool.worker` | `process_pool.worker` |
| `process_pool.add_worker` | `process_pool.add_worker` |
| `process_pool.reload_from_child_exit_callback` | `process_pool.reload_from_child_exit_callback` |
