# Differential fuzzing

Each target feeds the same bytes to a Rust `swr_v1_*` export and to the pinned
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
`parse_cookie`. The shim gives each upstream `sw_malloc` one spare byte, so
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
nobody-chunked flags, errno and the multipart boundary must match. `http_range`
runs `StaticHandler::parse_range` from the pinned static_handler.cc and the one
`migration/patches/http/0002-static-ranges.patch` puts in its place (over the Rust
static context), over a file size up to `i64::MAX` (an `off_t`), MIME type length,
If-Range verdict and any Range header: status, content length, every task and its
part header, and the closing boundary must match.

For MQTT and RESP, `mqtt_length` compares `mqtt::get_package_length` over any
received bytes and `resp_parse` compares `redis::parse` (every string and, on
glibc, `ERANGE`), each against the same body after
`migration/patches/protocols/0001-rust-packet-protocols.patch`. Outside the compared domain only the candidate runs; `resp_parse` models upstream's walk
without reading memory to find those inputs.

For packet framing (`open_length_check`), `framing_unpack` compares
`swr_v1_framing_unpack` with `swoole_unpack` for every length type, and
`framing_length` compares `swr_v1_framing_default` with
`Protocol::default_length_func` over a length type, field offset, body offset and
any received bytes. Outside the compared domain, Rust's -1 only has to match an upstream error.
`framing_stream` runs the receive loops (`recv_with_length_protocol`,
`recv_with_eof_protocol`, `recv_split_by_eof`) from the pinned base.cc and from it
after `migration/patches/framing/0001-base.patch` (the Rust receive loop with
native effects) over the same stream, delivered in fuzzer-chosen chunks:
every call's result, every packet and the final buffer must match.

For the clients, `clients_url` compares `Address::assign(url)` from the pinned
address.cc with the same body after
`migration/patches/clients/0001-rust-address-client-policy.patch` (type, host,
port, or a `std::stoi` throw), on URLs up to 256 bytes because libstdc++'s
`std::regex` recurses per character. `clients_cookie` compares
`swr_v1_clients_cookie` with the Set-Cookie split in
`php_swoole_http_parse_set_cookies`, and `clients_domain_decode` compares
`swr_v1_clients_domain_decode` with dns.cc's `domain_decode` wherever Rust stays
in bounds; other inputs are outside the compared domain and aren't run.

libFuzzer writes new inputs to the first corpus directory, so keep `corpus/`
(ignored) first and the committed `seeds/` second. A failure leaves its input in
`artifacts/<target>/`; replay it with `cargo +nightly fuzz run <target> <file>`.
On Linux, build with `CC=clang CXX=clang++` so the C++ ASan matches rustc's.

## Adding a target

1. Declare the upstream function in `crates/oracle/src/lib.rs` and compile its
   unmodified source in `crates/oracle/build.rs` (through `cpp()`).
2. Add a comparison to `src/lib.rs`: call both sides on the same input, with
   red zones around each output, and assert on the returns and the bytes.
3. Add `fuzz_targets/<name>.rs` calling it, a `[[bin]]` in `Cargo.toml`, a few
   seeds in `seeds/<name>/` taken from the crate's exhaustive tests. A
   `<prefix>.dict` named after the target's first word is picked up too.

## CI

ClusterFuzzLite (`.clusterfuzzlite/`) builds every target. On PRs touching
`crates/`, `fuzz/` or `.clusterfuzzlite/`, it fuzzes the targets the change reaches
for five minutes (`cflite_pr.yml`). Nightly it fuzzes all targets for an hour
and then prunes its stored corpus (`cflite_batch.yml`). A crash fails the run,
and the minimised input is uploaded as `cflite-crashes`.
