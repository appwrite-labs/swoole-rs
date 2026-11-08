# HTTP tests

The HTTP module is `crates/http`; [native/http/contract.md](../../../native/http/contract.md)
has its ABI.

Rust unit tests and the archive the native gates link against:

```sh
cargo test --locked -p swoole-rs-http
cargo clippy --locked -p swoole-rs-http --all-targets -- -D warnings
cargo build --locked -p swoole-rs-ffi --release
```

## Native differential gates

Each gate compiles pinned (v6.2.4) bodies against the hybrid build `--archive`
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
`pin_helpers` tests compare them with v6.2.4's bodies instead. Upstream's own
`http_server` GoogleTests run on the reference and the hybrid tree in the merge tier's
`http-native-originals`; on the hybrid the static-handler setup calls fill
`Server::locations` and `http_index_files` directly (`tools/core_tests.py`
ADAPTATIONS), and the ported TESTs below are excluded there.

### Ported GoogleTests

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
same inputs and expected values, and the lane excludes them (`tools/dev.py`):
`cargo test -p swoole-rs-http -p swoole-rs-websocket --lib`.

| Upstream TEST | Rust test |
| --- | --- |
| `EncodeDecodeBasic`, `EncodeDecodeWithSpecialChars` | `crates/http/src/http_server.rs`: `encode_decode_basic`, `encode_decode_with_special_chars` |
| `get_method`, `get_method_str`, `get_status_message`, `parse_multipart_boundary` | `crates/http/src/http_server.rs`: `get_method_cases`, `get_method_str`, `get_status_message_cases`, `parse_multipart_boundary_cases` |
| `has_expect_header`, `get_protocol`, `all_method` | `crates/http/src/request.rs`: same names |
| `websocket_encode`, `websocket_compressed_control_frame` | `crates/websocket/src/frame_api.rs`: same names (`print_frame` to a temp file instead of `/tmp/swoole.log`) |

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
  runner's unsupported custom `EXPECTF_85` section, zstd-disabled builds, missing
  `ip` executable (`server_addr`), `bug_2444` (needs MySQL), `tmp-content-type`
  (needs Composer Guzzle), coroutine `close_socket` (needs external HTTPS), and
  timeouts in `sendfile_client_reset`/issue 2360 (the original can stall waiting
  for more than 2 MiB from a randomly smaller file).
- Mixed Rust/native ASan linking fails on macOS.
