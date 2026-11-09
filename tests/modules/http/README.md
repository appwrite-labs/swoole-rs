# HTTP tests

The HTTP module is `crates/http`; its exports' contracts are their doc comments.

Rust unit tests and the archive the native gates link against:

```sh
cargo test --locked -p swoole-rs-http
cargo clippy --locked -p swoole-rs-http --all-targets -- -D warnings
cargo build --locked -p swoole-rs-ffi --release
```

## Native differential gates

Each gate compiles pinned (v6.2.5) bodies against the hybrid build `--archive`
comes from and compares outcomes. The two sides compile apart (`Sides` in
`gate.py`): upstream fragments against the pin's headers only, the rest against
the hybrid build's patched tree (`hybrid_tree`). Native code runs under ASan/UBSan;
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
URL and boundary helpers are Rust behind thin facades, so the crate's
`pin_helpers` tests compare them with v6.2.4's bodies instead.

### Ported GoogleTests

Every TEST of `core-tests/src/server/http_server.cpp` is Rust, below, and the file is
no longer built (#1881).

`http_parser.cpp` and `multipart_parser.cpp` tested only the bundled third-party
parsers, so they left the lane (#1756) and with them `include/swoole_llhttp.h`.
Their ports drive crates/oracle's unmodified copies of `thirdparty/llhttp` and
`thirdparty/multipart_parser.c` with the same inputs, callbacks and expected values,
one Rust test per TEST under the TEST's own name: `cargo test -p swoole-rs-oracle
--test http_parser --test multipart_parser`.

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
the request's effects running the bundled multipart parser and real temp files. The
recorded requests come from `crates/oracle/upstream/core-tests/fuzz/cases`.

| Upstream TEST | Rust test | Same input | Assertions |
| --- | --- | --- | --- |
| `parser1` | `parser1` | `req1.bin`, default `package_max_length` | `form_data.size() == 3`; `200 OK` as no error response and the request dispatched whole |
| `parser2` | `parser2` | `req2.bin`, `package_max_length` 64 KiB, `upload_max_filesize` 1 MiB | `form_data.size() == 3`; `200 OK` as above; also that the upload streamed to disk is `test.jpg` |
| `upload` | `upload` | curl's chunked `-F file=@test.jpg` to `/upload?test=curl`, 2 MiB | `files.size() == 1`, `request_path` `/upload`, `query_string` `test=curl`; the server sends nothing itself |
| `max_request_size` | `max_request_size` | curl's `-F file=@test.jpg`, 128 KiB | `413 Request Entity Too Large`, the request never dispatched |

The 11 TESTs that start no server call the helpers, the request-line reader and the
WebSocket codec directly. Their ports sit beside the Rust each one calls, with the
same inputs and expected values: `cargo test -p swoole-rs-http -p swoole-rs-websocket --lib`.

| Upstream TEST | Rust test |
| --- | --- |
| `EncodeDecodeBasic`, `EncodeDecodeWithSpecialChars` | `crates/http/src/http_server.rs`: `encode_decode_basic`, `encode_decode_with_special_chars` |
| `get_method`, `get_method_str`, `get_status_message`, `parse_multipart_boundary` | `crates/http/src/http_server.rs`: `get_method_cases`, `get_method_str`, `get_status_message_cases`, `parse_multipart_boundary_cases` |
| `has_expect_header`, `get_protocol`, `all_method` | `crates/http/src/request.rs`: same names |
| `websocket_encode`, `websocket_compressed_control_frame` | `crates/websocket/src/frame_api.rs`: same names (`print_frame` to a temp file instead of `/tmp/swoole.log`) |

The TESTs that start a server run in `server/` (the merge tier's `http-server-rust`
phase): a Rust case binary over the hybrid tree's libswoole.so, built and run by
`tests/modules/hooks/system.py --package tests/modules/http/server --fixture
tests/modules/http/server/fixture.cc`. `fixture.cc` builds the Servers upstream's
`test_base_server`, `test_http_server` and `test_ssl_http` built (a C++ constructor,
`std::string` setters and callback slots); the callbacks, cpp-httplib's WebSocket client
(the same upgrade request, frames from the hybrid's `websocket::encode`/`mask`/`decode`),
the curl runs and every assertion are in `server/src/main.rs`, one function per TEST
under its own name: `websocket_small`, `websocket_medium`, `websocket_big`,
`websocket_mask`, `node_websocket_client_1`, `node_websocket_client_2`, `sni`,
`client_ca`, `ssl`, `abort_connection`, `get_package_length`. The two node TESTs stay
excluded for the reason they had in the GoogleTest lane (they need Node.js and its ws
package). Each curl runs to completion before its log is read, where upstream slept a
second after starting it.

`abort_connection`'s fixture also sets `SwooleG.max_sockets` (2) before
`set_max_connection(1)` and hands back the old value, which the case restores once the
server stops; the 16 `SyncClient`s are blocking `TcpStream` connects. In
`get_package_length` the fixture builds the port, runs `init_protocol` and makes the
fake connection socket and its `Connection` (session 2025); the case calls the hybrid's
`http_server::get_package_length(_size)`, `websocket::encode`, `sw_rs_http2_default_settings`
(`http2::init_settings`) and `http2::pack_setting_frame` itself, with upstream's frames
and expected values.

The HTTP/1 TESTs that start a server are `server/src/http1.rs`, in the same binary and
phase, and need no fixture: `swr_v1_server_new`/`_delete` (crates/server/src/server_object.rs)
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

The SDK's own response writer (`Context::end`, whose `TEST_STR` body curl printed in
`upload`) went with it; responses on the production path are `Swoole\Http\Response`'s,
covered by the PHPTs.

## PHP checks

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
