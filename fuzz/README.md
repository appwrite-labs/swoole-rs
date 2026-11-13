# Differential fuzzing

Each target feeds the same bytes to a Rust entry point and to the pinned
upstream C++ in `crates/oracle`, and fails on any difference in the return value
or any output byte. `cargo fuzz` builds both sides with ASan, so a memory error
in either is a crash too. The workspace stays on its pinned stable toolchain;
fuzzing needs nightly:

```sh
rustup toolchain install nightly --profile minimal
cargo +nightly install cargo-fuzz --locked
cd fuzz
mkdir -p corpus/base64_decode
cargo +nightly fuzz run base64_decode corpus/base64_decode seeds/base64_decode -- -dict=base64.dict -max_total_time=60
```

Targets: `base64_decode`, `base64_encode`, and for the WebSocket codec
`ws_decode` (any bytes as one frame: verdict, fields and in-place unmasking),
`ws_frame_length` (total and header length of any prefix) and `ws_mask` (first
four bytes as the key; masking twice restores the data). The WebSocket oracle
compiles upstream's `get_package_length_impl`, `mask` and `decode` bodies
unchanged, copied by `crates/oracle/build.rs`. Inputs outside the compared domain are
not compared; Rust's result is still checked.


For HTTP, `http_url_decode`, `http_url_encode` and `http_cookies` (keys and
values in order) compare with upstream's `url_decode`, `url_encode` and
`parse_cookie` (`url_decode` over a slice, with the terminator upstream writes added
before the buffers are compared). The shim gives each upstream `sw_malloc` one spare byte, so
upstream runs to completion on every input.


For HTTP request framing, `http_frame` runs `get_protocol`, `get_header_length`
or `get_chunked_body_length` from a chosen offset, header length and request
state, and `http_header` runs `has_expect_header`, `get_header(name)` and
`get_method_string`, and `http_header_info` runs `parse_header_info`. Each compares
the pinned http.cc bodies (copied by `crates/oracle/build.rs`, over a `Request` with
upstream's field types) with `swoole_rs_http::Request`. Inputs outside the compared
domain are not compared; the candidate is still checked. Seeds include the requests
`http.client` sends for the corpus applications.

`http_header_info` runs `Request::parse_header_info` on both engines over any
request head (up to its blank line): content length, keep-alive, chunked and
excepted (a Transfer-Encoding not ending in chunked) flags, errno and the multipart boundary must match. `http_range`
runs `StaticHandler::parse_range` from the pinned static_handler.cc and
`swoole_rs_http::HttpStaticContext::parse_range`, which the server calls, over a file size up to `i64::MAX` (an `off_t`), MIME type length,
If-Range verdict and any Range header: status, content length, every task and its
part header, and the closing boundary must match.

For MQTT and RESP, `mqtt_length` compares `mqtt::get_package_length` over any
received bytes and `resp_parse` compares `redis::parse` (every string and, on
glibc, `ERANGE`), each against the hybrid's body over Rust's codec
(`src/protocols_native.rs`). Outside the compared domain only Rust runs; `resp_parse` models upstream's walk
without reading memory to find those inputs. `resp_stream` runs `redis::recv_packet`
over a chunked stream and `socks5` drives `Socks5Proxy` from `create` through
arbitrary server replies, each against Rust's (`recv_packet`, and the SOCKS5 codec
behind the hybrid's `Socks5Proxy` methods).

For packet framing (`open_length_check`), `framing_unpack` compares
`swoole_rs_framing::unpack` with `swoole_unpack` for every length type, and
`framing_length` compares `swoole_rs_framing::default_length` with
`Protocol::default_length_func` over a length type, field offset, body offset and
any received bytes. Outside the compared domain, Rust's -1 only has to match an upstream error.
`framing_stream` runs the receive loops (`recv_with_length_protocol`,
`recv_with_eof_protocol`, `recv_split_by_eof`) from the pinned base.cc and from
`crates/oracle/candidate/src/protocol/base.cc` (the Rust receive loop with
native effects) over the same stream, delivered in fuzzer-chosen chunks:
every call's result, every packet and the final buffer must match for the EOF
loops. The Rust length loop reads greedily (one `recv` of the buffer's free space,
then every whole package), so for it the packets and whether the stream ends in an
error must match.

For the clients, `clients_url` compares `Address::assign(url)` from the pinned
address.cc with `swoole_rs_clients::url` (type, host, port, or a `std::stoi`
throw), on URLs up to 256 bytes because libstdc++'s
`std::regex` recurses per character. `clients_cookie` compares
`swoole_rs_clients::cookie` with the Set-Cookie split in
`php_swoole_http_parse_set_cookies`, and `clients_domain_decode` compares
`swoole_rs_clients::domain_decode` with dns.cc's `domain_decode` wherever Rust stays
in bounds; other inputs are outside the compared domain and aren't run.

The targets in `src/network.rs` but `http2_hpack` have no oracle side; each checks a parser that
reads untrusted bytes against the wire format or against invariants of its output.
`http2_frame` checks `get_frame_length`, `sw_rs_http2_decode_frame` and
`sw_rs_http2_unpack_setting` against the RFC 9113 frame header and SETTINGS layout.
`clients_dns_reply` hands `swoole_rs_clients::dns::query` any reply (optionally echoing the
query's id): it succeeds exactly when it pushes an address, and each is IP text.
`http_multipart` feeds crates/http's multipart parser a boundary and body
in chunks, and runs `swoole_rs_http::multipart_filename` on each part's headers.
`http_chunked` feeds crates/http's llhttp a chunked request or response in chunks: body
spans stay inside their chunk and never exceed the input. `http2_hpack` splits its
input into header block fragments of one connection and decodes each with
`swoole_rs_http2::hpack` and the nghttp2 inflater upstream bundled
(`upstream/thirdparty/nghttp2`) through the step loop the HTTP/2 server and client
run (`sw_rs_http2_inflate_step`): both read the same bytes, decode the same fields
and fail on the same step, and the loop ends within one step per byte. `resp_server` walks a
`Redis\Server` request with `resp_php_next`: spans stay inside the request,
the walk advances and ends, and there is one command at most.
`sni_wildcard` splits its input at NUL into an SNI name and a certificate name and
checks `swoole_rs_utilities::sni_wildcard_matches` against a glob over the lowercased
names, in which the first `*`, with no `.` before it, matches any run without a `.`.
`http_rewrite` adds one `~pattern~` rule to `swoole_rs_http::RewriteRules` and applies it
to a URL (pattern, replacement and URL split at NUL): compiling and matching end, and a
pattern of letters, digits and `/` replaces each occurrence as literal text. ClusterFuzzLite
links libc++, not the libstdc++ upstream's `std::regex` comes from, so the comparison with
upstream is `tests/modules/http/rewrite.rs`'s, in `http-unit`.
`ws_inflate` hands any bytes to `swoole_rs_inflate::message` (permessage-deflate,
RFC 7692), then deflates the rest with `swoole_rs_deflate::message` at the level the
first byte picks: inflating that gives the rest back. `http_inflate` compresses a body
as gzip, zlib or raw deflate and feeds it to `Inflate::decode` (or the HTTP/2 client's
`Inflate::gzip_only`) in fuzzer-chosen chunks: one call and the chunks both give the body
back, except where a raw stream's first chunk can't trigger upstream's first-chunk raw
retry. The uncompressed body, whole and in the same chunks, decodes without a panic.

libFuzzer writes new inputs to the first corpus directory, so keep `corpus/`
(ignored) first and the committed `seeds/` second. A failure leaves its input in
`artifacts/<target>/`; replay it with `cargo +nightly fuzz run <target> <file>`.
On Linux, build with `CC=clang CXX=clang++` so the C++ ASan matches rustc's.
On macOS, ld64 keeps the crates' unreached references to upstream symbols, so link with
`RUSTFLAGS=-Clink-arg=-Wl,-undefined,dynamic_lookup`, and run with
`ASAN_OPTIONS=detect_container_overflow=0`: libFuzzer's own corpus reading otherwise
trips a container-overflow false positive against the system libc++.

## Adding a target

1. Declare the upstream function in `crates/oracle/src/lib.rs` and compile its
   unmodified source in `crates/oracle/build.rs` (through `cpp()`).
2. Add a comparison to `src/lib.rs`: call both sides on the same input, with
   red zones around each output, and assert on the returns and the bytes.
3. Add `fuzz_targets/<name>.rs` calling it, a `[[bin]]` in `Cargo.toml`, a few
   seeds in `seeds/<name>/` taken from the crate's exhaustive tests. A
   `<prefix>.dict` named after the target's first word is picked up too.

## CI

ClusterFuzzLite (`.clusterfuzzlite/`) builds every target. On PRs touching a crate
the targets link, `fuzz/` or `.clusterfuzzlite/`, it fuzzes the targets the change
reaches for two minutes (`cflite_pr.yml`). Nightly it fuzzes all targets for an hour
and then prunes its stored corpus (`cflite_batch.yml`), kept on the `cflite` branch.
A crash fails the run,
and the minimised input is uploaded as `cflite-crashes`.
