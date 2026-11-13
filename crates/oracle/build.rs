// upstream/ holds unmodified files from Swoole v6.2.5 (76c657d8); see src/lib.rs. candidate/
// holds base.cc as the hybrid shipped it before Rust replaced the file outright: its
// bodies call Rust, so its oracle runs Rust against upstream through the same C++.
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

/// `get_method` and `parse_multipart_boundary` from the pinned http.cc with its method
/// names, and `swoole_strcaseeq` and `swoole_str_istarts_with` from the pinned swoole.h
/// with the two macros that call them, copied unchanged for `shim/http_locale.cc`.
fn http_locale() {
    let http = fs::read_to_string("upstream/src/protocol/http.cc").expect("vendored http.cc");
    let header = fs::read_to_string("upstream/include/swoole.h").expect("vendored swoole.h");
    let mut parts: Vec<String> = header
        .lines()
        .filter(|line| {
            line.starts_with("#define SW_STRL(") || line.starts_with("#define SW_STR_ISTARTS_WITH(")
        })
        .map(str::to_string)
        .collect();
    parts.extend([
        definition(&header, "static inline unsigned int swoole_strcaseeq("),
        definition(
            &header,
            "static inline unsigned int swoole_str_istarts_with(",
        ),
        slice(&http, "static const char *method_strings[]", "\n};") + "\n};",
        definition(&http, "int get_method("),
        definition(&http, "bool parse_multipart_boundary("),
    ]);
    fs::write(out("http_locale.inc"), parts.join("\n") + "\n").unwrap();
}

/// The port callback enum from the pinned `php_swoole_server.h`, the event catalog and
/// `getCallback` from the pinned `swoole_server_port.cc`, and `swoole_strcaseeq` from
/// the pinned swoole.h, copied unchanged for `shim/ports.cc`.
fn ports() {
    let header = fs::read_to_string("upstream/ext-src/php_swoole_server.h")
        .expect("vendored php_swoole_server.h");
    let port = fs::read_to_string("upstream/ext-src/swoole_server_port.cc")
        .expect("vendored swoole_server_port.cc");
    let swoole = fs::read_to_string("upstream/include/swoole.h").expect("vendored swoole.h");
    let catalog = [
        slice(&header, "enum php_swoole_server_port_callback_type {", "};") + "};",
        slice(&port, "struct ServerPortEvent {", "// clang-format on"),
        definition(&swoole, "static inline unsigned int swoole_strcaseeq("),
    ];
    fs::write(out("ports_catalog.inc"), catalog.join("\n") + "\n").unwrap();
    let getter = definition(
        &port,
        "static PHP_METHOD(swoole_server_port, getCallback) {",
    );
    fs::write(out("ports_get_callback.inc"), getter + "\n").unwrap();
    cpp()
        .include(env::var("OUT_DIR").unwrap())
        .file("shim/ports.cc")
        .compile("swoole_upstream_ports");
}

/// `Server::add_rewrite_rule` and `apply_rewrite_rules` from the pinned
/// `static_handler.cc`, copied unchanged for `shim/http_rewrite.cc`.
fn http_rewrite() {
    let handler = fs::read_to_string("upstream/src/server/static_handler.cc")
        .expect("vendored static_handler.cc");
    fs::write(
        out("http_rewrite.inc"),
        definition(&handler, "void Server::add_rewrite_rule(")
            + "\n"
            + &definition(&handler, "bool Server::apply_rewrite_rules(")
            + "\n",
    )
    .unwrap();
}

/// MQTT's length decoder and RESP's `get_number`, `parse` and `format` from the
/// pinned mqtt.cc and redis.cc, and the request loop of the pinned PHP redis server,
/// copied unchanged for shim/protocols.cc, with redis's states and reply types.
fn protocols() {
    let mqtt = fs::read_to_string("upstream/src/protocol/mqtt.cc").expect("vendored mqtt.cc");
    let redis = fs::read_to_string("upstream/src/protocol/redis.cc").expect("vendored redis.cc");
    let parts = [
        slice(&mqtt, "#define SW_MQTT_RECV_LEN_AGAIN", "\n") + "\n",
        definition(&mqtt, "ssize_t get_package_length("),
        definition(&redis, "const char *get_number("),
        definition(
            &redis,
            "void format(String *buf, ReplyType type, const std::string &value)",
        ),
        definition(
            &redis,
            "void format(String *buf, ReplyType type, long value)",
        ),
        definition(&redis, "std::vector<std::string> parse("),
    ];
    fs::write(out("protocols_upstream.inc"), parts.join("\n") + "\n").unwrap();
    let header =
        fs::read_to_string("upstream/include/swoole_redis.h").expect("vendored swoole_redis.h");
    fs::write(
        out("protocols_states.inc"),
        slice(&header, "enum State {", "const char *get_number"),
    )
    .unwrap();
    let server = fs::read_to_string("upstream/ext-src/swoole_redis_server.cc")
        .expect("vendored swoole_redis_server.cc");
    fs::write(
        out("protocols_php_request.inc"),
        slice(
            &server,
            "    const char *p = Z_STRVAL(zdata);",
            "\n    if (command_len >=",
        ) + "\n",
    )
    .unwrap();
}

/// The `Address` methods, the hosts lookup, the Set-Cookie split, the DNS name
/// codec, the socket DNS lookup and the request formats with `sw_snprintf` from the
/// pinned sources, copied unchanged for shim/clients.cc.
fn clients() {
    let config =
        fs::read_to_string("upstream/include/swoole_config.h").expect("vendored swoole_config.h");
    let base = fs::read_to_string("upstream/src/core/base.cc").expect("vendored base.cc");
    fs::write(
        out("clients_http_format.inc"),
        [
            slice(
                &config,
                "#define SW_HTTP_CLIENT_BOUNDARY_TOTAL_SIZE",
                "#define SW_HTTP_FORM_FILE_DATA_FMT_LEN",
            ),
            slice(&base, "size_t sw_snprintf(", "size_t sw_vsnprintf("),
        ]
        .concat(),
    )
    .unwrap();
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

/// The pinned WebSocket length query, mask, encoder, decoder and close packer with the
/// header definitions they use, and the PHP server's pack and message policy with the
/// two bit macros it uses, copied unchanged for shim/websocket.cc to compile.
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
            "void print_frame(",
        ),
    ]
    .concat();
    let server = fs::read_to_string("upstream/ext-src/swoole_websocket_server.cc")
        .expect("vendored swoole_websocket_server.cc");
    let bits: String = fs::read_to_string("upstream/include/swoole.h")
        .expect("vendored swoole.h")
        .lines()
        .filter(|line| {
            line.starts_with("#define sw_unset_bit(") || line.starts_with("#define sw_set_bit(")
        })
        .flat_map(|line| [line, "\n"])
        .collect();
    fs::write(out("websocket_bits.inc"), bits).unwrap();
    fs::write(
        out("websocket_pack.inc"),
        slice(
            &server,
            "    bool need_compress =",
            "    if (need_compress)",
        ),
    )
    .unwrap();
    // The condition alone, without its `if (` and `) {`.
    let drop = slice(&server, "    if ((opcode == WebSocket::OPCODE_CLOSE", ") {");
    fs::write(
        out("websocket_drop.inc"),
        drop.strip_prefix("    if (").unwrap(),
    )
    .unwrap();
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

/// `swoole_type_size`, `swoole_dec2hex`, `swoole_hex2dec`, `swoole_version_compare`,
/// `swoole_common_divisor`, `swoole_common_multiple`, `swoole_itoa` and `swoole_get_env`
/// from the pinned base.cc, copied unchanged for `shim/corebase.cc`.
fn corebase() {
    let base = fs::read_to_string("upstream/src/core/base.cc").expect("vendored base.cc");
    let text = [
        slice(&base, "int swoole_type_size(char type)", "#ifndef RAND_MAX"),
        slice(&base, "int swoole_version_compare(", "size_t sw_snprintf("),
        slice(&base, "int swoole_itoa(", "int swoole_shell_exec("),
        slice(
            &base,
            "bool swoole_get_env(",
            "int swoole_get_systemd_listen_fds(",
        ),
    ]
    .concat();
    fs::write(out("corebase.inc"), text).unwrap();
}

/// The hashes from the pinned misc.cc, and from log.cc `get_pretty_name` (as `pretty`)
/// and the level clamp, rotation format, level label, line and microsecond formatting
/// out of `set_level`, `gen_real_file` and `put` wrapped as functions, with base.cc's
/// `sw_snprintf` and `swoole_log.h`'s enums, each copied unchanged for `shim/utilities.cc`.
fn utilities() {
    let misc = fs::read_to_string("upstream/src/core/misc.cc").expect("vendored misc.cc");
    let log = fs::read_to_string("upstream/src/core/log.cc").expect("vendored log.cc");
    let base = fs::read_to_string("upstream/src/core/base.cc").expect("vendored base.cc");
    let header =
        fs::read_to_string("upstream/include/swoole_log.h").expect("vendored swoole_log.h");
    let after = |anchor: &str| &log[log.find(anchor).expect(anchor)..];
    let text = [
        misc[misc.find("uint64_t swoole_hash_php").expect("hashes")..].to_string(),
        slice(&log, "std::string Logger::get_pretty_name", "bool Logger::open")
            .replace("Logger::get_pretty_name", "pretty"),
        slice(&header, "enum swLogLevel", "namespace swoole"),
        "\nint log_level(int level) { int log_level;\n".into(),
        slice(after("void Logger::set_level"), "    if (level < SW_LOG_DEBUG)", "\n}"),
        "\nreturn log_level; }\nconst char *log_rotation(int log_rotation) {\n".into(),
        slice(after("std::string Logger::gen_real_file"), "    const char *fmt;", "    size_t l_data_str"),
        "\nreturn fmt; }\nconst char *log_label(int level) { const char *level_str;\n".into(),
        slice(after("void Logger::put"), "    switch (level)", "    auto now ="),
        "\nreturn level_str; }\n#define sw_unlikely(x) (x)\n".into(),
        slice(&base, "size_t sw_snprintf(", "size_t sw_vsnprintf("),
        "\n#undef sw_unlikely\n#define SW_LOG_BUFFER_SIZE capacity\nsize_t log_line(char *log_str, size_t capacity, const char *date_str, int l_data_str, char worker_symbol, int worker_pid, int worker_id, int level, const char *content, int length) { const char *level_str = log_label(level);\n".into(),
        slice(after("    size_t n = sw_snprintf(log_str,"), "    size_t n", "    lock.lock();"),
        "\nreturn n; }\n#undef SW_LOG_BUFFER_SIZE\n#define SW_LOG_DATE_STRLEN capacity\nsize_t log_micros(char *date_str, size_t capacity, long long now_us) { long long now_sec = 0; size_t l_data_str = 0;\n".into(),
        slice(after("        l_data_str += sw_snprintf("), "        l_data_str", "    }"),
        "\nreturn l_data_str; }\n#undef SW_LOG_DATE_STRLEN\n".into(),
    ]
    .concat();
    fs::write(out("utilities.inc"), text).unwrap();
}

/// The pinned `swoole_lock.h` and the mutex, rwlock, spinlock and barrier bodies with
/// misc.cc's atomic wait, base.cc's `sw_wait_for` and timer.cc's realtime clock, each
/// copied unchanged but for its namespace (`locks_upstream`) and compiled with
/// `shim/locks.cc` under what configure finds on this target.
fn locks() {
    let read = |path: &str| fs::read_to_string(format!("upstream/{path}")).expect(path);
    let renamed = |text: String| text.replace("namespace swoole", "namespace locks_upstream");
    let header = read("include/swoole_lock.h").replace("#include \"swoole_memory.h\"\n", "");
    fs::write(out("locks_upstream.h"), renamed(header)).unwrap();
    // The futex headers come out of the namespace.
    let kernel = ["#include <linux/futex.h>\n", "#include <sys/syscall.h>\n"];
    let mut text = format!(
        "#ifdef HAVE_FUTEX\n{}{}#endif\nnamespace locks_upstream {{\n",
        kernel[0], kernel[1]
    );
    text += &slice(
        &read("src/core/misc.cc"),
        "#ifdef HAVE_FUTEX",
        "/* {{{ DJBX33A",
    )
    .replace(kernel[0], "")
    .replace(kernel[1], "");
    text += &slice(
        &read("src/core/base.cc"),
        "bool sw_wait_for(",
        "\nint swoole_itoa",
    );
    text += &slice(
        &read("src/os/timer.cc"),
        "void realtime_get(",
        "}  // namespace swoole",
    );
    text += "}\n";
    for name in ["mutex", "rw_lock", "spin_lock", "barrier"] {
        let body = read(&format!("src/lock/{name}.cc"));
        text += &renamed(body.replace("#include \"swoole_lock.h\"\n", ""));
    }
    fs::write(out("locks_upstream.inc"), text).unwrap();
    let mut build = cpp();
    build.define("HAVE_RWLOCK", None);
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        for found in [
            "HAVE_SPINLOCK",
            "HAVE_FUTEX",
            "HAVE_PTHREAD_BARRIER",
            "HAVE_MUTEX_TIMEDLOCK",
            "HAVE_RWLOCK_TIMEDRDLOCK",
        ] {
            build.define(found, None);
        }
    }
    build
        .flag_if_supported("-Wno-unused-parameter")
        .include(env::var("OUT_DIR").unwrap())
        .file("shim/locks.cc")
        .compile("swoole_oracle_locks");
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

/// `redis::recv_packet` and its `Request` from the pinned redis.cc, copied unchanged
/// for `shim/resp_stream.cc`.
fn resp_stream() {
    let redis = fs::read_to_string("upstream/src/protocol/redis.cc").expect("vendored redis.cc");
    let parts = [
        slice(&redis, "struct Request {", "\n};") + "\n};",
        definition(&redis, "const char *get_number("),
        definition(&redis, "int recv_packet("),
    ];
    fs::write(out("resp_stream_upstream.inc"), parts.join("\n") + "\n").unwrap();
}

/// The pinned socks5.cc with `swoole` renamed, compiled with `shim/socks5.cc` over
/// the facade in `shim/socks5`.
fn socks5() {
    cpp()
        // Its own namespace: other shims define different swoole classes.
        .define("swoole", "socks5_upstream")
        .include("shim/socks5")
        .include("upstream/include")
        .file("upstream/src/protocol/socks5.cc")
        .file("shim/socks5.cc")
        .compile("swoole_upstream_socks5");
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
    http_locale();
    http_rewrite();
    ports();
    protocols();
    resp_stream();
    clients();
    hooks_async();
    utilities();
    corebase();
    locks();
    // The hashes promote `char`, so each signedness is its own engine.
    for engine in ["signed", "unsigned"] {
        cpp()
            .define("SWR_UTILS_ENGINE", format!("{engine}_char").as_str())
            .flag(format!("-f{engine}-char"))
            .flag_if_supported("-Wno-implicit-fallthrough")
            .include(env::var("OUT_DIR").unwrap())
            .file("shim/utilities.cc")
            .compile(&format!("swoole_oracle_utilities_{engine}"));
    }
    cpp()
        .include(env::var("OUT_DIR").unwrap())
        .file("shim/corebase.cc")
        .compile("swoole_oracle_corebase");
    cpp()
        .include(env::var("OUT_DIR").unwrap())
        .file("shim/hooks_async.cc")
        .compile("swoole_oracle_hooks_async");
    cpp()
        .include(env::var("OUT_DIR").unwrap())
        .file("shim/clients.cc")
        .compile("swoole_oracle_clients");
    cpp()
        .flag_if_supported("-Wno-unused-parameter")
        .flag_if_supported("-Wno-implicit-fallthrough")
        .include(env::var("OUT_DIR").unwrap())
        .file("shim/protocols.cc")
        .compile("swoole_oracle_protocols");
    cpp()
        .define("SWR_RANGE_ENGINE", "upstream")
        .define("SWR_RANGE_INC", "\"http_range_upstream.inc\"")
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
        .include(env::var("OUT_DIR").unwrap())
        .file("shim/http_locale.cc")
        .file("shim/http_rewrite.cc")
        .compile("swoole_upstream_http_gates");
    cpp()
        .define("SWR_FRAME_ENGINE", "upstream")
        .define("SWR_FRAME_INC", "\"http_frame_upstream.inc\"")
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
    cpp()
        .flag_if_supported("-Wno-unused-parameter")
        .flag_if_supported("-Wno-unused-label")
        .include(env::var("OUT_DIR").unwrap())
        .file("shim/resp_stream.cc")
        .compile("swoole_upstream_resp_stream");
    socks5();
    // The bundled llhttp the HTTP server and client parse with.
    cc::Build::new()
        .files(
            ["api.c", "http.c", "llhttp.c"]
                .map(|file| format!("upstream/thirdparty/llhttp/{file}")),
        )
        .extra_warnings(false)
        .compile("swoole_oracle_llhttp");
    // Upstream's bundled nghttp2 HPACK coder, the reference crates/http2's hpack is tested against.
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
}
