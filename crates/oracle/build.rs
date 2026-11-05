// upstream/ holds unmodified files from Swoole v6.2.4 (c7e51450); see src/lib.rs.
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

/// The part of a multi-file overlay patch that changes `path`.
fn section<'a>(patch: &'a str, path: &str) -> &'a str {
    let start = patch.find(&format!("\n+++ b/{path}\n")).expect(path) + 1;
    let end = patch[start..]
        .find("\ndiff --git ")
        .map_or(patch.len(), |end| start + end + 1);
    &patch[start..end]
}

/// An overlay patch applied to the text of its one vendored original, hunk by
/// hunk, checking every context and removed line.
fn apply(original: &str, patch: &str) -> String {
    assert_eq!(
        patch
            .lines()
            .filter(|line| line.starts_with("+++ "))
            .count(),
        1,
        "one file per patch"
    );
    let lines: Vec<&str> = original.lines().collect();
    let (mut output, mut cursor) = (Vec::new(), 0);
    let mut hunks = patch
        .lines()
        .skip_while(|line| !line.starts_with("@@ "))
        .peekable();
    while let Some(header) = hunks.next() {
        let old = header.split(' ').nth(1).unwrap().trim_start_matches('-');
        let (start, length) = old.split_once(',').unwrap_or((old, "1"));
        let (start, length): (usize, usize) = (start.parse().unwrap(), length.parse().unwrap());
        let at = if length == 0 { start } else { start - 1 };
        let mut body = Vec::new();
        while let Some(line) = hunks.next_if(|line| !line.starts_with("@@ ")) {
            if !line.starts_with('\\') {
                body.push(if line.is_empty() { " " } else { line });
            }
        }
        let old_side: Vec<&str> = body
            .iter()
            .filter(|l| !l.starts_with('+'))
            .map(|l| &l[1..])
            .collect();
        assert_eq!(
            old_side,
            lines[at..at + length],
            "patch context matches original at {header}"
        );
        output.extend_from_slice(&lines[cursor..at]);
        output.extend(body.iter().filter(|l| !l.starts_with('-')).map(|l| &l[1..]));
        cursor = at + length;
    }
    output.extend_from_slice(&lines[cursor..]);
    output.iter().flat_map(|line| [*line, "\n"]).collect()
}

/// Apply an overlay patch to its vendored original, so the adapter compiled here
/// is exactly the file the hybrid build compiles.
fn patched(original: &str, patch: &str, name: &str) -> PathBuf {
    println!("cargo:rerun-if-changed={patch}");
    let patch = fs::read_to_string(patch).expect("overlay patch");
    let original = fs::read_to_string(original).expect("vendored original");
    let path = out(name);
    fs::write(&path, apply(&original, &patch)).unwrap();
    path
}

/// One `TEST(suite, name) { ... }` from an upstream core-test file, plus the
/// file-scope `static const` fixtures it may use.
fn original_test(file: &str, test: &str, name: &str) -> PathBuf {
    let source = fs::read_to_string(file).expect("vendored core test");
    let fixtures = source
        .lines()
        .filter(|line| line.starts_with("static const "));
    let start = source.find(test).expect("original test");
    let end = start + source[start..].find("\n}\n").expect("test body") + 3;
    let text: String =
        fixtures.flat_map(|line| [line, "\n"]).collect::<String>() + &source[start..end];
    let path = out(name);
    fs::write(&path, text).unwrap();
    path
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
    ];
    fs::write(out("http_frame_enums.inc"), enums.concat()).unwrap();
}

/// `StaticHandler::parse_range` from the pinned `static_handler.cc` (upstream, for
/// `shim/http_range.cc`) and, with the http static-ranges patch applied, the Rust
/// static context's `parse_range` with `ensure_boundary` and `resume_range_header`
/// (candidate, for `shim/http_range_context.cc`), copied unchanged.
fn http_range() {
    let patch = "../../migration/patches/http/0002-static-ranges.patch";
    println!("cargo:rerun-if-changed={patch}");
    let upstream = fs::read_to_string("upstream/src/server/static_handler.cc")
        .expect("vendored static_handler.cc");
    let candidate = apply(
        &upstream,
        &fs::read_to_string(patch).expect("static-ranges patch"),
    );
    let engines = [
        (
            "upstream",
            &upstream,
            &["void StaticHandler::parse_range("][..],
        ),
        (
            "candidate",
            &candidate,
            &[
                "static int32_t resume_range_header(",
                "uint32_t StaticHandler::ensure_boundary(",
                "void StaticHandler::parse_range(",
            ][..],
        ),
    ];
    for (name, text, signatures) in engines {
        let parts: Vec<String> = signatures
            .iter()
            .map(|signature| definition(text, signature))
            .collect();
        fs::write(
            out(&format!("http_range_{name}.inc")),
            parts.join("\n") + "\n",
        )
        .unwrap();
    }
}

/// MQTT's length decoder and RESP's parse from the pinned mqtt.cc and redis.cc
/// (upstream) and from them with the protocols overlay patch applied (candidate,
/// calling Rust), copied unchanged for shim/protocols.cc, with redis's states.
fn protocols() {
    let patch = "../../migration/patches/protocols/0001-rust-packet-protocols.patch";
    println!("cargo:rerun-if-changed={patch}");
    let patch = fs::read_to_string(patch).expect("protocols patch");
    let mqtt = fs::read_to_string("upstream/src/protocol/mqtt.cc").expect("vendored mqtt.cc");
    let redis = fs::read_to_string("upstream/src/protocol/redis.cc").expect("vendored redis.cc");
    let sources = [
        ("upstream", mqtt.clone(), redis.clone()),
        (
            "candidate",
            apply(&mqtt, section(&patch, "src/protocol/mqtt.cc")),
            apply(&redis, section(&patch, "src/protocol/redis.cc")),
        ),
    ];
    for (name, mqtt, redis) in sources {
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

/// `Address::assign(url)`, the Set-Cookie split and the DNS name decode from the
/// pinned sources, copied unchanged for shim/clients.cc.
fn clients() {
    let address =
        fs::read_to_string("upstream/src/network/address.cc").expect("vendored address.cc");
    fs::write(
        out("clients_url.inc"),
        definition(&address, "bool Address::assign(const std::string &url)") + "\n",
    )
    .unwrap();
    let dns = fs::read_to_string("upstream/src/network/dns.cc").expect("vendored dns.cc");
    fs::write(
        out("clients_dns.inc"),
        definition(&dns, "static void domain_decode(char *str) {") + "\n",
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

/// The stream receive loops from the pinned base.cc (upstream) and from it with
/// the framing overlay patch applied (candidate, the Rust receive with its
/// native effects), with String's split and reduce from the pinned string.cc, copied
/// unchanged for `shim/framing_stream.cc`.
fn framing_stream() {
    let patch = "../../migration/patches/framing/0001-base.patch";
    println!("cargo:rerun-if-changed={patch}");
    let upstream = fs::read_to_string("upstream/src/protocol/base.cc").expect("vendored base.cc");
    let candidate = apply(
        &upstream,
        &fs::read_to_string(patch).expect("framing patch"),
    );
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
/// from it with the protocols overlay patch applied (candidate, calling Rust),
/// copied unchanged for `shim/resp_stream.cc`.
fn resp_stream() {
    let patch = "../../migration/patches/protocols/0001-rust-packet-protocols.patch";
    let redis = fs::read_to_string("upstream/src/protocol/redis.cc").expect("vendored redis.cc");
    let candidate = apply(
        &redis,
        section(
            &fs::read_to_string(patch).expect("protocols patch"),
            "src/protocol/redis.cc",
        ),
    );
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

/// socks5.cc as pinned (upstream) and with the protocols overlay patch applied
/// (candidate, calling Rust), each with `swoole` renamed to its engine, compiled
/// with `shim/socks5.cc` over the facade in `shim/socks5`.
fn socks5() {
    let socks5 = fs::read_to_string("upstream/src/protocol/socks5.cc").expect("vendored socks5.cc");
    let patch =
        fs::read_to_string("../../migration/patches/protocols/0001-rust-packet-protocols.patch")
            .expect("protocols patch");
    fs::write(
        out("socks5_candidate.cc"),
        apply(&socks5, section(&patch, "src/protocol/socks5.cc")),
    )
    .unwrap();
    for (engine, source) in [
        ("upstream", PathBuf::from("upstream/src/protocol/socks5.cc")),
        ("candidate", out("socks5_candidate.cc")),
    ] {
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
    println!("cargo:rerun-if-changed=shim");

    // The original code: base64 under `oracle::`, crc32 renamed, so both can sit
    // beside the Rust-backed `swoole::` and `swoole_crc32`.
    websocket();
    http();
    framing();
    framing_stream();
    http_frame();
    http_range();
    protocols();
    resp_stream();
    clients();
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
        .include("../../native/http")
        .include(env::var("OUT_DIR").unwrap())
        .file("shim/http_range_context.cc")
        .compile("swoole_candidate_http_range");
    cpp()
        .define("SWR_FRAME_ENGINE", "upstream")
        .define("SWR_FRAME_INC", "\"http_frame_upstream.inc\"")
        .include("../../native/http")
        .include(env::var("OUT_DIR").unwrap())
        .file("shim/http_frame.cc")
        .compile("swoole_oracle_http_frame");
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

    // The original core-test assertions, run against the adapters that forward
    // to the Rust exports. Those symbols are resolved by the test binary.
    original_test(
        "upstream/core-tests/src/core/hash.cpp",
        "TEST(hash, crc32)",
        "crc32_test.cpp",
    );
    cpp()
        .include("shim")
        .include("upstream")
        .include("upstream/include")
        .include("../../native/include")
        .include(env::var("OUT_DIR").unwrap())
        .file(patched(
            "upstream/src/core/base64.cc",
            "../../migration/patches/base64/0001-rust-base64.patch",
            "base64_adapter.cc",
        ))
        .file(patched(
            "upstream/src/core/crc32.cc",
            "../../migration/patches/crc32/0001-rust-crc32.patch",
            "crc32_adapter.cc",
        ))
        .file("shim/original.cc")
        .file("shim/original_crc32.cc")
        .compile("swoole_original_tests");

    for engine in ["upstream", "candidate"] {
        cpp()
            .define("SWR_STREAM_ENGINE", engine)
            .define(
                "SWR_STREAM_INC",
                format!("\"framing_stream_{engine}.inc\"").as_str(),
            )
            .flag_if_supported("-Wno-unused-parameter")
            .flag_if_supported("-Wno-unused-label")
            .include("../../native/framing")
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
    cpp()
        .define("SWR_PROTO_ENGINE", "candidate")
        .define("SWR_PROTO_INC", "\"protocols_candidate.inc\"")
        .include("../../native/protocols")
        .include(env::var("OUT_DIR").unwrap())
        .file("shim/protocols.cc")
        .compile("swoole_candidate_protocols");
}
