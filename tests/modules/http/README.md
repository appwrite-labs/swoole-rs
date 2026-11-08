# HTTP checks

The HTTP module moves first-party helpers, request/header/chunk framing, cookies,
proxy policy, static paths/ranges/headers/calendar/rewrite/index rendering and
multipart accounting into Rust; it is graduated (always on). Native code keeps PHP and
server configuration, allocation/IO/logging, libc calendar/locale behaviour,
`std::regex` and the external llhttp/multipart parsers. See
[the ABI contract](../../../native/http/contract.md) for exact contracts.

Rust unit tests and the archive the native gates link against:

```sh
cargo test --locked -p swoole-rs-http
cargo clippy --locked -p swoole-rs-http --all-targets -- -D warnings
cargo build --locked -p swoole-rs-ffi --release
```

## Native differential gates

Each gate extracts the pinned (v6.2.4, `c7e514509451`) and the hybrid native
method bodies into separate namespaces and compares outcomes, effect traces,
errno and partial publication through deterministic native facades. The two
sides compile apart (`Sides` in `gate.py`): upstream fragments against the pin's
headers only, hybrid fragments and the driver against the fully patched tree of
the hybrid build `--archive` comes from (`hybrid_tree`), whose whole native
translation units also compile as C++14. Drivers that `#include` each engine's
bodies build one TU per engine (`SWR_ENGINE`). Native code runs under ASan/UBSan;
the Rust archive is uninstrumented unless `--rust-sanitize` is used (Linux;
links a private ASan HTTP archive instead, precompiled std stays uninstrumented).
All take `--source PINNED_GIT --archive HYBRID_ARCHIVE --output REPORT`; gates
marked `[R]` also take `--rust-sanitize`. `tools/dev.py` registers each as an
`http-*` phase.

| Script | Checks |
| --- | --- |
| `rewrite_gate.py` [R] | Pinned `add_rewrite_rule`/`apply_rewrite_rules` (std::regex) against the server's Rust rules (`swr_v1_server_static_rewrite`) |
| `locale_gate.py` | Case-insensitive helpers against libc `strncasecmp` across available locales |

Locale semantic runs must be unsanitized: under ASan the macOS interceptor
compares `C0`/`E0` with ASCII rules where libc in `en_US.ISO8859-1` treats them
as equal. Unavailable locales are reported as unsupported.

`gate.py` holds the helpers these share (`body`, `Sides`, `run`). The status, method,
URL and boundary helpers are Rust behind thin facades, so the crate's
`pin_helpers` tests compare them with v6.2.4's bodies instead. Upstream's own
`http_server`, `http_parser` and `multipart_parser` GoogleTests run on the reference
and the hybrid tree in the merge tier's `http-native-originals`; on the hybrid the
static-handler setup calls fill `Server::locations` and `http_index_files` directly
(`tools/core_tests.py` ADAPTATIONS), and the four ported TESTs below are excluded there.

### Ported GoogleTests

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

The SDK's own response writer (`Context::end`, whose `TEST_STR` body curl printed in
`upload`) went with it; responses on the production path are `Swoole\Http\Response`'s,
covered by the PHPTs.

## PHP checks

The merge tier's `api-surface` phase (`tools/capability_inventory.py
--require-same-api`) compares every Swoole class's reflected metadata,
including parameter and property defaults, against upstream.

### Original PHPTs

Upstream PHPTs for this module run in the merge tier's PHPT gate (the `phpt-*` jobs), which runs every pinned PHPT on upstream and the all-modules hybrid. To attribute a gate regression to this module, see *Attributing a regression* in [corpus/upstream/README.md](../../../corpus/upstream/README.md).

## Benchmark preparation

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

## Gate audit (#482)

Kept (23, now 17). Each differential gate compares pinned and patched native bodies
under ASan/UBSan, most with allocation-failure or errno injection that no PHPT,
Rust unit test or fuzz target reaches:
`http-native` (also the five original core-test bodies), `-framing`,
`-headers`, `-static`, `-conditionals`,
`-multipart`, `-multipart-headers`, `-multipart-emit`, `-rewrites`,
`-locales` (libc locales), `-context` and `-request-lifecycle`.
`-cookie-flow-positive` went with `parse_cookie` (dead once crates/phphttp parsed
cookies); its cases compare `swr_v1_http_cookie_next` with the pinned `parse_cookie`
in `crates/http/src/headers.rs` (`ported_cookie_flow_gate`). (`-boundary-rng` became a crates/corebase unit test, #995.)
`-status-buffer-positive` covers the TLS status buffer across threads.
`-status-codes-positive` covers the public `int[128]` ABI.

#752 made the Rust-context handler (formerly `StaticHandlerV2`) the only
`StaticHandler`, HTTP/2 included, and deleted the gates whose subject was the
legacy class: `-ranges`, `-paths`, `-index-pages`, `-inline` and
`-directory-compatibility`. `-context` compares the whole selected class with the
pinned one, and the `http_range` fuzz target now runs the selected `parse_range`.

Deleted (6 duplicates and one ported phase):

- `http-url-encode-positive`: the same 4,096 seeded encodings (same seed and
  check) run inside `http-native`.
- `http-static-positive`, `http-rewrites-positive`: strict subsets of `http-static`
  and `http-rewrites`, which run the same scenarios plus injected failures.
- `http-index-append-positive`, `http-proxy-auth-positive`: fault-free
  `make_index_page` and proxy auth differentials. `http-index-pages` and
  `http-proxy` ran the same functions over a wider path, entry, credential and
  capacity domain.
- `http-framing`, `http-headers`, `http-multipart`, `http-multipart-emit`,
  `http-multipart-headers`, `http-request-lifecycle`: compared `Request`'s C++
  bodies with upstream's. Those bodies are Rust now (`crates/http/src/request.rs`,
  #931); its unit tests carry the cases, including core-tests' `get_protocol` and
  `has_expect_header`, and the HTTP PHPTs cover the server path.
- `http-proxy`, `http-dispatch`: sliced `HttpProxy` out of the hybrid http.cc. Their
  cases run in `crates/http/src/owned_proxy.rs` (`ported_proxy_gate_*`,
  `ported_dispatch_gate`) against the pinned `HttpProxy` compiled in crates/oracle
  (`shim/http_proxy.cc`), through the Rust owner and the calls the C++ class makes.
  Injected C++ allocation faults became refused native-cache assignments, the
  failure the Rust owner reports; the C++ class itself is a forwarding shell.
- `http-binding-reflection`: ported into `api-surface`. `tools/php_api_inventory.php`
  now records parameter default values, class modifiers and default properties
  for every Swoole class, not just these five.
- `http-static`: compared `select_static_handler`'s C++ body with upstream's.
  `src/server/static_handler.cc` has left the build and the HTTP/1 static response
  is Rust (`crates/server/src/static_select.rs`). Its three case groups run there as
  `former_static_gate_response_branches`, `_range_extremes` and
  `_sends_change_the_request`, against the pinned body compiled in crates/oracle
  (`shim/http_static_select.cc`), with the same scenarios and comparisons.
- `http-context`, `http-conditionals`: compared the whole `StaticHandler` and its
  date callers through `context_facade.cc`, a test-only C++ facade over the Rust
  context. Their cases run in `crates/server/src/static_handler/tests.rs` as
  `former_context_gate_files`, `_directories` and `former_conditional_gate`:
  production's `StaticHandler` with scripted `Effects` (filesystem, MIME table,
  random strings, recorded libc calendar) against the pinned class compiled in
  crates/oracle (`shim/http_static_context.cc`) over the same script, comparing
  every answer, effect trace and errno. Three assertions were about the C++
  facade's own storage and do not apply to an owned Rust value: the native
  `String`'s reserve/extend events and fields (the Rust page is a `Vec`; the
  upstream side still runs every buffer capacity and its page bytes are compared),
  the candidate's MIME/content-type references (upstream's are still asserted),
  and the facade's null-server short circuit. The calendar zones run in a child
  process per `TZ`.
