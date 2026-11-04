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
cargo build --locked -p swoole-rs-ffi --no-default-features --features http --release
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
| `proxy_gate.py` [R] | Proxy auth packing/handshake with actual base64 |
| `static_gate.py` [R] | Static response method, send order, HEAD/keep-alive changes |
| `conditional_gate.py` | Date callers with libc `strptime`/`gmtime`/`mktime` |
| `rewrite_gate.py` [R] | `add_rewrite_rule`/`apply_rewrite_rules`, rewriting the Rust static context |
| `dispatch_gate.py` [R] | Route and proxy-init bodies |
| `locale_gate.py` | Case-insensitive helpers against libc `strncasecmp` across available locales |
| `cookie_flow_gate.py` [R] | Cookie callback flow |
| `context_gate.py` [R] | The whole `StaticHandler` (paths, locations, rewrites, index files and pages, ranges, content types, dates) against the pinned class |
| `boundary_rng_gate.py --archive ARCHIVE` | Random-boundary overloads: pinned bodies against the hybrid build's `base.cc` |

Locale semantic runs must be unsanitized: under ASan the macOS interceptor
compares `C0`/`E0` with ASCII rules where libc in `en_US.ISO8859-1` treats them
as equal. Unavailable locales are reported as unsupported.

`gate.py` holds the helpers these share (`body`, `Sides`, `run`). The status, method,
URL and boundary helpers are Rust behind thin facades, so the crate's
`pin_helpers` tests compare them with v6.2.4's bodies instead. Upstream's own
`http_server`, `http_parser` and `multipart_parser` GoogleTests run unchanged on
the reference and the hybrid tree in the merge tier's `http-native-originals`.

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
- `boundary_rng_gate.py` covers only the positive 24-byte length.
- Mixed Rust/native ASan linking fails on macOS.

## Gate audit (#482)

Kept (23, now 18). Each differential gate compares pinned and patched native bodies
under ASan/UBSan, most with allocation-failure or errno injection that no PHPT,
Rust unit test or fuzz target reaches:
`http-native` (also the five original core-test bodies), `-framing`,
`-headers`, `-proxy`, `-static`, `-conditionals`,
`-multipart`, `-multipart-headers`, `-multipart-emit`, `-rewrites`,
`-dispatch`, `-locales` (libc locales), `-context`, `-request-lifecycle`,
`-boundary-rng` (corebase base.cc) and `-cookie-flow-positive`.
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
  `http-proxy` run the same functions over a wider path, entry, credential and
  capacity domain.
- `http-framing`, `http-headers`, `http-multipart`, `http-multipart-emit`,
  `http-multipart-headers`, `http-request-lifecycle`: compared `Request`'s C++
  bodies with upstream's. Those bodies are Rust now (`crates/http/src/request.rs`,
  #931); its unit tests carry the cases, including core-tests' `get_protocol` and
  `has_expect_header`, and the HTTP PHPTs cover the server path.
- `http-binding-reflection`: ported into `api-surface`. `tools/php_api_inventory.php`
  now records parameter default values, class modifiers and default properties
  for every Swoole class, not just these five.
