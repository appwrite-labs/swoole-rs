# HTTP tests

The HTTP module is `crates/http`; its exports' contracts are their doc comments.

Rust unit tests and the archive the native gates link against:

```sh
cargo test --locked -p swoole-rs-http
cargo clippy --locked -p swoole-rs-http --all-targets -- -D warnings
cargo build --locked -p swoole-rs-ffi --release
```

## Native differential gates

Each gate compiles pinned (v6.2.5) bodies against the Cargo build SDK `--archive`
comes from and compares outcomes. The two sides compile apart (`Sides` in
`gate.py`): upstream fragments against the pin's headers only, the rest against
the SDK's headers and switches (`native_flags`). Native code runs under ASan/UBSan;
the Rust archive is uninstrumented unless `--rust-sanitize` is used (Linux;
links a private ASan HTTP archive instead, precompiled std stays uninstrumented).
All take `--source PINNED_GIT --archive HYBRID_ARCHIVE --output REPORT`; gates
marked `[R]` also take `--rust-sanitize`. `tools/dev.py` registers each as an
`http-*` phase.

| Script | Checks |
| --- | --- |
| `rewrite_gate.py` [R] | Pinned `add_rewrite_rule`/`apply_rewrite_rules` (std::regex) against the server's Rust rules (`swr_v1_server_static_rewrite`) |
| `locale_gate.py` | Case-insensitive helpers against libc `strncasecmp` across available locales |
| `server_config_owners.py` | The Server's Rust collection views and shared context lifetime, not a differential (`http-server-config-owners`) |

Locale semantic runs must be unsanitized: under ASan the macOS interceptor
compares `C0`/`E0` with ASCII rules where libc in `en_US.ISO8859-1` treats them
as equal. Unavailable locales are reported as unsupported.

`gate.py` holds the helpers these share (`body`, `Sides`, `run`). The status, method,
URL and boundary helpers are Rust behind thin facades, so crates/http-ffi's
`pin_helpers` tests compare them with v6.2.4's bodies instead.

### Ported GoogleTests

Every TEST of `core-tests/src/server/http_server.cpp` is Rust, below, and the file is
no longer built (#1881).

`http_parser.cpp` and `multipart_parser.cpp` tested only the bundled third-party
parsers, so they left the lane (#1756) and with them `include/swoole_llhttp.h`.
Their ports, with the same inputs, callbacks and expected values and one Rust test per
TEST under the TEST's own name, drive crates/http's Rust parsers, which replaced
`thirdparty/llhttp` and `thirdparty/multipart_parser.c` (#2658; `cargo test -p
swoole-rs-http --test http_parser --test multipart_parser`).
`llhttp_differential.rs` runs generated, cut and paused inputs through both the
Rust llhttp and crates/oracle's unmodified C llhttp 9.3 and requires every result,
error position, reason, field and callback to match (`CASES` and `SEED` widen it).

| Upstream file | Rust test file | TESTs |
| --- | --- | --- |
| `http_parser.cpp` | `http_parser.rs` | `get_request`, `version`, `incomplete`, `method`, `websocket`, `http2`, `header_field_and_value`, `query_string`, `chunk`, `response`, `request_error_case` (54 protocols and reasons), `response_error_case` (22), `request_success_case` (72), `response_success_case` (45) |
| `multipart_parser.cpp` | `multipart_parser.rs` | `error_message`, `header_field`, `header_error`, `data` (random part data from upstream's `base64_encode`, as `append_random_bytes(n, true)`) |

`swoole_llhttp_parser_execute` is reproduced in `http_parser.rs` (`Http::execute`).

Four `http_server` TESTs drove a request through the `http_server::listen` SDK
(`src/wrapper/http.cc`), which nothing else used; the SDK left the build (#1672) and
the TESTs left `http_server.cpp`. Their Rust ports, in `http_server.rs` (included by
`crates/ports/src/read.rs`'s tests: `cargo test -p swoole-rs-ports http_server`), send
the same request through the server's HTTP reader under the same port settings, with
the request's effects running the multipart parser and real temp files. The
recorded requests come from `crates/oracle/upstream/core-tests/fuzz/cases`.

| Upstream TEST | Rust test | Same input | Assertions |
| --- | --- | --- | --- |
| `parser1` | `parser1` | `req1.bin`, default `package_max_length` | `form_data.size() == 3`; `200 OK` as no error response and the request dispatched whole |
| `parser2` | `parser2` | `req2.bin`, `package_max_length` 64 KiB, `upload_max_filesize` 1 MiB | `form_data.size() == 3`; `200 OK` as above; also that the upload streamed to disk is `test.jpg` |
| `upload` | `upload` | curl's chunked `-F file=@test.jpg` to `/upload?test=curl`, 2 MiB | `files.size() == 1`, `request_path` `/upload`, `query_string` `test=curl`; the server sends nothing itself |
| `max_request_size` | `max_request_size` | curl's `-F file=@test.jpg`, 128 KiB | `413 Request Entity Too Large`, the request never dispatched |
| `ignore_http1_request_trailers` (v6.2.5's `http_request_trailer.cpp`) | `ignore_http1_request_trailers` | its chunked POST with a `Content-Type`/`Cookie`/`X-Trailer` trailer section | `200 OK` as above, the request (trailers too) dispatched whole; `Content-Type` `text/plain`, `Cookie` `initial=yes`, no `X-Trailer` |

The PHP request parser's handling of the same trailers (`crates/phphttp`'s llhttp
callbacks skip fields under `F_TRAILING`) is checked by the pinned PHPTs
`swoole_http_server/request_trailer.phpt` and `create_request_trailer.phpt`.

The 11 TESTs that start no server call the helpers, the request-line reader and the
WebSocket codec directly. Their ports sit beside the Rust each one calls, with the
same inputs and expected values: `cargo test -p swoole-rs-http -p swoole-rs-http-ffi -p swoole-rs-websocket-ffi --lib`.

| Upstream TEST | Rust test |
| --- | --- |
| `EncodeDecodeBasic`, `EncodeDecodeWithSpecialChars` | `crates/http-ffi/src/http_server.rs`: `encode_decode_basic`, `encode_decode_with_special_chars` |
| `get_method`, `get_method_str`, `get_status_message`, `parse_multipart_boundary` | `crates/http-ffi/src/http_server.rs`: `get_method_cases`, `get_method_str`, `get_status_message_cases`, `parse_multipart_boundary_cases` |
| `has_expect_header`, `get_protocol`, `all_method` | `crates/http-ffi/src/request.rs`: same names |
| `websocket_encode`, `websocket_compressed_control_frame` | `crates/websocket-ffi/src/frame_api.rs`: same names (`print_frame` to a temp file instead of `/tmp/swoole.log`) |
| `http_upload_provenance.cpp`: `reject_client_upload_marker`, `header_name_split_across_reads_is_assembled`, `overwritten_read_buffer_cannot_forge_upload_marker` | `crates/http-ffi/src/request.rs`: same names, over the multipart parser (#1938) |

The TESTs that start a server run in `server/` (the merge tier's `http-server-rust`
phase): a Rust case binary over the crates and system.py's static libswoole.a (the pin's thirdparty and native/'s Zend-free sources), built and run by
`tests/modules/hooks/system.py --package tests/modules/http/server`. `server/src/http1/case.rs`
builds the Servers upstream's `test_base_server`, `test_http_server` and `test_ssl_http`
built, over the same records as `http1.rs` (below), with each callback slot holding the
case's function as its closure context; the callbacks, cpp-httplib's WebSocket client
(the same upgrade request, frames from the hybrid's `websocket::encode`/`mask`/`decode`),
the curl runs and every assertion are in `server/src/main.rs`, one function per TEST
under its own name: `websocket_small`, `websocket_medium`, `websocket_big`,
`websocket_mask`, `node_websocket_client_1`, `node_websocket_client_2`, `sni`,
`client_ca`, `ssl`, `abort_connection`, `get_package_length`. The two node TESTs stay
excluded for the reason they had in the GoogleTest lane (they need Node.js and its ws
package). Each curl runs to completion before its log is read, where upstream slept a
second after starting it.

`abort_connection`'s Server builder also sets `SwooleG.max_sockets` (2) before
`set_max_connection(1)` and hands back the old value, which the case restores once the
server stops; the 16 `SyncClient`s are blocking `TcpStream` connects. In
`get_package_length` `case.rs` builds the port, runs `init_protocol` and makes the
fake connection socket and its `Connection` (session 2025); the case calls the hybrid's
`http_server::get_package_length(_size)` and `websocket::encode` itself, with upstream's
frames and expected values; its HTTP/2 frame is the bytes `http2::init_settings` and
`http2::pack_setting_frame` write, as a constant (crates/http2's tests cover that function).

The HTTP/1 TESTs that start a server are `server/src/http1.rs`, in the same binary and
phase: `swr_v1_server_new`/`_delete` (crates/server/src/server_object.rs)
make and delete the `Server`, and the cases set its configuration, callback slots,
tables, port configuration and static-handler sets through the Rust records
whose pointers head the `Server` and its `ListenPort`s, whose layouts each new server checks against
`http1.rs`'s copies (`swr_v1_server_record_layout`). Upstream's fixtures, inputs and
expected values are kept; where they ran llhttp the handlers read the request line and
headers themselves. The client is a port of core-tests' cpp-httplib 0.7
(`server/src/client.rs`): a failed request is `None` where httplib returns `nullptr`.
An expectation in a server callback can't unwind through the server, so it is recorded
and the case fails on it once the server stops; in a process-mode worker it goes
unseen, as gtest's did.

| Fixture | TESTs |
| --- | --- |
| `test_base_server` (base mode, client in a thread) | `get`, `reset_connection`, `heartbeat_check_interval`, `idle_time`, `post`, `static_get`, `static_files`, `not_modify` |
| `test_proxy_server` (forked) | `proxy_file`, `proxy_response` |
| `test_http_server` (forked, or base mode for `chunked`) | `heartbeat`, `overflow`, `process`, `process1`, `redundant_callback`, `pause`, `bad_request`, `chunked`, `max_queued_bytes`, `dispatch_func_return_error_worker_id`, `http_range`, `http_range2` |
| none | `fail` |

`core-tests/src/server/buffer.cpp`'s one TEST, `server.send_buffer` (crates/server's send
buffer: a 16 MiB response to one packet, then a close), is `server/src/http1/server.rs`
in the same binary and phase, over the same records. Upstream's client thread waits on a
mutex its `onWorkerStart` unlocks; here `onWorkerStart` starts it, and its `SyncClient` is
a blocking `TcpStream`.

`core-tests/src/server/server.cpp`'s TESTs go in the same binary and phase, suite `server`,
one function per TEST under its own name (#2533). Its `schedule*` TESTs are
`server/src/http1/schedule.rs`: each creates a process-mode Server and calls crates/server's
`schedule_worker` under one dispatch mode, with the workers' status, `coroutine_num` or
`concurrency` and the fake connections' `uid` and `worker_id` set as upstream sets them.
Upstream's stack `network::Socket fake_sock{}` is `make_socket(fd)`, its `info` assigned by
the `Address::assign` body (`swr_v1_clients_address_assign`).

| Upstream TEST | Dispatch mode |
| --- | --- |
| `schedule` | `DISPATCH_IDLE_WORKER`, all busy then four idle |
| `schedule_1` | `DISPATCH_ROUND` |
| `schedule_4` | `DISPATCH_IPMOD`: IPv4 and IPv6 connections and a UDP packet in `sw_tg_buffer()` |
| `schedule_5` | `DISPATCH_UIDMOD` |
| `schedule_8` | `DISPATCH_CO_CONN_LB` |
| `schedule_9` | `DISPATCH_CO_REQ_LB` (`Worker::coroutine_num`) |
| `schedule_10` | `DISPATCH_CONCURRENT_LB` (`Worker::concurrency`) |

The TESTs that reload, kill and restart a started Server's workers are
`server/src/http1/lifecycle.rs`. Upstream's lambdas are `extern "C"` callbacks in the Rust
slots, and what they captured is a static. `test::counter_*` is a shared anonymous mapping,
made before the server forks, as core-tests' main makes it from `sw_mem_pool()`. One more
slot in it counts failed expectations, so one in a forked worker fails the case too,
where gtest's parent never saw it. Upstream's
reload, kill and restart methods are crates/process's `workers::entry` functions, and `add_worker`
is crates/server's.

| Upstream TEST | What it drives |
| --- | --- |
| `reload_all_workers`, `reload_all_workers2` | `SIGUSR2` then `SIGUSR1` to the manager (to the master in `2`) from worker 1; `onBeforeReload`/`onAfterReload` twice each |
| `kill_user_workers`, `force_kill_all_workers`, `kill_user_workers1` | stopping a server whose user, task or event workers never return, with `max_wait_time` 1 |
| `create_task_worker_fail` | `create()` refusing coroutine task workers over a message queue (`SW_ERROR_WRONG_OPERATION`) |
| `reload_single_process`, `reload_no_task_worker` | `reload(true)` in a single-process base server (`SW_ERROR_OPERATION_NOT_SUPPORT`), and `reload(false)` with no task workers |
| `restart_worker_process` | `restart_worker_process` over a task worker registered under this pid |
| `reload_all_from_worker_error`, `reload_tasks_from_worker_error` | a reload requested from `onWorkerError`, with upstream's five-second watchdog |
| `kill_worker_1`, `_2`, `_4`, `_5`, `worker_exit`, `shutdown_event` | `test_kill_worker` in base and process mode, with and without `reload_async`, `onWorkerExit`, and an `SW_SERVER_EVENT_SHUTDOWN` packet between workers |
| `kill_self` | `test_kill_self`: a worker killing the worker that holds the connection |
| `clean_worker_1` | `test_clean_worker(MODE_BASE)`: a worker's `onConnect` defers `clean_worker_connections`, shuts the client's socket down and stops itself, so no `onReceive`, `onClose` or client receive, and three worker starts. The `AsyncClient` is crates/clients' client, made before the server forks and connected from `onStart`'s timer, as upstream's |
| `base`, `process`, `process_single_thread` | `test_base` and `test_process`: one request through each mode, the methods that must refuse (`shutdown` before start, `add_port` and `add_worker` after `create`, an unknown session, an unknown command), and the start, shutdown and manager callbacks |

The lock upstream's client thread waits on (a process-shared `Mutex`) is a slot in the shared
counter mapping, since `onStart` and `onWorkerStart` may run in a forked process, and a pipe
in `test_process`, whose forked worker 0 unlocks it.
Three upstream assertions have no code left to test, so the cases leave them out:

- `base`'s `Server::HOOK_WORKER_START` hook (`is_started()`), and `process`'s
  `counter[2] >= 2`, which `manager_alarm`'s `Server::HOOK_MANAGER_TIMER` hook counted:
  `Server::add_hook` and `manager_alarm` left the build (#1535).
- `process`'s `call_command_handler_in_master(9999, ...)`: its body is now part of
  `Server::command`, which looks the command up first, so nothing can call it with an
  unknown id.

Not yet run:

- `thread`, `task_thread`, `reload_thread`, `reload_thread_2` and `reload_thread_3` are
  `#ifdef SW_THREAD` upstream, and `clean_worker_2`, `kill_worker_3` and `kill_worker_6` use
  `MODE_THREAD`. None is ported yet. The binary is a `SW_THREAD` build (see the MODE_THREAD
  TESTs below), so they can be.

`core-tests/src/server/server.cpp`'s task-worker TESTs are `server/src/http1/task.rs`, suite
`server`, in the same binary and phase, one function per TEST under its own name with the
same Servers, inputs and expected values: `task_worker`, `task_worker2`, `task_worker_3`,
`task_base`, `task_process`, `task_ipc_stream`, `task_worker3`, `task_worker4`,
`task_sync_multi_task`, `task_sync`, `task_ipc_queue_1` .. `task_ipc_queue_5` and
`no_idle_task_worker`. `Server::task` and both `task_sync`s are crates/process's `Tasks`
over the server's task pool and the worker's notify pipe, lock and result slot (`tasks`,
as crates/phpserver's `task_state` lends them to `Swoole\Server`'s task methods); the other methods are the crates' functions under their
upstream symbols, and the inline ones (`send_pipe_message(id, data, len)`,
`get_task_result`, `get_worker_pid`) keep their bodies. `test::counter_*` is a shared
mapping the forked processes inherit, as core-tests' memory-pool counters are, and an
expectation in a callback records its failure there, so here one in a forked worker fails
the case too. `no_idle_task_worker`'s client waits on a shared flag its `onWorkerStart`
sets, where upstream's waited on a process-shared mutex it unlocked. `task_ipc_queue_5`
runs in thread mode (see the MODE_THREAD TESTs below).

`core-tests/src/server/server.cpp`'s TLS TESTs that start a server are
`server/src/http1/server_ssl.rs`, in the same binary and phase, one function per TEST
under its own name: `ssl`, `ssl_error`, `ssl_write`, `dtls`, `dtls2`, `ssl_client_cert_1`
(base mode), `ssl_client_cert_2` (process) and `ssl_client_cert_3` (thread), with upstream's
modes, certificates, packets and expected values. `server.ssl_matches_wildcard_name`
starts no server and is a crates/ports unit test (tests/modules/ports/README.md).

- `SyncClient` is crates/clients' `network::Client` through its `swr_v1_clients_*` exports,
  with `SyncClient`'s `connected`/`created` bookkeeping; `get_client()->set_ssl_cert_file`
  writes the client's `SSLContext` slot as the setter does.
- The `Mutex(true)` the client thread waits on and `onWorkerStart` unlocks is a socket
  pair, and `test::counter_*`'s counter is `sw_shm_malloc` memory, as upstream's is: in
  process mode the worker is another process. `EXPECT`s are recorded in that memory too,
  so unlike the cases above a process-mode worker's failure fails the case.
- An `ASSERT` in the client thread fails the case and signals the master down, where
  upstream's returned from the thread and left the server running.
- `ssl_write`'s 4 MiB of random bytes (`append_random_bytes`) are read from `/dev/urandom`.
- `ssl_client_cert`'s `test::dump_cert_info` prints the certificate's subject and issuer
  entries and the subject's one-line form; its validity and public-key lines, which the
  `organizationName: swoole` search can't match, are left out.
- `dtls2` reaps its forked client, which must exit 0.

`core-tests/src/server/server.cpp`'s connection and configuration TESTs (#2536) are
`server/src/http1/server_config.rs`, in the same binary, phase and records, under the
`server` suite with upstream's names. `server.protocols` needs only its port, so it is
crates/ports' `methods::tests::server_protocols` (tests/modules/ports/README.md).

| Upstream TEST | Rust case | Notes |
| --- | --- | --- |
| `max_connection`, `min_connection`, `worker_num` | same names | `SwooleG.max_sockets` is read and set through the `SwooleG` symbol, whose first member it is |
| `reactor_num_base`, `reactor_num_large`, `reactor_num_large2`, `reactor_num_zero` | same names | `SW_CPU_NUM` is `swr_v1_core_cpu_num()`, which `swoole_init` sets |
| `startup_error` | same name | its `new` Server leaks, as upstream's |
| `protocols` | crates/ports `methods::tests::server_protocols` | |
| `sendwait`, `forward_message`, `pause_and_resume`, `max_queued_bytes` | same names (`server_conn.rs`) | the client thread starts before `serv.start()`, as upstream's |
| `system` | same name | its listening socket takes a port the kernel picks, where upstream's took `get_random_port()` |
| `no_idle_worker`, `overflow_1` .. `overflow_4`, `max_request_1`, `max_request_2`, `watermark`, `discard_data` | same names | the client thread starts in `onStart`, as upstream's |
| `udp_packet` | same name | the server socket `sendto` answers on is `req->info.server_fd`, which `get_server_socket` looks up; the forked client's assertions fail the case |
| `pipe_message`, `reopen_log` | same names | |
| `reopen_log2` | same name | `signal_handler_reopen_logger` has no entry of its own in the crates; `master_signal_handler(SIGWINCH)` runs its body |
| `command_1`, `command_2` | same names | `add_command` and `command` are crates/server's `swr_v1_server_add_command` and `swr_v1_server_command` |
| `abnormal_pipeline_data` | same name | `send_fn` writes its datagram to worker 0's `pipe_master` (from `view::Server::worker(0)`), retrying while the pipe is full, as `send_sync` does |
| `base_event_worker_pool_cleanup` | same name | the pool's `map_` (pid to worker) is its `registry` in crates/process |
| `abort_worker`, `reactor_thread_pipe_writable`, `send_timeout` | same names | `get_session(i)` reads the shared session list; `move_packet()` is `swoole_rs_server::bus_move_packet`; `conn->socket->recv_timer` is the socket record's, at the offset swoole_rs_socket.h asserts |
| `heartbeat_check_1` .. `heartbeat_check_4`, `close_1` .. `close_8`, `eof_check` | same names | the `AsyncClient` is a blocking client in a thread of the process whose reactor upstream's ran in, with the same callbacks in order (`connected`, `on_connect` or `on_error`, `on_receive` per read, `on_close`); its `flags` are shared counters, since the server's callbacks may run in a worker |

The MODE_THREAD TESTs (`max_request_2`, `heartbeat_check_3`, `close_3`, `close_7`,
`task_ipc_queue_5`, `ssl_client_cert_3`) need upstream's `enable_thread` build (`SW_THREAD`),
which core-tests always use. This binary is one too: it turns on crates/process's `thread`
feature, so each thread has its own `SwooleWG` (worker id, type and pid), as
`SW_THREAD_LOCAL` gives upstream. Its `main` sets `swoole_rs_server::THREAD_SUPPORTED`, as
the extension's MINIT does under ZTS. Without the feature, the master and worker threads
shared one record and overwrote each other's worker type (#2636).

`server_conn.rs` keeps core-tests' shared counters (`test::counter_*`) and the
process-shared `Mutex(true)` its clients wait on in anonymous shared memory mapped
before the server forks; the mutex is a flag a worker sets and the client takes. An
expectation in a server callback is recorded there too, so the case fails on it once
the server stops, from whichever process it failed in (gtest's went unseen in a forked
worker). `SyncClient` is a blocking `TcpStream`: `send` writes it all, `recv` is one read.

The SDK's own response writer (`Context::end`, whose `TEST_STR` body curl printed in
`upload`) went with it; responses on the production path are `Swoole\Http\Response`'s,
covered by the PHPTs.

## PHP checks

`test_smuggling.py` (`http-smuggling`) sends HTTP/1 framing ambiguities to a whole
`Swoole\Http\Server` (`smuggling.php`) on each engine, one byte stream per fresh
connection: conflicting and malformed `Content-Length`, `Transfer-Encoding` variants,
CL.TE/TE.CL/TE.TE desyncs, bare `\n` and `\r`, chunk-size, extension and trailer
edge cases, oversized and malformed headers and request lines, and requests
pipelined behind an error. Every response's status line, `Connection` header and
body (the handler echoes the method, URI, headers and body it received) and whether
the server closed the connection must equal the first engine's:

```sh
python3 -m pytest tests/modules/http/test_smuggling.py \
  --engine upstream=target/upstream/src/modules/swoole.so --engine cargo=target/ext/swoole.so
```

A connection is read until the server closes it or sends nothing for a second; the
cases run 32 at a time, so the phase takes about five seconds.

The merge tier's `api-surface` phase (`tools/capability_inventory.py
--require-same-api`) compares every Swoole class's reflected metadata,
including parameter and property defaults, against upstream.

### Original PHPTs

Upstream PHPTs for this module run in the merge tier's PHPT gate (the `phpt-*` jobs), which runs every pinned PHPT on upstream and the all-modules hybrid. To attribute a gate regression to this module, see *Attributing a regression* in [corpus/upstream/README.md](../../../corpus/upstream/README.md).

## Known failures

- `If_Modified_Since.phpt` fails on macOS for native and hybrid (200 instead of
  304), consistent with a libc calendar offset difference.
- Full originals also fail or are unsupported for reasons outside Rust: the
  runner's unsupported custom `EXPECTF_85` section, missing
  `ip` executable (`server_addr`), `bug_2444` (needs MySQL), `tmp-content-type`
  (needs Composer Guzzle), coroutine `close_socket` (needs external HTTPS), and
  timeouts in `sendfile_client_reset`/issue 2360 (the original can stall waiting
  for more than 2 MiB from a randomly smaller file).
- Mixed Rust/native ASan linking fails on macOS.
