# Hooks

Pin `76c657d88be39b7af6bf85e46bcbac0da7387760`.

The socket fixtures borrow `corebase::buffer::string::CppString` directly; client
certificate buffers use `OwnedString`. Packet ownership transfer still uses
`CppString::pop`, with the same original byte, length and offset assertions.

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

The framing host lends initialized bytes and receives through
`Co::recv_spare`; its metadata view carries no pointer. The native receive and
protocol callback end metadata loans before a yield or PHP bailout can abandon
their frames. A parked receive owns its read allocation outside the socket and
restores it on return; teardown cannot free pending kernel write storage.
`pending_read_allocation_survives_socket_teardown` checks a completion write after
teardown, and `completed_read_restores_its_allocation_and_initialized_prefix`
checks normal restoration. `CppString::fill_within` commits only the prefix
verified by `Spare`. The 240 trace expectations
are unchanged. `co_socket::tests::tls_scratch_is_stable_and_independent_of_application_loans`
checks that TLS BIO scratch remains distinct and reusable while both application
buffers are lent; the scratch keeps the socket allocator and is freed with its state.
The `hooks-packet-bailout` gate compares a fatal PHP package-length callback
against upstream; `api-fuzz`'s `websocket-client-push-bailout.php` also checks
teardown while another coroutine has a packet receive parked.

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
`poll::socket_poll`), or `resolve::system_getaddrinfo`. Whole-file operations and
signal waits return typed `Result`s; local adapters retain the original test
assertions and expected integer/optional outcomes. `file_io` also checks that
pool refusal is distinct from an I/O error carried back from the worker thread.

Hooked `read`, `write` and the pool close adapter return a captured I/O error or
`PoolRefused`; the C read/write/close exports keep their original integer ABI.
`stdio::tests::close_errors_cross_the_pool_and_refusal_keeps_errno` checks that
an error from the worker thread survives independently of caller errno and that
refusing the job preserves errno without running close.

Event waits and descriptor polls return typed `Result`s too. `WaitError` preserves
whether validation/immediate polling sets Swoole's last error or a suspended wait
sets errno. Poll failures distinguish an I/O error, no registered descriptor and
timeout; PHP keeps its existing false/zero results. `swoole_coroutine_poll` retains
the native integer ABI. `System::wait_event` had only the Rust original-test caller;
it now calls `wait_event` by path, and the unused C++ symbol is removed. The Rust-only `PollFd` has
no C layout requirement, and the old `WaitEvent` status record survives only as
a test adapter so the original assertions stay intact.

`core-tests/src/coroutine/hook.cpp` is Rust too: `system/src/hook.rs` holds its 26
TESTs, one function each (`coroutine_hook.<name>`, upstream's names, inputs and
expected values), calling the C ABI `swoole_coroutine_*` exports; the same
`hooks-system-rust` phase runs them. Five have no caller outside Rust and are Rust
functions, called by path: `swoole_coroutine_connect`, `_accept`, `_poll_fake`,
`_sendmsg` and `_recvmsg` are `hooked::connect`, `accept`, `poll_fake`, `sendmsg` and
`recvmsg`, over a `SocketAddress` (which `address::assign` and `getsockname` fill), a
slice of pollfds and a `SendMessage` or `RecvMessage` of borrowed buffers (as
`coroutine_socket.sendmsg_and_recvmsg` passes `Co::sendmsg` and `recvmsg`). Upstream's test helpers are written out over
libc (`File`, `String`, `create_socket_pair`, `file_get_contents`,
`Address::assign`), `System::gethostbyname` is `resolve::system_gethostbyname`,
and `exists` reads the socket's `sock_fd` through the hybrid's record layout.
Before each case the binary writes `TEST_TMP_FILE` as core-tests' run did and
removes the `/tmp/123.txt` that `ftruncate` leaves behind with mode 0.
`statvfs` reads `/proc` where upstream reads `/tmp`: the free counts of the filesystem
`/tmp` sits on change whenever anything on the host writes to it between the two calls.
`sendmsg_and_recvmsg` stays excluded (below). `core-tests/src/coroutine/accept.cpp`'s one
TEST, `coroutine_hook.accept`, is `hook.rs`'s `accept`, in the same suite and phase.

The Rust-only hooked `send`, `recv`, `sendmsg`, `recvmsg`, `connect` and `accept`
return `io::Result`, retaining the OS error as well as errno. The original hook
cases convert those results back to their original integer expectations at the
assertion boundary; descriptor ownership, transfer flags and coroutine routing
are unchanged.

The original `coroutine_hook.getaddrinfo` calls `stdio::resolve_addrinfo` by path.
Its `Result` separates resolver codes, `EAI_SYSTEM` with the executing thread's
errno, and pool refusal; the test adapter retains the original 0/`EAI_NONAME`
assertions. The stdio sleep effect reports completion as a boolean: flock preserves its
cancellation behavior, while the exported sleep hooks discard it. Flock keeps
invalid operations, captured I/O errors and cancellation distinct until its C
export maps them to the original statuses. Duration conversions are unchanged.

`core-tests/src/coroutine/gethostbyname.cpp` is `system/src/gethostbyname.rs`
(`coroutine_gethostbyname.<name>`, upstream's names, inputs and expected values):
`System::gethostbyname` is `resolve::system_gethostbyname` and the DNS cache calls
(`set_dns_cache_capacity`, `_expire`, `clear_dns_cache`, `get_dns_cache_hit_ratio`) are
crates/hooks' `dns_cache.rs` (`swr_v1_hook_dns_cache_*`). `resolve_cache`,
`resolve_cache_inet4_and_inet6` and `dns_expire` look up public names (www.baidu.com,
ipv6.sjtu.edu.cn, www.gov.cn), so `hooks-system-rust` excludes them as it does `dns.lookup1`
(external DNS and network). `impl_async` has no case: `gethostbyname_impl_with_async` returns a
`std::string` and the hybrid exports no symbol for it (it is crates/hooks `resolve.rs`'s
`gethostbyname_with_async`, which `System::gethostbyname` runs on a cache miss without c-ares).

`core-tests/src/coroutine/socket.cpp` is Rust: `system/src/socket.rs` holds its 54
TESTs, one function each (`coroutine_socket.<name>`, upstream's names, inputs, ports
and expected values), run by `hooks-system-rust`. A `Socket` is the hooks crate's
`Co`, whose methods the cases call: made by `co_socket::make_quiet` and deleted
by dropping its `OwnedCo`, the vectored members over an `IOVector` owner
`swoole_rs_corebase::reactor::ReactorIov::new` makes, and its fields
through the record layout (`swoole_rs_co_socket.h`). `enable_ssl_encrypt` and the
`set_ssl_*` setters write the `SSLContext` owner (`owner_new`,
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
user resolver); lookups go through `resolve::name_resolver_lookup`. The DNS
requests drive the resolver record (`swr_v1_core_resolver_*`) as
`network::gethostbyname`/`getaddrinfo` do, and `Address` is crates/clients'
`address::assign`, `url` and `address_string` with the `swr_v1_clients_address_*`
exports. Excluded as in the
GoogleTest run: `dns.lookup1`, `dns.lookup_ipv6` and `dns.bad_family` (external
DNS and network) and `dns.cancel` (a lookup that misses the resolver cache).
`address.record` is the former repo-owned `address_record.cc` check, with the
same assertions: the record's fields and value copies, and the per-thread scratch
`get_addr` and `verify_ip` share.
The repo-owned C++ witnesses of copies and `std::bad_alloc` containment
(`rust_dns_request.*`, formerly `dns_request_owner.cpp`) checked the resolver
record's C++ views, `GethostbynameRequest` and `GetaddrinfoRequest`. Those left
`swoole_async.h` with the exports under them, so what the witnesses checked of the
record is `tests/modules/corebase/resolver.rs` (tests/modules/corebase/README.md);
`rust_dns_request.actual_async_numeric_results` stays here. `rust_socket_scopes.protocol_snapshot_and_copy_restore`
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
The file's other 13 TESTs are not yet ported (#3045).

Its other TESTs are `crates/hooks/src/uring_socket/ported.rs`, one test per TEST with the
same inputs and expected values, run by `cargo test --all-features` beside `uring::ported`
and over the same test host. Each `UringSocket` is a `UringSocketOps` over a test socket,
and its errCode is the last error it set. The second coroutine is a thread with its own ring.
`bind`, `listen` and the socket itself are `coroutine::Socket`'s, so they are the system
calls, on an ephemeral port in place of `__LINE__ + TEST_PORT`. TLS sessions use the
anonymous cipher above, and the jpg the cases send is that many generated bytes.

| Upstream TEST | Rust test or reason |
| --- | --- |
| `uring_socket.connect` | `connect`, `#[ignore]`d: it needs external DNS and network |
| `uring_socket.ssl_connect` | `ssl_connect`, `#[ignore]`d as `connect`. `check_liveness` is a non-blocking `MSG_PEEK` on the descriptor |
| `uring_socket.accept` | `accept` |
| `uring_socket.ssl_accept` | `ssl_accept`: the server's certificate, key, dhparams and curve are the anonymous cipher |
| `uring_socket.length_3` | `coroutine_socket.length_3` (`hooks-system-rust`): `recv_packet`'s length check is the same code over the ring's recv, and only a full coroutine runtime with the ring links it |
| `uring_socket.sendmsg_and_recvmsg` | `sendmsg_and_recvmsg` |
| `uring_socket.sendto_recvfrom_udp` | `sendto_recvfrom_udp` (`SW_SOCK_UDP`) and `sendto_recvfrom_udp6` (`SW_SOCK_UDP6`), `#[ignore]`d: the test containers have no IPv6, as for `coroutine_socket.tcp6` |
| `uring_socket.writev_and_readv` | `writev_and_readv` |
| `uring_socket.writevall_and_readvall` | `writevall_and_readvall` |
| `uring_socket.sendfile` | `sendfile_`: length 0 is the file's size, which `UringSocket::sendfile`'s caller resolves before the transfer |
| `uring_socket.send_and_recv_all` | `send_and_recv_all` |
| `uring_socket.poll` | `poll_` |
| `uring_socket.ssl_readv` | `ssl_readv`, `#[ignore]`d as `connect` |

`core-tests/src/coroutine/iouring.cpp` is `crates/hooks/src/uring/ported.rs`, one test per
TEST (upstream's names, inputs and expected values), run by `cargo test --all-features`
(the `rust` lane, Linux). They call the engine (`crate::uring`) over its test host, which
drives the ring where a coroutine would suspend. A second coroutine becomes a thread
with its own ring, and the cases take turns as GoogleTest's did, since they share /tmp
paths and `wait` collects any child. `Iouring::open` and `close` are `open_file` and
`close_fd`. The ring filesystem APIs return `io::Result`: open carries the
descriptor and status-only calls carry `()`. The syscall wrappers and original
test adapters restore the integer protocol; failed stat calls leave output intact.
Ring process waits, descriptor polls and futex operations also return `io::Result`,
retaining their PID/count or errno. Reactor/syscall callbacks and original test
adapters restore the integer protocol; a failed wait leaves its status word intact.
Completion events and requests retain `io::Result` across coroutine resumption;
errno is still published before resuming, including ECANCELED to ETIMEDOUT.
Ring scalar/message/vector byte transfers and accept/connect return io::Result,
retaining zero-byte EOF and the captured error. TLS BIO loops consume those Results;
the remaining uring_socket integer adapters and timer/sendfile are pending #4089.
`System::sleep` is `uring::sleep`, as it is with io_uring.
`sleep(tv_sec, tv_nsec)` is `sleep_timespec`, which `sleep(seconds)` converts to.
Three calls have left the build, and only core-tests called them at the pin.
`Iouring::socket` is gone, so the cases make their sockets with socket(2).
`Iouring::wait(stat_loc, timeout)` is gone too; the cases call its body,
`waitpid(-1, stat_loc, 0, timeout)`. `list_all_opcode` is the third. Upstream's `bind`
and `listen` were already bind(2) and listen(2). `sendfile` sends a file of the pin's
`examples/test.jpg` size (218787 bytes), which the crate's tests cannot read. It reads
once the send has returned: the single read upstream asserts on needs the whole file
to be in the socket's buffer. `wait_timeout` reaps its killed child, because `wait_success`
may run after it.

| Upstream TEST | Rust test (`uring::ported::`) or reason |
| --- | --- |
| `iouring.create` | `create` |
| `iouring.list_all_opcode` | none: `list_all_opcode` is not in the build (only core-tests called it) |
| `iouring.open_and_close` | `open_and_close` |
| `iouring.mkdir_and_rmdir` | `mkdir_and_rmdir` |
| `iouring.write_and_read` | `write_and_read` |
| `iouring.rename` | `rename` |
| `iouring.fstat_and_stat` | `fstat_and_stat` |
| `iouring.fsync_and_fdatasync` | `fsync_and_fdatasync` |
| `iouring.ftruncate` | `ftruncate` |
| `iouring.connect` | `connect` (ignored: external DNS and network, www.gov.cn) |
| `iouring.send_recv` | `send_recv` (ignored: external DNS and network, www.gov.cn) |
| `iouring.sendfile` | `sendfile` |
| `iouring.accept` | `accept` |
| `iouring.sleep` | `sleep` |
| `iouring.wait_success` | `wait_success` |
| `iouring.wait_timeout` | `wait_timeout` |
| `iouring.waitpid` | `waitpid` |
| `iouring.poll` | `poll`, except `poll(fds, 2, 1000)` failing with `SW_ERROR_INVALID_PARAMS`: the engine's `poll` takes one pollfd, and the hooked `poll` (like upstream's hook) sends only `nfds == 1` to it |
| `iouring.accept_timeout` | `accept_timeout` |

Upstream PHPTs for this module run in the merge tier's PHPT gate (the `phpt-*` jobs), which runs every pinned PHPT on upstream and the Cargo build. To attribute a gate regression to this module, see *Attributing a regression* in [corpus/upstream/README.md](../../../corpus/upstream/README.md).

## PHP gates

`select_handler_arrays.py` (`hooks-select-handler-arrays`, Cargo build, every
platform) runs `select_handler_arrays.php`: a warning's handler replaces `$read`
during the hooked `stream_select`, with `[]` or with an array another variable
holds. The call returns, and the other variable keeps its elements.

## Known failures

- `hooks-system-rust`: 18 of system.cpp's 20 cases pass; the two `getaddrinfo`
  network cases are excluded. socket.cpp: 49 of 54 pass (Linux aarch64, docker).
  `coroutine_hook.sendmsg_and_recvmsg` is excluded:
  it failed in ~15% of GoogleTest runs on both engines, depending on which close
  completes first. hook.cpp's other 25 cases pass (Linux aarch64,
  docker); `gethostbyname`, `getaddrinfo` and `socket` need external DNS and
  network.

`core-tests/src/os/timer.cpp` and `protocol/ssl.cpp` are Rust: `system/src/timer.rs`
and `ssl.rs`, one function per TEST (`timer.<name>`, `ssl.<name>`), run by
`hooks-system-rust`; the `timer-native-originals` and `tls-native-originals` lanes,
the last two that compiled core-tests files on their own, are gone. The timer cases
call `corebase::timer`'s Rust functions with owned closures and issued timer IDs; `SwooleTG.timer` is crates/corebase's `thread_timer`. Upstream's shared-memory
counters become `Cell`s (the callbacks run in the case's process), and its
`uint64_t` elapsed-time subtractions wrap as they do in C++. The ssl cases call
corebase::tls's `swoole_ssl_init`, `swr_v1_tls_destroy`, `swr_v1_tls_error` and
`ssl_context_create` (`SSLContext::create`) by path, and build the context through `owner_new` and its string slots' `set_shared`;
`ERR_put_error` is network_socket.rs's.

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
`sw_mem_pool()` memory (corebase::memory's `members::pool::alloc`, by path), so workers'
increments reach the master. `tcp_raw` calls the
raw-bytes `swr_v1_process_pool_ipc_send_sync` (by path) and `_listener`, as core-tests' adapted
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
Rust `system_waitpid` and `system_wait` with the original infinite timeout, and
`System::waitpid_safe` through its live native export; their
children `_exit(0)` where upstream's `exit(0)`, so a forked child skips the parent's
exit handlers. The async cases call `async::dispatch` and read `sw_async_threads()`'s
counts through `async_threads::stats`. They set `SwooleG`'s `aio_*` fields through
crates/corebase's setters, by path. `async.misc`'s `sw_async_threads()->notify_one()` has
no exported counterpart. It wakes a worker with nothing queued, so nothing in the TEST
observes it, and the case leaves it out. `os.daemon` discards its child's status, as
upstream's does, so the child's assertions don't decide the case: its
`isatty(STDIN_FILENO)` needs a terminal on stdin. `async.thread_starts_with_signals_blocked`
is crates/corebase/src/ossupport's `ossupport::pool::tests::thread_starts_with_signals_blocked`. It uses a new
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
| `async.thread_starts_with_signals_blocked` | `ossupport::pool::tests::thread_starts_with_signals_blocked` (crates/corebase/src/ossupport) |
| `os.daemon` | `os.daemon` |

`core-tests/src/os/signal.cpp` is Rust in `system/src/signal.rs`, run by
`hooks-system-rust`. The cases call `corebase::reactor`'s `signal_upstream` (`set`,
`_callback`, `_get_handler`, `_get_listener_num`, `_clear`) and corebase::process's `signal`
(the sigaction form of `swoole_signal_set`, `_dispatch`, `_isset`,
`_unblock_all`, `_to_str`), corebase's `signal_block_all` and
`signalfd::swoole_signalfd_init` by path. Each case runs in
its own process, as the rest of the package does, so no handler outlives its case.
`block` and `unblock` fork through `spawn_exec`, whose child `_exit(0)`s where upstream's
`exit(0)`s; the SIGIO handler still `exit(255)`s. `signalfd` is `#ifdef HAVE_SIGNALFD`
upstream and Linux-only here. `os_signal.restore_previous_mask` is crates/corebase's
`signal_mask::tests::restore_previous_mask`, in a new thread rather than a forked child: the
mask and the flag are per thread.

| Upstream TEST | Rust case |
| --- | --- |
| `os_signal.signalfd` | `os_signal.signalfd` |
| `os_signal.block` | `os_signal.block` |
| `os_signal.unblock` | `os_signal.unblock` |
| `os_signal.restore_previous_mask` | `signal_mask::tests::restore_previous_mask` (crates/corebase) |
| `os_signal.signal_to_str` | `os_signal.signal_to_str` |
| `os_signal.set` | `os_signal.set` |
| `os_signal.dispatch` | `os_signal.dispatch` |
| `os_signal.nested` | `os_signal.nested` |
| `os_signal.error` | `os_signal.error` |

The ported case runner records the executable SHA-256 in each report's `binary`
field. It runs an independent `case-binary` copy in the report's artifacts directory,
retaining it when a case or inventory check fails and deleting it after a passing
suite. CI uploads retained copies with the reports. To investigate, use that copy
with the same Linux dependencies and the suite's fixture working directory; a
subsequent Cargo rebuild does not replace it.

Ring timer/sleep and sendfile outcomes use `io::Result`. Sleep retains the pinned
ambient-errno `ETIME` rule even when ring creation fails. Sendfile retains the
first failure, completed output on a later error/EOF, short-output stopping and
explicit offset advancement. Scripted splice tests cover those branches without
a ring; original runtime tests still require successful io_uring setup.

The ring socket accept helper keeps `io::Result` through socket error publication
and descriptor adoption. TLS sendfile reports an optional `TlsWarning` alongside
completed bytes; the original scripted cases retain read/write warnings, retry,
EOF, zero-send and partial-progress expectations. Ring sendfile completion compares
unsigned byte counts without a negative error sentinel.

The cURL multi-engine tests in `curl.rs` inject recording host operations into
`Multi`. Both builds compile the native reactor/coroutine and PHP adapters and
share the owned reactor socket implementation. Existing cases cover libcurl's
socket registration, event coalescing, waiterless removal, queued wake-up release,
handle reset and timer cancellation. A native socket ownership case also verifies
that freeing a watch twice leaves the libcurl-owned descriptor open.

`hooks-socket-cancel` (`socket_cancel.py` and `.php`) checks cancel and close during a resolver's second park and a
UDP receive, from both a coroutine and (`reactor` argument) a non-coroutine timer
callback. The normal gates and io_uring, c-ares and thread-context variants run it; the latter must complete the
canceled request before the operation releases its buffers.

The network socket fixture constructs the actual `SocketRecord` and calls its
Rust operations through typed fields and socket handles. Its C++ status adapters
keep the original integer assertions. Twenty member exports used only by Rust
fixtures are removed, including the eleven-method export block; native socket
factory/free and TLS exports with other callers remain.

The original `timer.get` and `timer.delay` use the issued timer ID returned
by `swoole_timer_get`, and `delay_held` moves its deadline. Their ID, elapsed-time
and missing-timer expectations are unchanged; `None` represents the original
null node. The test-only `swoole_timer_delay` C++ export is removed.

The remaining timer cases and their process-pool, async and OS consumers schedule
owned Rust captures and cancel issued `Held` IDs. The timer destructor case runs
when the final callback owner drops; fixture handles retain only a weak reference.
The original inputs, counters, timing bounds, error 505, zero-interval failures,
null operations and reinitialization assertions are unchanged. With the HTTP
fixtures migrated too, the raw add/after/tick/delete exports and copied node layout
are gone; `TimerNode` is private to the timer crate.
