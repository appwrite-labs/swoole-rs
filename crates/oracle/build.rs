// upstream/ holds unmodified files from Swoole v6.2.5 (76c657d8); see src/lib.rs. candidate/
// holds four of them as the hybrid shipped them before Rust replaced each file outright:
// their bodies call Rust, so each oracle runs Rust against upstream through the same C++.
#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "a build script reports failure by panicking"
)]
use std::{env, fs, path::PathBuf};

fn out(name: &str) -> PathBuf {
    PathBuf::from(env::var("OUT_DIR").unwrap()).join(name)
}

/// The text of `text` from `start` up to (not including) `end`, unchanged.
fn slice(text: &str, start: &str, end: &str) -> String {
    let from = text.find(start).expect(start);
    text[from..from + text[from..].find(end).expect(end)].to_string()
}

/// The pinned HTTP cookie parser and URL decode/encode, copied unchanged for
/// shim/http.cc to compile.
fn http() {
    let source = fs::read_to_string("upstream/src/protocol/http.cc").expect("vendored http.cc");
    let text = [
        slice(
            &source,
            "void parse_cookie(",
            "bool parse_multipart_boundary(",
        ),
        slice(
            &source,
            "static int url_htoi(",
            "int Request::get_protocol()",
        ),
    ]
    .concat();
    fs::write(out("http.inc"), text).unwrap();
}

/// `HttpProxy::create`, `get_auth_str`, `pack` and `handshake` from the pinned http.cc,
/// copied unchanged for `shim/http_proxy.cc`.
fn http_proxy() {
    let source = fs::read_to_string("upstream/src/protocol/http.cc").expect("vendored http.cc");
    let text = slice(
        &source,
        "HttpProxy *HttpProxy::create(",
        "namespace http_server {",
    );
    fs::write(out("http_proxy.inc"), text).unwrap();
}

/// The definition starting at `signature`, through its matching brace.
fn definition(text: &str, signature: &str) -> String {
    let start = text.find(signature).expect(signature);
    let mut depth = 0;
    for (offset, char) in text[start..].char_indices() {
        match char {
            '{' => depth += 1,
            '}' if depth == 1 => {
                let body = &text[start..=(start + offset)];
                assert!(
                    !body.contains("\n#"),
                    "no preprocessor branches in {signature}"
                );
                return body.to_string();
            }
            '}' => depth -= 1,
            _ => {}
        }
    }
    panic!("unterminated {signature}");
}

/// Request framing from the pinned http.cc, copied unchanged for `shim/http_frame.cc`,
/// with the method and version enums from the pinned `swoole_http.h`. The candidate is
/// `swoole_rs_http::Request` itself (fuzz/src/lib.rs).
fn http_frame() {
    let text = fs::read_to_string("upstream/src/protocol/http.cc").expect("vendored http.cc");
    let mut parts = vec![slice(&text, "static const char *method_strings[]", "\n};") + "\n};"];
    for signature in [
        "void Request::parse_header_info(",
        "int Request::get_protocol(",
        "int Request::get_header_length(",
        "static bool is_http_token_char(",
        "static bool is_http_quoted_char(",
        "static bool is_http_quoted_pair_char(",
        "static ChunkSizeResult parse_chunk_size(",
        "int Request::get_chunked_body_length(",
        "bool Request::has_expect_header(",
        "std::string Request::get_header(",
        "const char *get_method_string(",
    ] {
        parts.push(definition(&text, signature));
    }
    fs::write(out("http_frame_upstream.inc"), parts.join("\n") + "\n").unwrap();
    let header =
        fs::read_to_string("upstream/include/swoole_http.h").expect("vendored swoole_http.h");
    let enums = [
        slice(&header, "enum swHttpVersion {", "\n};") + "\n};\n",
        slice(&header, "enum swHttpMethod {", "\n};") + "\n};\n",
        slice(&header, "enum ChunkState {", "\n};") + "\n};\n",
        slice(&header, "enum ChunkSizeResult {", "\n};") + "\n};\n",
    ];
    fs::write(out("http_frame_enums.inc"), enums.concat()).unwrap();
}

/// `StaticHandler::parse_range` from the pinned `static_handler.cc`, copied unchanged
/// for `shim/http_range.cc`.
fn http_range() {
    let upstream = fs::read_to_string("upstream/src/server/static_handler.cc")
        .expect("vendored static_handler.cc");
    fs::write(
        out("http_range_upstream.inc"),
        definition(&upstream, "void StaticHandler::parse_range(") + "\n",
    )
    .unwrap();
}

/// `Server::select_static_handler` from the pinned `static_handler.cc` and
/// `get_status_message` from the pinned http.cc, copied unchanged for
/// `shim/http_static_select.cc`.
fn http_static_select() {
    let handler = fs::read_to_string("upstream/src/server/static_handler.cc")
        .expect("vendored static_handler.cc");
    let http = fs::read_to_string("upstream/src/protocol/http.cc").expect("vendored http.cc");
    fs::write(
        out("http_static_select.inc"),
        definition(&handler, "bool Server::select_static_handler(") + "\n",
    )
    .unwrap();
    fs::write(
        out("http_static_select_status.inc"),
        slice(&http, "static thread_local char http_status_message[", "\n")
            + "\n"
            + &definition(&http, "const char *get_status_message(")
            + "\n",
    )
    .unwrap();
}

/// The `StaticHandler` class from the pinned `swoole_static_handler.h`, its method
/// bodies and `Server::apply_rewrite_rules` from the pinned `static_handler.cc`, and
/// `url_decode` from the pinned http.cc, copied unchanged for `shim/http_static_context.cc`.
fn http_static_context() {
    let header = fs::read_to_string("upstream/include/swoole_static_handler.h")
        .expect("vendored swoole_static_handler.h");
    fs::write(
        out("http_static_context_class.inc"),
        slice(
            &header,
            "struct RewriteRule {",
            "};  // namespace http_server",
        ),
    )
    .unwrap();
    let http = fs::read_to_string("upstream/src/protocol/http.cc").expect("vendored http.cc");
    let handler = fs::read_to_string("upstream/src/server/static_handler.cc")
        .expect("vendored static_handler.cc");
    let mut parts = vec![
        definition(&http, "static int url_htoi("),
        definition(&http, "size_t url_decode("),
    ];
    for signature in [
        "bool StaticHandler::is_modified(",
        "bool StaticHandler::is_modified_range(",
        "std::string StaticHandler::get_date_last_modified(",
        "bool StaticHandler::get_absolute_path(",
        "bool StaticHandler::try_serve(",
        "bool StaticHandler::try_serve_index_file(",
        "size_t StaticHandler::make_index_page(",
        "bool StaticHandler::get_dir_files(",
        "bool StaticHandler::set_filename(",
        "void StaticHandler::parse_range(",
        "bool Server::apply_rewrite_rules(",
    ] {
        parts.push(definition(&handler, signature));
    }
    fs::write(out("http_static_context.inc"), parts.join("\n") + "\n").unwrap();
}

/// MQTT's length decoder and RESP's parse from the pinned mqtt.cc and redis.cc
/// (upstream) and from candidate/'s (calling Rust), copied unchanged for
/// shim/protocols.cc, with redis's states.
fn protocols() {
    for name in ["upstream", "candidate"] {
        let mqtt = fs::read_to_string(format!("{name}/src/protocol/mqtt.cc")).expect("mqtt.cc");
        let redis = fs::read_to_string(format!("{name}/src/protocol/redis.cc")).expect("redis.cc");
        let mut parts = Vec::new();
        if mqtt.contains("#define SW_MQTT_RECV_LEN_AGAIN") {
            parts.push(slice(&mqtt, "#define SW_MQTT_RECV_LEN_AGAIN", "\n") + "\n");
        }
        parts.push(definition(&mqtt, "ssize_t get_package_length("));
        if redis.contains("const char *get_number(") {
            parts.push(definition(&redis, "const char *get_number("));
        }
        parts.push(definition(&redis, "std::vector<std::string> parse("));
        fs::write(
            out(&format!("protocols_{name}.inc")),
            parts.join("\n") + "\n",
        )
        .unwrap();
    }
    let header =
        fs::read_to_string("upstream/include/swoole_redis.h").expect("vendored swoole_redis.h");
    fs::write(
        out("protocols_states.inc"),
        slice(&header, "enum State {", "\n};") + "\n};\n",
    )
    .unwrap();
}

/// The `Address` methods, the hosts lookup, the Set-Cookie split, the DNS name
/// codec and the socket DNS lookup from the pinned sources, copied unchanged for shim/clients.cc.
fn clients() {
    let address =
        fs::read_to_string("upstream/src/network/address.cc").expect("vendored address.cc");
    let methods = [
        "static bool IN_IS_ADDR_LOOPBACK(const in_addr *a) {",
        "bool Address::verify_ip(int family, const std::string &str) {",
        "bool Address::verify_port(const int port, const bool for_connect) {",
        "bool Address::assign(SocketType _type, const std::string &_host, int _port, bool _resolve_name) {",
        "bool Address::assign(const std::string &url)",
        "bool Address::is_loopback_addr() const {",
    ];
    fs::write(
        out("clients_address.inc"),
        methods
            .iter()
            .map(|signature| definition(&address, signature) + "\n")
            .collect::<String>(),
    )
    .unwrap();
    let dns = fs::read_to_string("upstream/src/network/dns.cc").expect("vendored dns.cc");
    fs::write(
        out("clients_dns.inc"),
        [
            "static void domain_decode(char *str) {",
            "std::string get_ip_by_hosts(const std::string &search_domain) {",
        ]
        .iter()
        .map(|signature| definition(&dns, signature) + "\n")
        .collect::<String>(),
    )
    .unwrap();
    let records = [
        "enum RecordType {",
        "struct RecordHeader {",
        "struct Q_FLAGS {",
        "struct RR_FLAGS {",
    ];
    let functions = [
        "static int domain_encode(const char *src, int n, char *dest) {",
        "static void domain_decode(char *str) {",
        "static std::string parse_ip_address(void *vaddr, int type) {",
        "std::vector<std::string> dns_lookup_impl_with_socket(",
    ];
    fs::write(
        out("clients_dns_lookup.inc"),
        records
            .iter()
            .map(|signature| definition(&dns, signature) + ";\n")
            .chain(
                functions
                    .iter()
                    .map(|signature| definition(&dns, signature) + "\n"),
            )
            .collect::<String>(),
    )
    .unwrap();
    let coro = fs::read_to_string("upstream/ext-src/swoole_http_client_coro.cc")
        .expect("vendored swoole_http_client_coro.cc");
    fs::write(
        out("clients_cookie.inc"),
        slice(
            &coro,
            "    const char *eof = at + length;",
            "    if (value_len != 0) {",
        ),
    )
    .unwrap();
}

/// The pinned packet length function and integer unpacking, copied unchanged for
/// shim/framing.cc to compile.
fn framing() {
    let source = fs::read_to_string("upstream/src/protocol/base.cc").expect("vendored base.cc");
    let from = source
        .find("int64_t swoole_unpack(")
        .expect("swoole_unpack");
    let text = [
        slice(
            &source,
            "ssize_t Protocol::default_length_func(",
            "int Protocol::recv_split_by_eof(",
        ),
        source[from..].to_string(),
    ]
    .concat();
    fs::write(out("framing.inc"), text).unwrap();
}

/// The pinned WebSocket length query, mask and decoder with the header definitions
/// they use, copied unchanged for shim/websocket.cc to compile.
fn websocket() {
    let source =
        fs::read_to_string("upstream/src/protocol/websocket.cc").expect("vendored websocket.cc");
    let header =
        fs::read_to_string("upstream/include/swoole_websocket.h").expect("vendored header");
    let constants: String = header
        .lines()
        .filter(|line| line.starts_with("#define SW_WEBSOCKET_"))
        .flat_map(|line| [line, "\n"])
        .collect();
    let text = [
        constants,
        slice(&header, "enum Flag {", "#define WEBSOCKET_VERSION"),
        slice(&header, "enum Opcode {", "enum CloseReason {"),
        slice(
            &header,
            "static inline bool is_control_frame",
            "static inline uint16_t get_ext_flags",
        ),
        slice(
            &source,
            "static ssize_t get_package_length_impl",
            "bool encode(",
        ),
        slice(&source, "bool decode(", "bool pack_close_frame("),
    ]
    .concat();
    fs::write(out("websocket.inc"), text).unwrap();
}

/// `coroutine::async`, `async(fn)`, `System::sleep`, `gethostbyname_impl_with_async` and
/// `System::getaddrinfo` from the pinned system.cc, copied unchanged for `shim/hooks_async.cc`.
fn hooks_async() {
    let text = fs::read_to_string("upstream/src/coroutine/system.cc").expect("vendored system.cc");
    let mut parts = vec![
        // Its only preprocessor branch is io_uring's, off here.
        slice(&text, "int System::sleep(", "\n}\n") + "\n}",
    ];
    for signature in [
        "static void async_task_completed(",
        "bool async(async::Handler handler,",
        "struct AsyncLambdaTask {",
        "static void async_lambda_handler(",
        "static void async_lambda_callback(",
        "bool async(const std::function<void()> &fn)",
        "std::string gethostbyname_impl_with_async(",
        "std::vector<std::string> System::getaddrinfo(",
    ] {
        let body = definition(&text, signature);
        parts.push(if signature.starts_with("struct ") {
            body + ";"
        } else {
            body
        });
    }
    fs::write(out("hooks_async.inc"), parts.join("\n") + "\n").unwrap();
}

/// A C++ build that matches the Rust side's sanitizer, so `cargo fuzz`'s `ASan`
/// also checks the upstream code.
fn cpp() -> cc::Build {
    let mut build = cc::Build::new();
    build.cpp(true).std("c++14");
    if env::var("CARGO_CFG_SANITIZE").is_ok_and(|s| s.split(',').any(|s| s == "address")) {
        build.flag("-fsanitize=address");
        // rustc links its own ASan runtime; Apple clang would require its own by name.
        if build.get_compiler().is_like_clang() {
            build
                .flag("-mllvm")
                .flag("-asan-guard-against-version-mismatch=0");
        }
    }
    build
}

/// The stream receive loops from the pinned base.cc (upstream) and candidate/'s
/// (the Rust receive with its native effects), with String's split and reduce from the pinned string.cc, copied
/// unchanged for `shim/framing_stream.cc`.
fn framing_stream() {
    let upstream = fs::read_to_string("upstream/src/protocol/base.cc").expect("vendored base.cc");
    let candidate =
        fs::read_to_string("candidate/src/protocol/base.cc").expect("candidate base.cc");
    let methods = [
        "int Protocol::recv_split_by_eof(",
        "int Protocol::recv_with_length_protocol(",
        "int Protocol::recv_with_eof_protocol(",
    ];
    for (name, text) in [("upstream", &upstream), ("candidate", &candidate)] {
        let mut parts = Vec::new();
        if text.contains("namespace {\nstruct Recv {") {
            parts.push(
                slice(text, "static_assert(offsetof(String", "}  // namespace\n")
                    + "}  // namespace\n",
            );
        }
        parts.extend(methods.iter().map(|signature| definition(text, signature)));
        fs::write(
            out(&format!("framing_stream_{name}.inc")),
            parts.join("\n") + "\n",
        )
        .unwrap();
    }
    let string = fs::read_to_string("upstream/src/core/string.cc").expect("vendored string.cc");
    let parts = [
        definition(&string, "void String::reduce("),
        // Its only preprocessor branch is a trace counter, off here.
        slice(&string, "ssize_t String::split(", "\n}\n") + "\n}\n",
    ];
    fs::write(out("framing_stream_string.inc"), parts.join("\n") + "\n").unwrap();
}

/// `redis::recv_packet` and its `Request` from the pinned redis.cc (upstream) and
/// from candidate/'s (calling Rust), copied unchanged for `shim/resp_stream.cc`.
fn resp_stream() {
    let redis = fs::read_to_string("upstream/src/protocol/redis.cc").expect("vendored redis.cc");
    let candidate =
        fs::read_to_string("candidate/src/protocol/redis.cc").expect("candidate redis.cc");
    for (name, text) in [("upstream", &redis), ("candidate", &candidate)] {
        let request = if text.contains("struct Request {") {
            slice(text, "struct Request {", "\n};") + "\n};"
        } else {
            slice(text, "using Request = ", "\n")
        };
        let mut parts = vec![request];
        if text.contains("const char *get_number(") {
            parts.push(definition(text, "const char *get_number("));
        }
        parts.push(definition(text, "int recv_packet("));
        fs::write(
            out(&format!("resp_stream_{name}.inc")),
            parts.join("\n") + "\n",
        )
        .unwrap();
    }
}

/// socks5.cc as pinned (upstream) and candidate/'s (calling Rust), each with
/// `swoole` renamed to its engine, compiled with `shim/socks5.cc` over the facade
/// in `shim/socks5`.
fn socks5() {
    for engine in ["upstream", "candidate"] {
        let source = format!("{engine}/src/protocol/socks5.cc");
        cpp()
            // Its own namespace: other shims define different swoole classes per engine.
            .define("swoole", format!("socks5_{engine}").as_str())
            .define("SWR_SOCKS5_ENGINE", engine)
            .include("shim/socks5")
            .include("upstream/include")
            .include("../../native/protocols")
            .file(source)
            .file("shim/socks5.cc")
            .compile(&format!("swoole_{engine}_socks5"));
    }
}

fn main() {
    println!("cargo:rerun-if-changed=upstream");
    println!("cargo:rerun-if-changed=candidate");
    println!("cargo:rerun-if-changed=shim");

    // The original code: base64 under `oracle::`, crc32 renamed, so both can sit
    // beside the Rust-backed `swoole::` and `swoole_crc32`.
    websocket();
    http();
    http_proxy();
    framing();
    framing_stream();
    http_frame();
    http_range();
    http_static_select();
    http_static_context();
    protocols();
    resp_stream();
    clients();
    hooks_async();
    cpp()
        .include(env::var("OUT_DIR").unwrap())
        .file("shim/hooks_async.cc")
        .compile("swoole_oracle_hooks_async");
    cpp()
        .include(env::var("OUT_DIR").unwrap())
        .file("shim/clients.cc")
        .compile("swoole_oracle_clients");
    cpp()
        .define("SWR_PROTO_ENGINE", "upstream")
        .define("SWR_PROTO_UPSTREAM", "1")
        .define("SWR_PROTO_INC", "\"protocols_upstream.inc\"")
        .flag_if_supported("-Wno-unused-parameter")
        .include("../../native/protocols")
        .include(env::var("OUT_DIR").unwrap())
        .file("shim/protocols.cc")
        .compile("swoole_oracle_protocols");
    cpp()
        .define("SWR_RANGE_ENGINE", "upstream")
        .define("SWR_RANGE_INC", "\"http_range_upstream.inc\"")
        .include("../../native/http")
        .include(env::var("OUT_DIR").unwrap())
        .file("shim/http_range.cc")
        .compile("swoole_upstream_http_range");
    cpp()
        .include(env::var("OUT_DIR").unwrap())
        .file("shim/http_static_select.cc")
        .compile("swoole_upstream_http_static_select");
    cpp()
        .include(env::var("OUT_DIR").unwrap())
        .file("shim/http_static_context.cc")
        .compile("swoole_upstream_http_static_context");
    cpp()
        .define("SWR_FRAME_ENGINE", "upstream")
        .define("SWR_FRAME_INC", "\"http_frame_upstream.inc\"")
        .include("../../native/http")
        .include(env::var("OUT_DIR").unwrap())
        .file("shim/http_frame.cc")
        .compile("swoole_oracle_http_frame");
    // Before swoole_oracle, whose base64_encode it calls: archives link in order.
    cpp()
        .define("swoole", "oracle_proxy")
        .include("upstream/include")
        .include(env::var("OUT_DIR").unwrap())
        .file("shim/http_proxy.cc")
        .compile("swoole_oracle_http_proxy");
    cpp()
        .define("swoole", "oracle")
        .define("swoole_crc32", "oracle_swoole_crc32")
        .include("shim")
        .include("upstream/include")
        .file("upstream/src/core/base64.cc")
        .file("upstream/src/core/crc32.cc")
        .file("shim/oracle.cc")
        .include(env::var("OUT_DIR").unwrap())
        .file("shim/websocket.cc")
        .file("shim/http.cc")
        .file("shim/framing.cc")
        .compile("swoole_oracle");

    for engine in ["upstream", "candidate"] {
        cpp()
            .define("SWR_STREAM_ENGINE", engine)
            .define(
                "SWR_STREAM_INC",
                format!("\"framing_stream_{engine}.inc\"").as_str(),
            )
            .flag_if_supported("-Wno-unused-parameter")
            .flag_if_supported("-Wno-unused-label")
            .include(env::var("OUT_DIR").unwrap())
            .file("shim/framing_stream.cc")
            .compile(&format!("swoole_{engine}_framing_stream"));
    }
    for engine in ["upstream", "candidate"] {
        cpp()
            .define("SWR_RESP_ENGINE", engine)
            .define(
                "SWR_RESP_INC",
                format!("\"resp_stream_{engine}.inc\"").as_str(),
            )
            .flag_if_supported("-Wno-unused-parameter")
            .flag_if_supported("-Wno-unused-label")
            .include("../../native/protocols")
            .include(env::var("OUT_DIR").unwrap())
            .file("shim/resp_stream.cc")
            .compile(&format!("swoole_{engine}_resp_stream"));
    }
    socks5();
    // The bundled third-party multipart parser the server streams uploads through.
    cc::Build::new()
        .file("upstream/thirdparty/multipart_parser.c")
        .compile("swoole_oracle_multipart_parser");
    // The bundled llhttp the HTTP server and client parse with.
    cc::Build::new()
        .files(
            ["api.c", "http.c", "llhttp.c"]
                .map(|file| format!("upstream/thirdparty/llhttp/{file}")),
        )
        .extra_warnings(false)
        .compile("swoole_oracle_llhttp");
    // The bundled nghttp2 HPACK coder the HTTP/2 server and client link.
    cc::Build::new()
        .files(
            [
                "nghttp2_hd.c",
                "nghttp2_rcbuf.c",
                "nghttp2_helper.c",
                "nghttp2_buf.c",
                "nghttp2_mem.c",
                "nghttp2_hd_huffman.c",
                "nghttp2_hd_huffman_data.c",
            ]
            .map(|file| format!("upstream/thirdparty/nghttp2/{file}")),
        )
        .extra_warnings(false)
        .compile("swoole_oracle_nghttp2");
    cpp()
        .define("SWR_PROTO_ENGINE", "candidate")
        .define("SWR_PROTO_INC", "\"protocols_candidate.inc\"")
        .include("../../native/protocols")
        .include(env::var("OUT_DIR").unwrap())
        .file("shim/protocols.cc")
        .compile("swoole_candidate_protocols");
}
