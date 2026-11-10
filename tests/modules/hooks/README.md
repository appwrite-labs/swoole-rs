# Hooks

Pin `76c657d88be39b7af6bf85e46bcbac0da7387760`.

## Native-facade gates

Gates whose bodies moved to Rust became `ported_*` tests in `crates/hooks`.
The packet gate (`hooks-packet-policy`) ran the pinned `Socket::recv_packet`,
`recv_packet_with_length_protocol` and `recv_packet_with_eof_protocol` and the
Rust framing over one scripted native socket and compared their traces. Its 240
scenarios, same inputs, are three tests in `crates/hooks/src/packet/ported.rs`,
checked against the pinned bodies' traces in `ported.txt`:

| Scenarios | Rust test |
| --- | --- |
| 0-95, 216-227 (length protocol) | `packet::ported::ported_length_protocol_scenarios` |
| 96-191, 228-239 (EOF protocol, raw when EOF is off) | `packet::ported::ported_eof_protocol_scenarios` |
| 192-215 (raw receive, availability, timer failure) | `packet::ported::ported_raw_receive_scenarios` |

## Original tests

`core-tests/src/coroutine/system.cpp` is Rust: `system/` holds its 20 TESTs, one
function each (`coroutine_system.<name>`, upstream's names, inputs and expected
values), linked with the crates and the static `libswoole.a` that
`hooks-system-rust` (`system.py`, Linux) builds from the pin's thirdparty and
native/'s Zend-free sources. It runs each case in its own process,
excluding `getaddrinfo` (external DNS and network) and `getaddrinfo_timeout` (a
lookup that misses the resolver cache), as the GoogleTest run did. Where
upstream's `System` signature carries C++ library types the cases call the hooks
crate's Rust functions instead (`file_io::read_file`, `wait_signal::system_wait_signal`,
`poll::socket_poll`), or `swr_v1_system_getaddrinfo`.

`core-tests/src/coroutine/hook.cpp` is Rust too: `system/src/hook.rs` holds its 26
TESTs, one function each (`coroutine_hook.<name>`, upstream's names, inputs and
expected values), calling the C ABI `swoole_coroutine_*` exports; the same
`hooks-system-rust` phase runs them. Upstream's test helpers are written out over
libc (`File`, `String`, `create_socket_pair`, `file_get_contents`,
`Address::assign`), `System::gethostbyname` is `swr_v1_system_gethostbyname`,
and `exists` reads the socket's `sock_fd` through the hybrid's record layout.
Before each case the binary writes `TEST_TMP_FILE` as core-tests' run did and
removes the `/tmp/123.txt` that `ftruncate` leaves behind with mode 0.
`statvfs` reads `/proc` where upstream reads `/tmp`: the free counts of the filesystem
`/tmp` sits on change whenever anything on the host writes to it between the two calls.
`sendmsg_and_recvmsg` stays excluded (below). `core-tests/src/coroutine/accept.cpp`'s one
TEST, `coroutine_hook.accept`, is `hook.rs`'s `accept`, in the same suite and phase.

`core-tests/src/coroutine/gethostbyname.cpp` is `system/src/gethostbyname.rs`
(`coroutine_gethostbyname.<name>`, upstream's names, inputs and expected values):
`System::gethostbyname` is `swr_v1_system_gethostbyname` and the DNS cache calls
(`set_dns_cache_capacity`, `_expire`, `clear_dns_cache`, `get_dns_cache_hit_ratio`) are
crates/hooks' `dns_cache.rs` under their upstream symbols. `resolve_cache`,
`resolve_cache_inet4_and_inet6` and `dns_expire` look up public names (www.baidu.com,
ipv6.sjtu.edu.cn, www.gov.cn), so `hooks-system-rust` excludes them as it does `dns.lookup1`
(external DNS and network). `impl_async` has no case: `gethostbyname_impl_with_async` returns a
`std::string` and the hybrid exports no symbol for it (it is crates/hooks `resolve.rs`'s
`gethostbyname_with_async`, which `System::gethostbyname` runs on a cache miss without c-ares).

`core-tests/src/coroutine/socket.cpp` is Rust: `system/src/socket.rs` holds its 54
TESTs, one function each (`coroutine_socket.<name>`, upstream's names, inputs, ports
and expected values), run by `hooks-system-rust`. A `Socket` is the hooks crate's
`Co`, whose methods the cases call: made by `co_socket::make` and deleted by
`swr_v1_hook_co_socket_free`, the vectored members over an `IOVector` owner
`swr_v1_reactor_iov_prepare` makes (as crates/phpruntime does), and its fields
through the record layout (`swoole_rs_co_socket.h`). `enable_ssl_encrypt` and the
`set_ssl_*` setters write the `SSLContext` owner (`swr_v1_tls_owner_new`,
`_set_path` and its string slots and fields); `TimeoutSetter` and
`TimeoutController` are the crate's `TimeoutScope` and its `timed_out` (the `rust_socket_scopes.selected_timeout_restore_and_copy`
witness, whose `TimeoutSetter` left the hybrid header, is a Rust case there too); `pop_read_buffer`/`pop_write_buffer`
take the record's buffer fields. The `Server` fixture `recv_success` and
`recv_fail` fork is a std echo (or close-on-receive) server: those cases assert only
on the client. Excluded as before: `tcp6` (IPv6) and the four proxy cases (a local
HTTP or SOCKS5 proxy).

`core-tests/src/network/dns.cpp` and `network/address.cpp` are Rust too:
`system/src/dns.rs` holds dns.cpp's 15 TESTs and `system/src/address.rs`
address.cpp's 12, one function each (`dns.<name>`, `address.<name>`, upstream's
names, inputs and expected values), run by `hooks-system-rust`. `dns_lookup`,
`get_ip_by_hosts`, `swoole_set_dns_server`/`_get_dns_server`/`_load_resolv_conf`,
and `swoole_set_hosts_path` are called under their upstream symbols, passing
libstdc++'s `std::string` and `std::vector` by layout (a returned `std::string` is
read only outside a coroutine); the resolver list is Rust's
(`swr_v1_core_name_resolver_add`, and `_remove` for the `each` that removes the
user resolver); lookups go through `swoole_name_resolver_lookup_run`. The DNS
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
(compiled into system.py's lib-swoole with `--fixture`), unchanged but for plain
assertion macros in place of GoogleTest's: the five `rust_dns_request.*` cases
(formerly `dns_request_owner.cpp`). `rust_socket_scopes.protocol_snapshot_and_copy_restore`
and `.error_text_publication` (formerly `socket_scopes.cpp`) witnessed the C++
`ProtocolSwitch` and `set_err` overloads; with those gone from the header they are
crates/hooks unit tests, `socket_scope::tests::protocol_snapshot_and_restore` (less
the C++ copy) and `co_io::tests::error_text_publication`. The fixture also holds the allocation fault
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

`core-tests/src/coroutine/uring_socket.cpp`'s `uring_socket.ssl_close_notify` (v6.2.5)
is `ssl_close_notify` in `crates/hooks/src/uring_tls.rs`, with the same exchange and
assertions over the ring's TLS transport; both sessions use an anonymous TLS 1.2 cipher
in place of the pin's `tests/include/ssl_certs`, which the crate's tests cannot read.

Upstream PHPTs for this module run in the merge tier's PHPT gate (the `phpt-*` jobs), which runs every pinned PHPT on upstream and the all-modules hybrid. To attribute a gate regression to this module, see *Attributing a regression* in [corpus/upstream/README.md](../../../corpus/upstream/README.md).

## Known failures

- `hooks-system-rust`: 18 of system.cpp's 20 cases pass; the two `getaddrinfo`
  network cases are excluded. socket.cpp: 49 of 54 pass (Linux aarch64, docker).
  `coroutine_hook.sendmsg_and_recvmsg` is excluded:
  it segfaulted in ~15% of GoogleTest runs on both engines (its child reads the
  finished parent's stack). hook.cpp's other 25 cases pass (Linux aarch64,
  docker); `gethostbyname`, `getaddrinfo` and `socket` need external DNS and
  network.

`core-tests/src/os/timer.cpp` and `protocol/ssl.cpp` are Rust: `system/src/timer.rs`
and `ssl.rs`, one function per TEST (`timer.<name>`, `ssl.<name>`), run by
`hooks-system-rust`; the `timer-native-originals` and `tls-native-originals` lanes,
the last two that compiled core-tests files on their own, are gone. The timer cases
call crates/timer under its upstream symbols, with each lambda's captures as the
node's data; the fixture reads `SwooleTG.timer`. Upstream's shared-memory
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
worker-retirement witness (formerly `tests/modules/process/worker_retirement.cc`, then
this fixture's `rust_worker_retirement.abort_and_reset`) is crates/server's
`worker_methods::tests::abort_and_reset`.

| Upstream TEST | Rust case |
| --- | --- |
| `pipe.unixsock` | `pipe.unixsock` |
| `pipe.base` | `pipe.base` |
| `process_pool.tcp` | `process_pool.tcp` |
| `process_pool.unix_sock` | `process_pool.unix_sock` |
| `process_pool.task_protocol_rejects_malformed_packets` | `process_pool.task_protocol_rejects_malformed_packets` |
| `process_pool.task_protocol_rejects_malformed_socket_packet` | `process_pool.task_protocol_rejects_malformed_socket_packet` |
| `process_pool.tcp_raw` | `process_pool.tcp_raw` |
| `process_pool.msgqueue` | `process_pool.msgqueue` |
| `process_pool.msgqueue_2` | `process_pool.msgqueue_2` |
| `process_pool.message_protocol` | `process_pool.message_protocol` |
| `process_pool.message_protocol_with_timer` | `process_pool.message_protocol_with_timer` |
| `process_pool.stream_protocol` | `process_pool.stream_protocol` |
| `process_pool.stream_protocol_small_packet` | `process_pool.stream_protocol_small_packet` |
| `process_pool.stream_protocol_msgqueue_ignores_package_size` | `process_pool.stream_protocol_msgqueue_ignores_package_size` |
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

`core-tests/src/coroutine/async.cpp` is Rust in `system/src/co_async.rs`, run by
`hooks-system-rust`. `coroutine::async(handler, event, timeout)` and `async(fn)` are
crates/hooks's `swr_v1_hook_async` and `swr_v1_hook_async_job` over `COROUTINE_ASYNC`,
the operations the hooked functions use, and the package calls them through its
`swoole-rs-hooks` dependency. `coroutine_async.gethostbyname` resolves www.baidu.com, so
it is excluded as external network. It checks upstream's
`(\d{1,3}).(\d{1,3}).(\d{1,3}).(\d{1,3})` match as an IPv4 parse, which accepts
inet_ntop's output exactly when that output is a dotted quad.

| Upstream TEST | Rust case |
| --- | --- |
| `coroutine_async.usleep` | `coroutine_async.usleep` |
| `coroutine_async.gethostbyname` | `coroutine_async.gethostbyname` (excluded: external network) |
| `coroutine_async.error` | `coroutine_async.error` |
| `coroutine_async.cancel` | `coroutine_async.cancel` |
| `coroutine_async.timeout` | `coroutine_async.timeout` |

`core-tests/src/coroutine/http_server.cpp`'s two TESTs have no Rust case. Their
assertions are status 200 and the bodies of core-tests' own coroutine httplib server
(`core-tests/include/httplib_server.h`, test-library code), as an httplib client thread
sees them, so a port would test a Rust rewrite of that server and nothing in the
build. The Swoole operations the server runs on already have cases:
`coroutine_socket.bind_success`, `listen`, `accept`, `recv_success`,
`write_and_read` and `cancel` (`SW_EVENT_READ`, which the server's `stop()` cancels its
accept with), `coroutine_system.getaddrinfo`, and `coroutine_base.create`.

| Upstream TEST | Rust case or reason |
| --- | --- |
| `coroutine_http_server.get` | none: it asserts the test library's httplib server, whose Swoole operations are the cases above |
| `coroutine_http_server.post` | none: as `get` |

`core-tests/src/os/wait.cpp`, `os/async.cpp` and `os/os.cpp`'s `os.daemon` are Rust in
`system/src/os.rs`, run by `hooks-system-rust`. The wait cases call
`swoole_coroutine_waitpid`, `swoole_coroutine_wait` and `System::waitpid_safe`; their
children `_exit(0)` where upstream's `exit(0)`, so a forked child skips the parent's
exit handlers. The async cases call `async::dispatch` and read `sw_async_threads()`'s
counts through `swr_v1_async_threads_stats`. They set `SwooleG`'s `aio_*` fields through
crates/corebase's setters, by path. `async.misc`'s `sw_async_threads()->notify_one()` has
no exported counterpart. It wakes a worker with nothing queued, so nothing in the TEST
observes it, and the case leaves it out. `os.daemon` discards its child's status, as
upstream's does, so the child's assertions don't decide the case: its
`isatty(STDIN_FILENO)` needs a terminal on stdin. `async.thread_starts_with_signals_blocked`
is crates/ossupport's `pool::tests::thread_starts_with_signals_blocked`. It uses a new
thread per case rather than a forked child and a pool of its own. Its `thread_init` hook
observes the worker's mask before `swoole_thread_init` blocks signals itself, as
upstream's `std_allocator.malloc` hook did (the Rust pool doesn't allocate through
`SwooleG.std_allocator`).

| Upstream TEST | Rust case |
| --- | --- |
| `os_wait.waitpid_before_child_exit` | `os_wait.waitpid_before_child_exit` |
| `os_wait.waitpid_after_child_exit` | `os_wait.waitpid_after_child_exit` |
| `os_wait.wait_before_child_exit` | `os_wait.wait_before_child_exit` |
| `os_wait.wait_after_child_exit` | `os_wait.wait_after_child_exit` |
| `os_wait.waitpid_safe` | `os_wait.waitpid_safe` |
| `async.dispatch` | `async.dispatch` |
| `async.schedule` | `async.schedule` |
| `async.misc` | `async.misc` |
| `async.thread_starts_with_signals_blocked` | `pool::tests::thread_starts_with_signals_blocked` (crates/ossupport) |
| `os.daemon` | `os.daemon` |
