# Migration approach

The upstream Swoole extension keeps its entry point, PHP object model, worker
lifecycle, scheduler and hooks. Bounded subsystems are replaced one at a time
by Rust code linked as a static library and called through a small C ABI.

Swoole's internals are C++ classes, not a C API, so the seam uses hand-written
`extern "C"` wrappers rather than bindgen over C++ headers. At every boundary:

- Pass opaque handles, byte pointers with lengths and integer error codes.
- Allocate and release on the same side. Say who owns each buffer and whether a
  callee may retain it.
- No Rust panic or C++ exception crosses the ABI. Zend calls and PHP
  exception/bailout handling stay in the PHP-facing C++ layer.
- PHP values stay on their owning thread. Upstream owns the reactor and
  scheduling; no Tokio runtime drives Swoole descriptors.
- Build and test only the all-modules hybrid; the pinned upstream build is the
  oracle. Delete an upstream body once its replacement passes the corpus,
  upstream tests and benchmarks.

Mature third-party libraries (HTTP parsers, TLS, compression, database
clients) are not rewritten unless doing so removes a dependency or fixes a
demonstrated problem.

See [migration modules](migration-modules.md), [ABI v1](abi-v1.md) and
[hybrid build](hybrid-build.md).

## Graduation

A module graduates when:

1. Every merge-tier phase tagged with it passes on two consecutive completed
   merge-tier runs.
2. Its original PHPTs pass wherever they pass on upstream, as far as the PHPT
   manifest covers it.
3. Its callgrind cost on the corpus workloads that exercise it is measured and
   posted, and graduating it keeps the merge tier's ratchet (see Speed). Module
   micro-benchmarks and timings are reported but don't gate.
4. It has no open `p0`/`p1` `kind:bug` issues.

Every Rust build applies every module's patches, sets its feature bit and links
its crate; no module has a configure switch, a `SWOOLE_RS_<MODULE>` guard or a
cargo feature. A graduated module is listed in `GRADUATED` in
`tools/build_hybrid.py`, and `dev.py` runs its gates on every run. Reference builds (`tools/build_upstream.py`,
`build_hybrid.py --backend upstream`) stay pristine: they are the oracle that
differential tests compare against, not a fallback.

Graduated: base64, buffer, corebase, crc32, error, framing, http, protocols, websocket, timer.

## Speed

The merge tier's `callgrind` job is a ratchet, not a target:
[`benchmarks/callgrind_limits.json`](../benchmarks/callgrind_limits.json)
records the all-modules hybrid/upstream instruction ratio per workload, and the
job fails if one rises more than 1% (noise) above its record.

- A PR that makes a workload cheaper lowers its record in the same PR.
- A PR that moves work into Rust may raise a record by up to 3% of its value,
  stating the before/after numbers in the PR. Migration may be slow before it
  is fast; the ratchet keeps that visible and bounded.
- A record may also rise when an A/B of the same base with and without the
  change shows the extra instructions outside the code it changed (code layout,
  allocator): the PR states the A/B (#686).
- Speed work keeps or grows Rust's share. The default fix moves the whole
  operation, decision and effect, into Rust (#366). Handing code back to
  upstream is only for glue in which Rust decided nothing, and the PR says so.
- Within 5% of upstream is the target for Phase 6, when Rust owns the event
  loop and the HTTP path end to end.

## Roadmap

Progress is measured by the upstream first-party lines (`src/`, `ext-src/`,
`include/`, excluding `thirdparty/` and the generated embedded library) that the
all-modules patch set deletes (`tools/badges.py`). At the start of the roadmap
(2026-10-25) about 77k of 89k were still compiled in; at `b5f2efe` (2026-11-06)
16.1k of 90k remain (82.1% replaced). The migration is done when nothing remains
apart from declared third-party code.

1. **Green baseline** ([#20](https://github.com/loks0n/swoole-rs/issues/20), open).
   A build with every module passes the merge tier on `main`: corpus, selected
   PHPTs and benchmarks against upstream. Last green run:
   [`3cd1411`](https://github.com/loks0n/swoole-rs/actions/runs/40701810400) (2026-10-28).
2. **Wider oracle** ([#58](https://github.com/loks0n/swoole-rs/issues/58), done).
3. **Graduate and delete** ([#125](https://github.com/loks0n/swoole-rs/issues/125), done).
   The nine leaf modules and corebase are Rust only; the per-module switches are
   gone (#1312).
4. **Deepen thin modules** ([#439](https://github.com/loks0n/swoole-rs/issues/439), open).
   Process, hooks, coroutine, reactor and clients move from Rust policy over
   C++ mechanisms to Rust mechanisms.
5. **Zend surface and host** ([#754](https://github.com/loks0n/swoole-rs/issues/754),
   open; the old phases 5 and 6). PHP classes, module startup and class
   registration move into Rust over `crates/zend`. Leaf classes, Process/Thread
   and the optional integrations are done; the Server family, Client family
   and the host remain. The pinned tree stops being a build input when this ends.
6. **Speed** ([#366](https://github.com/loks0n/swoole-rs/issues/366), open). The
   all-modules hybrid is within 5% of upstream on every callgrind workload;
   `benchmarks/callgrind_limits.json` records where it stands.

### Upstream files still in the build

Kept upstream lines per file at `b5f2efe`, from `tools/badges.py --source
target/upstream/src --output target/badges --kept`. Headers count while any
translation unit includes them; database, curl, ssh2, thread and io_uring files
compile only under their configure switch.

- `include/` (6,599): swoole_server.h 1170, swoole.h 614, swoole_coroutine_socket.h 337, swoole_client.h 253, swoole_coroutine.h 227, swoole_http.h 208, swoole_http2.h 208, swoole_string.h 190, swoole_websocket.h 187, swoole_util.h 185, swoole_protocol.h 146, swoole_log.h 143, swoole_message_bus.h 130, swoole_timer.h 127, swoole_coroutine_api.h 124, swoole_lock.h 122, swoole_atomic.h 118, swoole_ssl.h 108, swoole_async.h 104, swoole_coroutine_context.h 101, swoole_lru_cache.h 100, swoole_table.h 95, swoole_coroutine_channel.h 94, swoole_memory.h 88, swoole_coroutine_system.h 86, swoole_proxy.h 73, swoole_uring_socket.h 73, swoole_signal.h 70, swoole_iouring.h 67, swoole_asm_context.h 65, swoole_pipe.h 65, swoole_config.h 62, swoole_mqtt.h 62, swoole_api.h 57, swoole_redis.h 57, swoole_heap.h 54, swoole_llhttp.h 54, swoole_dtls.h 53, swoole_socket_hook.h 53, swoole_static_handler.h 53, swoole_file_hook.h 52, swoole_file.h 51, swoole_msg_queue.h 48, swoole_channel.h 45, swoole_thread.h 41, swoole_socket_impl.h 33, swoole_version.h 33, swoole_mime_type.h 31, swoole_base64.h 29, swoole_error.h 28, swoole_hash.h 25
- `ext-src/` (5,272): swoole_server.cc 541, php_swoole_private.h 485, php_swoole_cxx.h 215, swoole_socket_coro.cc 179, swoole_thread.cc 163, swoole_oracle.cc 161, swoole_http_response.cc 158, php_swoole_http.h 149, swoole_http_client_coro.cc 147, swoole_http_server_coro.cc 146, swoole_client.cc 138, swoole_http_server.cc 137, swoole_sqlite.cc 132, php_swoole_odbc.h 127, swoole_http2_server.cc 122, swoole_websocket_server.cc 113, swoole_coroutine.cc 106, php_swoole_coroutine.h 104, swoole_odbc.cc 99, php_swoole_firebird.h 90, php_swoole_curl.h 84, swoole_http_request.cc 79, swoole_event.cc 76, swoole_pgsql.cc 75, php_swoole_pgsql.h 71, swoole_admin_server.cc 70, php_swoole_oracle.h 67, swoole_curl.cc 67, swoole_curl_interface.h 61, swoole_timer.cc 61, php_swoole_http_server.h 57, php_swoole_ssh2.h 57, php_swoole_ftp_def.h 53, php_swoole_sqlite.h 52, swoole_firebird.cc 50, php_swoole_websocket.h 49, php_swoole_thread.h 48, php_swoole_stdext.h 45, php_swoole_ssh2_def.h 43, swoole_name_resolver.cc 42, swoole_coroutine_system.cc 39, swoole_stdext.cc 38, php_swoole_cxx.cc 37, php_swoole_client.h 35, swoole_lock.cc 32, swoole_tracer.cc 32, php_swoole_call_stack.h 29, swoole_channel_coro.cc 29, swoole_table.cc 27, swoole_thread_lock.cc 26, swoole_atomic.cc 25, swoole_thread_barrier.cc 25, swoole_thread_queue.cc 25, swoole_thread_arraylist.cc 24, swoole_thread_map.cc 24, swoole_thread_atomic.cc 23, swoole_client_async.cc 22, php_swoole_coroutine_system.h 21, swoole_coroutine_lock.cc 21, swoole_http_cookie.cc 19
- `src/coroutine/` (911): system.cc 231, base.cc 173, uring_socket.cc 134, iouring.cc 121, thread_context.cc 105, context.cc 98, channel.cc 49
- `src/protocol/` (789): http.cc 140, dtls.cc 124, websocket.cc 119, mime_type.cc 69, ssl.cc 69, redis.cc 67, message_bus.cc 65, socks5.cc 50, base.cc 43, mqtt.cc 43
- `src/server/` (779): master.cc 494, static_handler.cc 148, process.cc 137
- `src/network/` (493): dns.cc 158, stream.cc 146, client.cc 127, address.cc 62
- `src/wrapper/` (344): timer.cc 132, http.cc 111, event.cc 101
- `src/core/` (292): timer.cc 90, misc.cc 75, error.cc 46, heap.cc 31, crc32.cc 24, log.cc 20, base64.cc 6
- `src/memory/` (268): fixed_pool.cc 70, shared_memory.cc 67, table.cc 49, global_memory.cc 45, ring_buffer.cc 37
- `src/lock/` (155): barrier.cc 33, coroutine_lock.cc 32, rw_lock.cc 31, mutex.cc 30, spin_lock.cc 29
- `include/helper/` (125): kqueue.h 125
- `src/os/` (120): async_thread.cc 120

References: [Rust FFI](https://doc.rust-lang.org/nomicon/ffi.html),
[upstream CMake build](https://github.com/swoole/swoole-src/blob/v6.2.4/CMakeLists.txt),
[upstream config.m4](https://github.com/swoole/swoole-src/blob/v6.2.4/config.m4).
