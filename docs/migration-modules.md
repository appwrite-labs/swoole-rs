# Migration modules

[`migration/modules.json`](../migration/modules.json) lists the migration
modules. Its top level holds `schema_version`, the pinned `upstream` (Swoole
v6.2.4, commit `c7e514509451beafd4d70ce5b78943d51e76381c`, fetched at
`target/upstream/src/`) and `modules`. Each module entry has:

| Field | Meaning |
| --- | --- |
| `id`, `title` | Module name (`M00`..`M29`) and description. |
| `wave` | Position in the dependency order. |
| `depends_on` | Module IDs that must be integrated first. |
| `owned_paths` | Repository paths for the module (crate, native adapter, patches, tests). |
| `upstream_patch_targets` | Upstream files the module mainly patches (informational). |
| `applies_first` | Apply this module's patches before every other module's. Only M15 (halfclose) sets it: Rust patches on the same files include its changes as context. |
| `feature_bit_reserved` | Feature value, where one is listed. |
| `abi_contract` | Path to the module's contract document or header. |

`python3 tools/check_migration.py` checks for duplicate IDs, unknown
prerequisites and dependency cycles, and prints the resulting waves.

Each module's patches live in `migration/patches/<module>/` and apply in
file-name order; their destinations are read from their diff headers.
`tools/build_hybrid.py` applies them to a fresh clone of the pinned reference
(see [hybrid build](hybrid-build.md)). Every Rust build applies every module and
sets its feature bit in `swr_v1_feature_bits()`.

## Modules

Feature values come from `MODULES` in `tools/build_hybrid.py`.

| ID | Name | Feature | Upstream targets | Scope |
| --- | --- | ---: | --- | --- |
| M00 | (always) | — | `config.m4`, `CMakeLists.txt`, `ext-src/php_swoole.cc`, `include/swoole_server.h`, `src/server/master.cc` | ABI base (`native/include/`, `crates/ffi/`), build registration, startup ABI check. See [ABI v1](abi-v1.md). |
| M01 | `buffer` | 2 | `src/core/buffer.cc`, `src/core/string.cc` and headers | Output buffer queue and `String` storage. [Contract](buffer-abi-v1.md). Graduated: always on. |
| M02 | `websocket` | 1 | `src/protocol/websocket.cc` (+ header, `ext-src/swoole_websocket_server.cc`) | Frame length, header writing, masking. [Boundary](websocket-boundary.md). Graduated: always on. |
| M03 | `timer` | 4 | `src/core/timer.cc`, `ext-src/swoole_timer.cc`, `src/wrapper/timer.cc`, `src/os/timer.cc` (+ header) | Timer queue. [Contract](timer-abi-v1.md), [subsystem](timer-subsystem.md). Graduated: always on. |
| M04 | `http` | 256 | `src/protocol/http.cc`, `src/server/static_handler.cc` (+ header) | HTTP framing, header/cookie/URL/multipart helpers, method/status, ranges, static responses. Third-party parsers stay. Graduated: always on. [Contract](../native/http/contract.md). |
| M05 | `reactor` | 16384 | `src/reactor/`, `src/network/{socket,stream}.cc`, `src/server/{base,reactor_thread}.cc`, `ext-src/swoole_admin_server.cc` (+ headers) | Readiness policy and socket output. [Boundary](reactor-abi-v1.md). |
| M06 | `coroutine` | 32768 | `src/coroutine/{base,context,thread_context,channel}.cc` (+ headers) | Scheduler transitions, channel fairness/storage. [Boundary](coroutine-abi-v1.md). |
| M07 | `hooks` | 65536 | `src/coroutine/{hook,socket,system,file,file_lock,iouring,uring_socket}.cc`, `ext-src/swoole_runtime.cc`, `include/swoole_coroutine_socket.h` | Coroutine I/O and runtime hooks. [Boundary](hooks-abi-v1.md). |
| M08 | `process` | 524288 | `src/server/{worker,task_worker,process,manager,reactor_process}.cc`, `src/os/{process_pool,pipe,signal,wait}.cc` | Workers, process pool, IPC, signals. [Boundary](process-abi-v1.md). |
| M09 | `base64` | 8 | `src/core/base64.cc` | [Contract](core-abi-v1.md). Graduated: always on. |
| M10 | `crc32` | 16 | `src/core/crc32.cc` | [Contract](core-abi-v1.md). Graduated: always on. |
| M11 | `error` | 32 | `src/core/error.cc` | Known error strings and formatting. [Contract](core-abi-v1.md). Graduated: always on. |
| M12 | `memory` | 64 | `src/memory/`, `src/core/{heap,channel}.cc` (+ headers) | Memory pools, shared storage, core collections. `native/memory/swoole_rs_memory.h`. |
| M13 | `protocols` | 128 | `src/protocol/{mqtt,redis,socks5,message_bus}.cc`, `ext-src/swoole_redis_server.cc` (+ headers) | Packet protocol and proxy codecs. `native/protocols/swoole_rs_protocols.h`. Graduated: always on. |
| M14 | `utilities` | 512 | `src/core/{misc,log}.cc`, `include/swoole_{log,util}.h`, `include/swoole.h` | Core utilities and logging. Hash width, signed-char promotion and wrapping follow the pinned code. [Contract](../native/utilities/contract.md). |
| M15 | `--fix-halfclose` | — | patches (applied first) on `src/server/{base,master,reactor_thread}.cc`, `src/network/socket.cc` | Native (not Rust) correction: drain queued output before close, keep pre-queue responses, send TLS close_notify. |
| M16 | `locks` | 1024 | `src/lock/*.cc`, `ext-src/swoole_{lock,atomic,thread_lock,thread_atomic}.cc` (+ headers) | Native locks and PHP lock/atomic. [Contract](../native/locks/contract.md). |
| M17 | `wrappers` | 2048 | `src/wrapper/{event,http}.cc` | Event, timer and HTTP wrapper policy. [Contract](../native/wrappers/contract.md). |
| M18 | `framing` | 4096 | `src/protocol/{base,mime_type}.cc` | Packet framing and MIME lookup. Graduated: always on. |
| M19 | `corebase` | 8192 | `src/core/base.cc` | Core base utilities and lifecycle policy. Graduated: always on. |
| M20 | `ossupport` | 131072 | `src/os/{async_thread,base,file,msg_queue,sendfile,unix_socket}.cc` | OS helpers and async worker pool. [Boundary](os-support-abi-v1.md). |
| M21 | `tls` | 262144 | `src/protocol/{ssl,dtls}.cc` | TLS/DTLS setup and BIO policy; OpenSSL unchanged. [Boundary](tls-dtls-abi-v1.md). |
| M22 | `clients` | 1048576 | `src/network/{address,client,dns}.cc`, `ext-src/swoole_{client,client_async,client_coro,name_resolver,http_client_coro}.cc` | Address parsing, resolver, clients. [Boundary](clients-resolver-abi-v1.md). |
| M23 | `httpbindings` | 2097152 | `ext-src/swoole_http_{cookie,request,response,server_coro}.cc` | PHP HTTP Request/Response/Cookie bindings. [Boundary](http-bindings-abi-v1.md). |
| M24 | `http2` | 4194304 | `src/protocol/http2.cc`, `ext-src/swoole_http2_{server,client_coro}.cc` | HTTP/2 frame/settings/stream policy. [Boundary](http2-abi-v1.md). |
| M25 | `ports` | 8388608 | `src/server/port.cc`, `ext-src/swoole_server_port.cc` | ListenPort options and PHP Port. [Boundary](ports-abi-v1.md). |
| M26 | `phphelpers` | 16777216 | `ext-src/php_swoole_cxx.{cc,h}`, `ext-src/swoole_stdext.cc` | Common PHP helpers and stdext. [Boundary](phphelpers-abi-v1.md). |
| M27 | `phpcollections` | 33554432 | `ext-src/swoole_{table,channel_coro}.cc` | PHP Table and coroutine Channel. [Boundary](phpcollections-abi-v1.md). |
| M28 | `phpevents` | 67108864 | `ext-src/swoole_event.cc` | PHP Event frontend. |
| M29 | `phpcoroutines` | 134217728 | `ext-src/swoole_{coroutine,coroutine_system,coroutine_scheduler,tracer}.cc`, `ext-src/php_swoole_coroutine.h` | PHP Coroutine, System and Scheduler frontend. [Boundary](phpcoroutines-boundary.md). |

What upstream code each file still keeps is listed in the [roadmap](migration.md#upstream-files-still-in-the-build).
