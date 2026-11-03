//! Pinned upstream C++ for differential tests. Add a module's original files to
//! `upstream/` unchanged, compile them in `build.rs`, and declare them here.
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]

pub mod base64 {
    unsafe extern "C" {
        /// Upstream `swoole::base64_encode`. Undefined for zero length.
        #[link_name = "swr_oracle_base64_encode"]
        pub fn encode(input: *const u8, length: usize, output: *mut u8) -> usize;
        /// Upstream `swoole::base64_decode`.
        #[link_name = "swr_oracle_base64_decode"]
        pub fn decode(input: *const u8, length: usize, output: *mut u8) -> usize;
        /// Runs upstream's `core-tests/src/protocol/base64.cpp` against the native
        /// adapter; returns the number of failed assertions. The caller's binary
        /// must link `swr_v1_base64_*`.
        #[link_name = "swr_original_base64_failures"]
        pub fn original_test_failures() -> i32;
    }
}

pub mod crc32 {
    unsafe extern "C" {
        /// Upstream `swoole_crc32`.
        #[link_name = "swr_oracle_crc32"]
        pub fn crc32(data: *const u8, size: usize) -> u32;
        /// Runs upstream's `TEST(hash, crc32)` from `core-tests/src/core/hash.cpp`
        /// against the native adapter; returns the number of failed assertions.
        /// The caller's binary must link `swr_v1_crc32`.
        #[link_name = "swr_original_crc32_failures"]
        pub fn original_test_failures() -> i32;
    }
}

pub mod websocket {
    /// Fields of upstream's `websocket::Frame` after `decode`; flags as `Frame::get_flags()`.
    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default, PartialEq)]
    pub struct Frame {
        pub payload_length: u64,
        pub header_length: u16,
        pub opcode: u8,
        pub flags: u8,
        pub length_code: u8,
        pub mask_key: [u8; 4],
    }

    unsafe extern "C" {
        /// Upstream `get_package_length_impl` over `length` bytes; `header_length`
        /// is the `PacketLength::header_len` it starts from and leaves.
        #[link_name = "swr_oracle_ws_frame_length"]
        pub fn frame_length(data: *const u8, length: usize, header_length: *mut u32) -> isize;
        /// Upstream `websocket::decode`, which unmasks the payload in place.
        #[link_name = "swr_oracle_ws_decode"]
        pub fn decode(data: *mut u8, length: usize, frame: *mut Frame) -> bool;
        /// Upstream `websocket::mask`.
        #[link_name = "swr_oracle_ws_mask"]
        pub fn mask(data: *mut u8, length: usize, key: *const u8);
    }
}

pub mod http {
    unsafe extern "C" {
        /// Upstream `http_server::url_decode`, in place over `length + 1` bytes
        /// (it writes a terminator); returns the decoded length.
        #[link_name = "swr_oracle_http_url_decode"]
        pub fn url_decode(data: *mut u8, length: usize) -> usize;
        /// Upstream `http_server::url_encode`, copied with its NUL into `output`
        /// (capacity `3 * length + 1`); returns the encoded length.
        #[link_name = "swr_oracle_http_url_encode"]
        pub fn url_encode(data: *const u8, length: usize, output: *mut u8) -> usize;
        /// Every `http_server::parse_cookie` callback as u32 key length, key,
        /// u32 value length, value, into `output` (capacity `8 * length + 16`).
        #[link_name = "swr_oracle_http_cookies"]
        pub fn cookies(data: *const u8, length: usize, output: *mut u8) -> usize;
    }
}

pub mod framing {
    unsafe extern "C" {
        /// Upstream `swoole_unpack`: reads the format's width from `data`.
        #[link_name = "swr_oracle_framing_unpack"]
        pub fn unpack(format: u8, data: *const u8) -> i64;
        /// Upstream `Protocol::default_length_func` for this length type, field
        /// size, field offset and body offset; `header_length` is what it leaves
        /// in `PacketLength::header_len`.
        #[link_name = "swr_oracle_framing_length"]
        pub fn length(
            format: u8,
            size: u8,
            offset: u16,
            body_offset: u16,
            data: *const u8,
            length: u32,
            header_length: *mut u32,
        ) -> isize;
    }
}

pub mod http_frame {
    use std::ffi::c_char;

    /// The `Request` fields request framing reads and writes, and the buffer offset.
    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default, PartialEq)]
    pub struct State {
        pub offset: usize,
        pub content_length: u64,
        pub method: u32,
        pub version: u32,
        pub excepted: u32,
        pub known_length: u32,
        pub url_offset: u32,
        pub url_length: u32,
        pub line_length: u32,
        pub header_length: u32,
    }

    /// Request framing from one engine: `UPSTREAM` is the pinned http.cc; a candidate
    /// supplies the same entry points (fuzz/src/lib.rs runs `swoole_rs_http::Request`).
    pub struct Engine {
        /// `operation` 0 `get_protocol`, 1 `get_header_length`, 2
        /// `get_chunked_body_length` over `length` bytes followed by a NUL;
        /// returns `SW_OK` (0) or `SW_ERR` (-1).
        pub frame: unsafe extern "C" fn(u32, *mut c_char, usize, *mut State) -> i32,
        /// `Request::has_expect_header` over `length` bytes followed by a NUL.
        pub expect: unsafe extern "C" fn(*mut c_char, usize) -> bool,
        /// `Request::get_header(name)` with this url offset, url length and header
        /// length, copied into `output` (capacity `length`); returns its size.
        pub header: unsafe extern "C" fn(
            *mut c_char,
            usize,
            u32,
            u32,
            u32,
            *const c_char,
            *mut c_char,
        ) -> usize,
        /// `get_method_string`.
        pub method: unsafe extern "C" fn(i32) -> *const c_char,
        /// `Request::parse_header_info` over `length` bytes followed by a NUL, with this
        /// request line length and header length, from errno 0.
        pub header_info: unsafe extern "C" fn(*mut c_char, usize, u32, u32, *mut HeaderInfo),
    }

    /// The request after `parse_header_info`. Flags: `known_length` 1, `keep_alive`
    /// 2, `chunked` 4, `nobody_chunked` 8, `header_parsed` 16, `excepted` 32. Without
    /// a multipart Content-Type, `boundary_offset` is -1.
    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct HeaderInfo {
        pub content_length: u64,
        pub flags: u32,
        /// errno afterwards.
        pub error: u32,
        pub boundary_offset: i64,
        pub boundary_length: i64,
    }

    unsafe extern "C" {
        fn swr_oracle_http_frame_upstream(
            operation: u32,
            data: *mut c_char,
            length: usize,
            state: *mut State,
        ) -> i32;
        fn swr_oracle_http_expect_upstream(data: *mut c_char, length: usize) -> bool;
        fn swr_oracle_http_header_upstream(
            data: *mut c_char,
            length: usize,
            url_offset: u32,
            url_length: u32,
            header_length: u32,
            name: *const c_char,
            output: *mut c_char,
        ) -> usize;
        fn swr_oracle_http_method_upstream(method: i32) -> *const c_char;
        fn swr_oracle_http_header_info_upstream(
            data: *mut c_char,
            length: usize,
            line_length: u32,
            header_length: u32,
            out: *mut HeaderInfo,
        );
    }

    pub const UPSTREAM: Engine = Engine {
        frame: swr_oracle_http_frame_upstream,
        expect: swr_oracle_http_expect_upstream,
        header: swr_oracle_http_header_upstream,
        method: swr_oracle_http_method_upstream,
        header_info: swr_oracle_http_header_info_upstream,
    };
}

pub mod framing_stream {
    /// One fuzz case for the stream receive loops: `mode` 0 is the length
    /// protocol, 1 EOF and 2 EOF split; each recv returns at most the next of
    /// `chunk_count` chunk sizes of the `length` stream bytes, then EAGAIN (or the
    /// peer's close when `close_at_end`).
    #[repr(C)]
    pub struct Stream {
        pub mode: u8,
        pub eof_len: u8,
        pub close_at_end: u8,
        pub length_size: u8,
        pub length_offset: u16,
        pub body_offset: u16,
        pub max_length: u32,
        pub buffer_size: u32,
        pub eof: *const u8,
        pub data: *const u8,
        pub length: usize,
        pub chunks: *const u16,
        pub chunk_count: usize,
    }

    /// Runs one engine's receive loop over the stream until it fails or the
    /// chunks are drained, writing each call's result (`R<n>;`), each packet
    /// (`P<len>:` and its bytes) and the final buffer length and offset to `out`.
    /// Returns the log's full length. `upstream` is the pinned base.cc,
    /// `candidate` the one with the framing overlay patch, which calls the Rust
    /// exports (the caller's binary must link them).
    pub type Run =
        unsafe extern "C" fn(stream: *const Stream, out: *mut u8, capacity: usize) -> usize;

    unsafe extern "C" {
        fn swr_oracle_framing_stream_upstream(
            stream: *const Stream,
            out: *mut u8,
            capacity: usize,
        ) -> usize;
        fn swr_oracle_framing_stream_candidate(
            stream: *const Stream,
            out: *mut u8,
            capacity: usize,
        ) -> usize;
    }

    pub const UPSTREAM: Run = swr_oracle_framing_stream_upstream;
    pub const CANDIDATE: Run = swr_oracle_framing_stream_candidate;
}

pub mod resp_stream {
    /// One fuzz case for `redis::recv_packet`: each recv returns at most the next
    /// of `chunk_count` chunk sizes of the `length` stream bytes, then EAGAIN (or
    /// the peer's close when `close_at_end`), into a buffer of `buffer_size` bytes
    /// that grows to `max_length`.
    #[repr(C)]
    pub struct Stream {
        pub data: *const u8,
        pub length: usize,
        pub chunks: *const u16,
        pub chunk_count: usize,
        pub buffer_size: u32,
        pub max_length: u32,
        pub close_at_end: u8,
    }

    /// Runs one engine's `recv_packet` over the stream until it fails or the
    /// chunks are drained, writing each call's result and request state, each
    /// package and the final buffer length and offset to `out`. Returns the log's
    /// full length. `upstream` is the pinned redis.cc, `candidate` the one with the
    /// protocols overlay patch, which calls the Rust exports (the caller's binary
    /// must link them).
    pub type Run =
        unsafe extern "C" fn(stream: *const Stream, out: *mut u8, capacity: usize) -> usize;

    unsafe extern "C" {
        fn swr_oracle_resp_stream_upstream(
            stream: *const Stream,
            out: *mut u8,
            capacity: usize,
        ) -> usize;
        fn swr_oracle_resp_stream_candidate(
            stream: *const Stream,
            out: *mut u8,
            capacity: usize,
        ) -> usize;
    }

    pub const UPSTREAM: Run = swr_oracle_resp_stream_upstream;
    pub const CANDIDATE: Run = swr_oracle_resp_stream_candidate;
}

pub mod socks5 {
    /// One fuzz case for `Socks5Proxy`: its settings (`socket_type` as upstream's
    /// `SocketType`) and the server's replies, `reply_sizes[i]` bytes each, back to
    /// back in `replies`. With `send_short` every send writes one byte too few.
    #[repr(C)]
    pub struct Case {
        pub user: *const u8,
        pub user_length: usize,
        pub password: *const u8,
        pub password_length: usize,
        pub target_host: *const u8,
        pub target_host_length: usize,
        pub target_port: u16,
        pub dns_tunnel: u8,
        pub socket_type: u8,
        pub send_short: u8,
        pub replies: *const u8,
        pub reply_sizes: *const u16,
        pub reply_count: usize,
    }

    /// Runs one engine's `Socks5Proxy` from `create` through the replies, writing
    /// every byte sent, each handshake result, state and method, and each reply
    /// code's `strerror` to `out`. Returns the log's full length. `upstream` is the
    /// pinned socks5.cc, `candidate` the one with the protocols overlay patch,
    /// which calls the Rust exports (the caller's binary must link them).
    pub type Run = unsafe extern "C" fn(case: *const Case, out: *mut u8, capacity: usize) -> usize;

    unsafe extern "C" {
        fn swr_oracle_socks5_upstream(case: *const Case, out: *mut u8, capacity: usize) -> usize;
        fn swr_oracle_socks5_candidate(case: *const Case, out: *mut u8, capacity: usize) -> usize;
    }

    pub const UPSTREAM: Run = swr_oracle_socks5_upstream;
    pub const CANDIDATE: Run = swr_oracle_socks5_candidate;
}

pub mod http_range {
    use std::ffi::c_char;

    /// Bytes per task in `parse_range`'s task output: offset and length (u64 each),
    /// then the part header, NUL-padded to `SW_HTTP_SERVER_PART_HEADER`.
    pub const TASK_SIZE: usize = 16 + 256;

    /// The handler after `parse_range`.
    #[repr(C)]
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Outcome {
        pub content_length: usize,
        pub status_code: i32,
        /// 1 if the body threw.
        pub thrown: i32,
        pub tasks: usize,
        pub end_part_length: usize,
        /// The first 128 bytes of `end_part`.
        pub end_part: [u8; 128],
    }

    impl Default for Outcome {
        fn default() -> Self {
            Self {
                content_length: 0,
                status_code: 0,
                thrown: 0,
                tasks: 0,
                end_part_length: 0,
                end_part: [0; 128],
            }
        }
    }

    /// `StaticHandler::parse_range(range, if_range ? "" : nullptr)` compiled from one
    /// engine's `static_handler.cc` (`upstream` pinned, `candidate` with the http
    /// static-ranges patch, over the Rust static context), on a file of `filesize`
    /// bytes with this MIME type and boundary, `is_modified_range` answering
    /// `modified`. Up to `capacity` tasks go to `tasks` ([`TASK_SIZE`] bytes each).
    pub type ParseRange = unsafe extern "C" fn(
        range: *const c_char,
        filesize: u64,
        mime: *const c_char,
        boundary: *const c_char,
        if_range: i32,
        modified: i32,
        out: *mut Outcome,
        tasks: *mut u8,
        capacity: usize,
    );

    unsafe extern "C" {
        fn swr_oracle_http_range_upstream(
            range: *const c_char,
            filesize: u64,
            mime: *const c_char,
            boundary: *const c_char,
            if_range: i32,
            modified: i32,
            out: *mut Outcome,
            tasks: *mut u8,
            capacity: usize,
        );
        fn swr_oracle_http_range_candidate(
            range: *const c_char,
            filesize: u64,
            mime: *const c_char,
            boundary: *const c_char,
            if_range: i32,
            modified: i32,
            out: *mut Outcome,
            tasks: *mut u8,
            capacity: usize,
        );
    }

    pub const UPSTREAM: ParseRange = swr_oracle_http_range_upstream;
    pub const CANDIDATE: ParseRange = swr_oracle_http_range_candidate;
}

pub mod protocols {
    use std::ffi::c_char;

    /// MQTT's length decoder and RESP's parse compiled from one engine's sources:
    /// `upstream` is the pinned file, `candidate` the file with the protocols overlay
    /// patch, which calls the Rust exports (the caller's binary must link them).
    pub struct Engine {
        /// `mqtt::get_package_length` over `size` bytes.
        pub mqtt_length: unsafe extern "C" fn(*const c_char, u32) -> i64,
        /// `redis::parse` over `length` bytes followed by a NUL, from errno 0: each
        /// string as u32 length and bytes in a buffer of `*size` bytes (release with
        /// [`free`]); `*error` is errno afterwards.
        pub resp_parse:
            unsafe extern "C" fn(*const c_char, usize, *mut usize, *mut i32) -> *mut c_char,
    }

    unsafe extern "C" {
        fn swr_oracle_mqtt_length_upstream(data: *const c_char, size: u32) -> i64;
        fn swr_oracle_resp_parse_upstream(
            data: *const c_char,
            length: usize,
            size: *mut usize,
            error: *mut i32,
        ) -> *mut c_char;
        fn swr_oracle_mqtt_length_candidate(data: *const c_char, size: u32) -> i64;
        fn swr_oracle_resp_parse_candidate(
            data: *const c_char,
            length: usize,
            size: *mut usize,
            error: *mut i32,
        ) -> *mut c_char;
        /// Releases a `resp_parse` buffer.
        #[link_name = "swr_oracle_free"]
        pub fn free(pointer: *mut c_char);
    }

    pub const UPSTREAM: Engine = Engine {
        mqtt_length: swr_oracle_mqtt_length_upstream,
        resp_parse: swr_oracle_resp_parse_upstream,
    };
    pub const CANDIDATE: Engine = Engine {
        mqtt_length: swr_oracle_mqtt_length_candidate,
        resp_parse: swr_oracle_resp_parse_candidate,
    };
}

pub mod clients {
    use std::ffi::c_char;

    /// `Address::assign(url)` over `length` bytes, from one engine's sources:
    /// `upstream` is the pinned file, `candidate` the file with the clients overlay
    /// patch, which calls the Rust export (the caller's binary must link it).
    /// 1 assigned, 0 rejected, -1 `std::stoi` threw; the host goes to `host`
    /// (`length` bytes) and its length to `*host_length`.
    pub type Url = unsafe extern "C" fn(
        *const c_char,
        usize,
        *mut i32,
        *mut i32,
        *mut c_char,
        *mut usize,
    ) -> i32;

    unsafe extern "C" {
        fn swr_oracle_clients_url_upstream(
            data: *const c_char,
            length: usize,
            kind: *mut i32,
            port: *mut i32,
            host: *mut c_char,
            host_length: *mut usize,
        ) -> i32;
        fn swr_oracle_clients_url_candidate(
            data: *const c_char,
            length: usize,
            kind: *mut i32,
            port: *mut i32,
            host: *mut c_char,
            host_length: *mut usize,
        ) -> i32;
        /// `php_swoole_http_parse_set_cookies`'s split of `length` bytes: key
        /// length, value offset and value length.
        #[link_name = "swr_oracle_clients_cookie"]
        pub fn cookie(
            at: *const c_char,
            length: usize,
            key: *mut usize,
            offset: *mut usize,
            value: *mut usize,
        );
        /// `domain_decode` in place over a NUL-terminated name.
        #[link_name = "swr_oracle_clients_domain_decode"]
        pub fn domain_decode(name: *mut c_char);
    }

    pub const UPSTREAM_URL: Url = swr_oracle_clients_url_upstream;
    pub const CANDIDATE_URL: Url = swr_oracle_clients_url_candidate;
}
