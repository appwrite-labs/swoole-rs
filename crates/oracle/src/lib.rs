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
    }
}

pub mod crc32 {
    unsafe extern "C" {
        /// Upstream `swoole_crc32`.
        #[link_name = "swr_oracle_crc32"]
        pub fn crc32(data: *const u8, size: usize) -> u32;
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
        /// Upstream `websocket::encode` into a fresh `String`: its result, with the
        /// String's length in `written` and its bytes in `out` when `capacity` holds them.
        #[link_name = "swr_oracle_ws_encode"]
        pub fn encode(
            data: *const u8,
            length: usize,
            opcode: u8,
            flags: u8,
            out: *mut u8,
            capacity: usize,
            written: *mut usize,
        ) -> bool;
        /// Upstream `websocket::pack_close_frame`, as [`encode`].
        #[link_name = "swr_oracle_ws_pack_close"]
        pub fn pack_close(
            code: i32,
            reason: *const u8,
            length: usize,
            flags: u8,
            out: *mut u8,
            capacity: usize,
            written: *mut usize,
        ) -> bool;
        /// `WebSocketFrame` pack's flag normalization in `swoole_websocket_server.cc`:
        /// the flags it encodes with in the low byte, whether it compresses in bit 8.
        #[link_name = "swr_oracle_ws_pack_policy"]
        pub safe fn pack_policy(opcode: u8, flags: u8, length: usize) -> u16;
        /// The server's condition for keeping a close, ping or pong frame from
        /// `onMessage` (settings bits close=1, ping=2, pong=4): 0 deliver, 1 drop,
        /// 2 answer a ping with a pong.
        #[link_name = "swr_oracle_ws_message_policy"]
        pub safe fn message_policy(opcode: u8, settings: u8) -> u8;
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
        /// u32 value length, value, into `output` (capacity `8 * length + 16`);
        /// the callback stops it after the first when `first_only`. A non-null
        /// `scratch` receives the `length + 1` bytes it tokenized in place.
        #[link_name = "swr_oracle_http_cookies"]
        pub fn cookies(
            data: *const u8,
            length: usize,
            output: *mut u8,
            first_only: bool,
            scratch: *mut u8,
        ) -> usize;
    }
}

/// The pinned `get_method` and `parse_multipart_boundary` and swoole.h's two
/// case-insensitive helpers (see `shim/http_locale.cc`): each folds case through libc's
/// `strncasecmp`, so in the process locale.
pub mod http_locale {
    use std::ffi::{c_char, c_int, c_uint};

    unsafe extern "C" {
        /// Upstream `http_server::get_method`.
        #[link_name = "swr_oracle_http_get_method"]
        pub fn get_method(method: *const c_char, length: usize) -> c_int;
        /// Upstream `http_server::parse_multipart_boundary`: the boundary's offset in
        /// `at` and its length, written only when it returns true.
        #[link_name = "swr_oracle_http_boundary"]
        pub fn boundary(
            at: *const c_char,
            length: usize,
            offset: usize,
            found_offset: *mut usize,
            found_length: *mut c_int,
        ) -> bool;
        /// Upstream `swoole_strcaseeq`.
        #[link_name = "swr_oracle_http_strcaseeq"]
        pub fn strcaseeq(
            a: *const c_char,
            a_length: usize,
            b: *const c_char,
            b_length: usize,
        ) -> c_uint;
        /// Upstream `swoole_str_istarts_with`.
        #[link_name = "swr_oracle_http_istarts_with"]
        pub fn istarts_with(
            text: *const c_char,
            length: usize,
            prefix: *const c_char,
            prefix_length: usize,
        ) -> c_uint;
    }
}

/// The pinned `Server::add_rewrite_rule` and `apply_rewrite_rules` over a handler that
/// publishes into strings (see `shim/http_rewrite.cc`). Its regexes are the C++
/// standard library's `std::regex`.
pub mod http_rewrite {
    use std::ffi::{c_char, c_int, c_uint, c_void};

    pub use super::http_static_select::Line;

    #[repr(C)]
    pub struct Rule {
        pub pattern: *const c_char,
        pub pattern_length: usize,
        pub replacement: *const c_char,
        pub replacement_length: usize,
    }

    unsafe extern "C" {
        /// Adds each rule to a new server and applies them to a handler for `url`: the
        /// handler's URL and then its original URL go to `text`, and its
        /// `set_request_url` calls are counted in `publications`. The result of
        /// `apply_rewrite_rules`, or -1 for an exception it let through.
        #[link_name = "swr_oracle_rewrite"]
        pub fn rewrite(
            rules: *const Rule,
            count: usize,
            url: *const c_char,
            length: usize,
            publications: *mut c_uint,
            text: Line,
            out: *mut c_void,
        ) -> c_int;
        /// `apply_rewrite_rules` with a null handler on a server without rules.
        #[link_name = "swr_oracle_rewrite_without_rules"]
        pub safe fn without_rules() -> c_int;
    }
}

pub mod http_proxy {
    unsafe extern "C" {
        /// Upstream `HttpProxy::create`: returns its port and copies its host,
        /// username and password into `output` (capacity the three input lengths),
        /// their lengths into `lengths`.
        #[link_name = "swr_oracle_http_proxy_create"]
        pub fn create(
            host: *const u8,
            host_length: usize,
            port: i32,
            user: *const u8,
            user_length: usize,
            password: *const u8,
            password_length: usize,
            output: *mut u8,
            lengths: *mut [usize; 3],
        ) -> i32;
        /// Upstream `HttpProxy::get_auth_str` over these credentials into
        /// `output` (capacity 512); `error` is errno after it, EINTR before.
        #[link_name = "swr_oracle_http_proxy_auth"]
        pub fn auth(
            user: *const u8,
            user_length: usize,
            password: *const u8,
            password_length: usize,
            output: *mut u8,
            error: *mut i32,
        ) -> usize;
        /// Upstream `HttpProxy::pack` into `capacity` bytes of `output`; `error`
        /// is errno after it, EINTR before.
        #[link_name = "swr_oracle_http_proxy_pack"]
        pub fn pack(
            user: *const u8,
            user_length: usize,
            password: *const u8,
            password_length: usize,
            target: *const u8,
            target_length: usize,
            target_port: i32,
            host: *const u8,
            host_length: usize,
            output: *mut u8,
            capacity: usize,
            error: *mut i32,
        ) -> usize;
        /// Upstream `HttpProxy::handshake`; `error` is the last error it set
        /// (`SW_ERROR_HTTP_PROXY_HANDSHAKE_FAILED`, 7103) or 0. It sets errno to
        /// EINTR first, so the caller can see whether the handshake changed it.
        #[link_name = "swr_oracle_http_proxy_handshake"]
        pub fn handshake(data: *const u8, length: usize, error: *mut i32) -> bool;
    }
}

pub mod hooks_async {
    use core::ffi::c_void;

    unsafe extern "C" {
        /// Runs scenario 0..426 through upstream's `coroutine::async`, `async(fn)`,
        /// `System::sleep`, `gethostbyname_impl_with_async` and `System::getaddrinfo`
        /// over a recording facade (`shim/hooks_async.cc`), handing each effect in
        /// order to `line(context, text, length)`.
        #[link_name = "swr_oracle_hooks_async"]
        pub fn trace(
            scenario: i32,
            line: unsafe extern "C" fn(*mut c_void, *const u8, usize),
            context: *mut c_void,
        );
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
        pub max_length: u32,
        pub too_large: u32,
        pub chunk_state: u32,
        pub chunk_length: usize,
        pub chunk_offset: usize,
        pub chunk_scan_offset: usize,
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
    /// 2, `chunked` 4, `header_parsed` 16, `excepted` 32. Without
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
        fn swr_oracle_resp_trace_upstream(
            stream: *const Stream,
            out: *mut u8,
            capacity: usize,
        ) -> usize;
    }

    pub const UPSTREAM: Run = swr_oracle_resp_stream_upstream;
    pub const CANDIDATE: Run = swr_oracle_resp_stream_candidate;
    /// [`UPSTREAM`] with errno in the log: each call starts with it at `EDOM`, and the
    /// log gains `E<errno>` after each package and each call's result, `L<length>O<offset>`
    /// of the buffer after each call, and the request state (with its offset last)
    /// after a failed call too.
    pub const UPSTREAM_TRACE: Run = swr_oracle_resp_trace_upstream;
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

    /// `StaticHandler::parse_range(range, if_range ? "" : nullptr)` compiled from the
    /// pinned `static_handler.cc`, on a file of `filesize` bytes with this MIME type
    /// and boundary, `is_modified_range` answering `modified`. Up to `capacity`
    /// tasks go to `tasks` ([`TASK_SIZE`] bytes each).
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
    }

    pub const UPSTREAM: ParseRange = swr_oracle_http_range_upstream;
}

pub mod http_static_select {
    use std::ffi::{c_char, c_int, c_void};

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct Text {
        pub data: *const c_char,
        pub length: usize,
    }

    impl Text {
        #[must_use]
        pub fn of(bytes: &[u8]) -> Self {
            Self {
                data: bytes.as_ptr().cast(),
                length: bytes.len(),
            }
        }
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct Task {
        pub offset: i64,
        pub length: usize,
        pub part_header: [u8; 256],
    }

    /// The facade's answers: the handler's facts, the request's and what each
    /// send does to it.
    #[repr(C)]
    pub struct Scenario {
        pub serve: c_int,
        pub modified: c_int,
        pub index_ok: c_int,
        pub index: c_int,
        pub autoindex: c_int,
        pub directory: c_int,
        pub status: c_int,
        pub date: Text,
        pub last_modified: Text,
        pub mime: Text,
        pub content_type: Text,
        pub body: Text,
        pub filename: Text,
        pub end_part: Text,
        pub modified_header: Text,
        pub filesize: usize,
        pub content_length: usize,
        pub tasks: *const Task,
        pub task_count: usize,
        pub throw_on_send: c_int,
        pub send_result: c_int,
        pub toggle_keep_on_send: u32,
        pub toggle_method_on_send: u32,
        pub head: c_int,
        pub keep_alive: c_int,
    }

    pub type Line = unsafe extern "C" fn(out: *mut c_void, line: *const c_char, length: usize);

    unsafe extern "C" {
        /// The pinned `Server::select_static_handler` over the scenario's facade:
        /// each traced call, send and the request's final state go to `line`
        /// (see `shim/http_static_select.cc`). The result, or 2 if it threw.
        pub fn swr_oracle_static_select(
            scenario: *const Scenario,
            line: Line,
            out: *mut c_void,
        ) -> c_int;
    }
}

/// The pinned `StaticHandler` over a scripted filesystem, MIME table and random
/// strings (see `shim/http_static_context.cc`), and the libc calendar calls it and
/// a candidate make, recorded in order.
pub mod http_static_context {
    use std::ffi::{c_char, c_int, c_long, c_void};

    pub use super::http_static_select::Line;

    #[repr(C)]
    pub struct Rule {
        pub pattern: *const c_char,
        pub replacement: *const c_char,
        pub is_regex: c_int,
    }

    /// The server: `http_autoindex`, a `/` location, `http_index_files` (when
    /// `has_index`), the timezone and the rewrite rules. Strings are NUL-terminated.
    #[repr(C)]
    pub struct Server {
        pub autoindex: c_int,
        pub location: c_int,
        pub has_index: c_int,
        pub timezone: c_long,
        pub index: *const *const c_char,
        pub index_count: usize,
        pub rules: *const Rule,
        pub rule_count: usize,
    }

    /// What the scripted filesystem answers: every stat gives `file_size` and
    /// `file_time`, a symlink at `link`, a directory at `directory`, else a file;
    /// `canonical` (or the path itself when empty) resolves every path; every
    /// directory lists `names`. `mime` is every MIME type. Strings are NUL-terminated.
    #[repr(C)]
    pub struct Fs {
        pub file_size: usize,
        pub file_time: i64,
        pub directory: *const c_char,
        pub link: *const c_char,
        pub canonical: *const c_char,
        pub mime: *const c_char,
        pub names: *const *const c_char,
        pub name_count: usize,
    }

    unsafe extern "C" {
        pub fn swr_oracle_static_script(fs: *const Fs);
        /// Whether the shim's compiler evaluates `sw_snprintf`'s arguments left to
        /// right (clang) rather than right to left (GCC); C++ leaves it unspecified.
        pub fn swr_oracle_static_arguments_left_to_right() -> c_int;
        /// `count` outputs of `std::mt19937(seed)`.
        pub fn swr_oracle_mt19937(seed: u32, out: *mut u32, count: usize);
        pub fn swr_oracle_static_trace(line: Line, out: *mut c_void);
        pub fn swr_oracle_calendar_events(line: Line, out: *mut c_void);
        pub fn swr_oracle_calendar_parse(
            text: *const c_char,
            format: *const c_char,
            value: *mut c_void,
        ) -> bool;
        pub fn swr_oracle_calendar_time(value: *mut c_void) -> i64;
        pub fn swr_oracle_calendar_gmt(time: i64) -> *mut c_void;
        pub fn swr_oracle_static_new(
            server: *const Server,
            url: *const c_char,
            length: usize,
        ) -> *mut c_void;
        pub fn swr_oracle_static_free(handle: *mut c_void);
        pub fn swr_oracle_static_try_serve(handle: *mut c_void) -> c_int;
        pub fn swr_oracle_static_try_serve_index_file(handle: *mut c_void) -> c_int;
        pub fn swr_oracle_static_get_dir_files(handle: *mut c_void) -> c_int;
        pub fn swr_oracle_static_parse_range(
            handle: *mut c_void,
            range: *const c_char,
            if_range: *const c_char,
        );
        pub fn swr_oracle_static_is_modified(
            handle: *mut c_void,
            date: *const c_char,
            length: usize,
        ) -> c_int;
        pub fn swr_oracle_static_is_modified_range(
            handle: *mut c_void,
            date: *const c_char,
            length: usize,
        ) -> c_int;
        pub fn swr_oracle_static_filename(handle: *mut c_void, line: Line, out: *mut c_void);
        pub fn swr_oracle_static_buffer_new(capacity: usize) -> *mut c_void;
        pub fn swr_oracle_static_buffer_free(buffer: *mut c_void);
        /// `make_index_page` into the buffer; the bytes it appended go to `line`.
        pub fn swr_oracle_static_index_page(
            handle: *mut c_void,
            buffer: *mut c_void,
            line: Line,
            out: *mut c_void,
        ) -> usize;
        /// The handler's state as lines; nonzero when its MIME and content-type
        /// references are upstream's.
        pub fn swr_oracle_static_snapshot(
            handle: *mut c_void,
            line: Line,
            out: *mut c_void,
        ) -> c_int;
    }
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
    use std::ffi::{c_char, c_int, c_uint, c_void};

    /// What the pinned `Address::assign(type, host, port, resolve)` leaves in a
    /// record filled with 0x73, type -1, length 987, errno 17 and last error 11.
    /// `addr` holds the first `address_size()` bytes of the sockaddr union.
    #[repr(C)]
    pub struct Assign {
        pub addr: [u8; 128],
        pub len: u32,
        pub kind: c_int,
        pub result: c_int,
        pub error: c_int,
        pub last_error: c_int,
        pub dns_calls: c_int,
        pub loopback: c_int,
        pub dns_host_length: usize,
        pub dns_host: [u8; 256],
    }

    unsafe extern "C" {
        /// The pinned `Address::assign(url)` over `length` bytes, into a recorder that
        /// returns `accept`: its result, or -1 for `std::out_of_range` and -2 for
        /// `std::invalid_argument`. The type (87 if unset), port, host (`length`
        /// bytes) and error-log count it leaves go to the out parameters.
        #[link_name = "swr_oracle_clients_url"]
        pub fn url(
            data: *const c_char,
            length: usize,
            accept: bool,
            kind: *mut c_int,
            port: *mut c_int,
            host: *mut c_char,
            host_length: *mut usize,
            logs: *mut c_int,
        ) -> c_int;
        /// The request formatter's pinned `sw_snprintf` call for `kind` (0 length, 1
        /// proxy URI, 2 multipart type, 3 field, 4 file, 5 closing boundary) over four
        /// texts and `number`, into `capacity` bytes at `output` (null for 0), from
        /// errno 17; errno afterwards goes to `error`.
        #[link_name = "swr_oracle_clients_http_format"]
        pub fn http_format(
            kind: c_uint,
            texts: *const *const c_char,
            lengths: *const usize,
            number: usize,
            output: *mut c_char,
            capacity: usize,
            error: *mut c_int,
        ) -> usize;
        /// The pinned `Address::verify_port`.
        #[link_name = "swr_oracle_clients_verify_port"]
        pub fn verify_port(port: c_int, for_connect: bool) -> bool;
        /// The pinned `Address::verify_ip` over `length` bytes, run with errno 17 and
        /// its 46-byte scratch filled with 0x53; errno and the scratch after go out.
        #[link_name = "swr_oracle_clients_verify_ip"]
        pub fn verify_ip(
            family: c_int,
            host: *const c_char,
            length: usize,
            error: *mut c_int,
            scratch: *mut c_char,
        ) -> bool;
        /// The size of the pinned `Address`'s sockaddr union.
        #[link_name = "swr_oracle_clients_address_size"]
        pub fn address_size() -> usize;
        /// The pinned `Address::assign(type, host, port, resolve)` over `length`
        /// bytes, with a `gethostbyname` double that fails for `resolve-fail`.
        #[link_name = "swr_oracle_clients_assign"]
        pub fn assign(
            kind: c_int,
            host: *const c_char,
            length: usize,
            port: c_int,
            resolve: bool,
            out: *mut Assign,
        );
        /// The pinned `Address::is_loopback_addr` for `kind` over 16 address bytes.
        #[link_name = "swr_oracle_clients_loopback"]
        pub fn loopback(kind: c_int, bytes: *const u8) -> bool;
        /// Which of the 256 bytes the C++ locale's ctype classes as space, with the
        /// extra spaces of [`hosts`] when `extra_space`.
        #[link_name = "swr_oracle_clients_spaces"]
        pub fn spaces(extra_space: bool, spaces: *mut u8);
        /// The pinned `get_ip_by_hosts` over the hosts file at `path`, under a C++
        /// locale that also classes NUL, 0xff and `@` as space when `extra_space`;
        /// the address goes to `address` and its length is returned.
        #[link_name = "swr_oracle_clients_hosts"]
        pub fn hosts(
            path: *const c_char,
            name: *const c_char,
            length: usize,
            extra_space: bool,
            address: *mut c_char,
            capacity: usize,
        ) -> usize;
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
        /// `domain_encode` over `n` bytes of the NUL-terminated `src` into `dest`.
        #[link_name = "swr_oracle_clients_domain_encode"]
        pub fn domain_encode(src: *const c_char, n: c_int, dest: *mut c_char) -> c_int;
        /// `dns_lookup_impl_with_socket` (request ids from 1 per thread) over a UDP
        /// socket double that answers the query it was sent with one record of
        /// `record_type` (1 A, 28 AAAA, else a CNAME): `receive_mode` 0 fails the
        /// receive, 1 receives 3 bytes, 2 the whole answer; `bad_id` flips the answer's
        /// id. Each address goes to `effect(context, 0, ..)`, each socket call and
        /// last error set (`timeout`, `send`, `receive`, `error:N`) as kind 1 and the
        /// query sent as kind 2.
        /// `parse_ip_address` of the 4- (`AF_INET`) or 16-byte address into `text`;
        /// its length.
        #[link_name = "swr_oracle_clients_ip_text"]
        pub fn ip_text(
            address: *mut c_void,
            family: c_int,
            text: *mut c_char,
            capacity: usize,
        ) -> usize;
        /// The socket double's answer to the `length`-byte `query` it was sent, into
        /// `buffer`: -1 when `receive_mode` is 0, its first 3 bytes when 1, all of it
        /// when 2.
        #[link_name = "swr_oracle_clients_dns_answer"]
        pub fn dns_answer(
            record_type: c_int,
            receive_mode: c_int,
            bad_id: bool,
            query: *const c_char,
            length: usize,
            buffer: *mut c_char,
            capacity: usize,
        ) -> isize;
        #[link_name = "swr_oracle_clients_dns_lookup"]
        pub fn dns_lookup(
            domain: *const c_char,
            family: c_int,
            timeout: f64,
            record_type: c_int,
            receive_mode: c_int,
            send_success: bool,
            canceled: bool,
            bad_id: bool,
            context: *mut c_void,
            effect: unsafe extern "C" fn(*mut c_void, c_int, *const c_char, usize),
        );
    }
}

/// Upstream's bundled nghttp2 HPACK inflater, the reference for crates/http2's `hpack`.
pub mod nghttp2 {
    use core::ffi::{c_int, c_void};

    /// `nghttp2_nv`.
    #[repr(C)]
    pub struct Nv {
        pub name: *const u8,
        pub value: *const u8,
        pub name_length: usize,
        pub value_length: usize,
        pub flags: u8,
    }

    /// `NGHTTP2_HD_INFLATE_FINAL`.
    pub const INFLATE_FINAL: c_int = 1;
    /// `NGHTTP2_HD_INFLATE_EMIT`.
    pub const INFLATE_EMIT: c_int = 2;

    unsafe extern "C" {
        pub fn nghttp2_hd_inflate_new(inflater: *mut *mut c_void) -> c_int;
        pub fn nghttp2_hd_inflate_del(inflater: *mut c_void);
        pub fn nghttp2_hd_inflate_hd(
            inflater: *mut c_void,
            nv: *mut Nv,
            flags: *mut c_int,
            input: *const u8,
            length: usize,
            in_final: c_int,
        ) -> isize;
        pub fn nghttp2_hd_inflate_end_headers(inflater: *mut c_void) -> c_int;
    }
}

/// Upstream core-tests' recorded requests (`core-tests/fuzz/cases`).
pub mod cases {
    /// A multipart POST of three form fields.
    pub const REQ1: &[u8] = include_bytes!("../upstream/core-tests/fuzz/cases/req1.bin");
    /// A multipart POST of the same three fields and `examples/test.jpg`.
    pub const REQ2: &[u8] = include_bytes!("../upstream/core-tests/fuzz/cases/req2.bin");
}

/// Upstream's hashes (misc.cc), `Logger::get_pretty_name` and the logger's level,
/// label, rotation, line and microsecond formatting (log.cc), built with
/// `-fsigned-char`; the hashes promote `char`, so they come `-funsigned-char` too.
pub mod utilities {
    use std::ffi::{c_char, c_int};
    unsafe extern "C" {
        /// `swoole_hash_php` (0), `swoole_hash_jenkins` (1) or `swoole_hash_austin` (2).
        #[link_name = "swr_oracle_utils_signed_char_hash"]
        pub fn hash(algorithm: u32, key: *const u8, length: usize) -> u64;
        /// [`hash`] with `char` unsigned.
        #[link_name = "swr_oracle_utils_unsigned_char_hash"]
        pub fn hash_unsigned(algorithm: u32, key: *const u8, length: usize) -> u64;
        /// `Logger::set_level`'s clamp.
        #[link_name = "swr_oracle_utils_signed_char_log_level"]
        pub fn log_level(level: c_int) -> c_int;
        /// `Logger::put`'s level label.
        #[link_name = "swr_oracle_utils_signed_char_log_label"]
        pub fn log_label(level: c_int) -> *const c_char;
        /// `Logger::gen_real_file`'s date format.
        #[link_name = "swr_oracle_utils_signed_char_log_rotation"]
        pub fn log_rotation(rotation: c_int) -> *const c_char;
        /// `Logger::get_pretty_name` copied into `output` (capacity `length`); returns its length.
        #[link_name = "swr_oracle_utils_signed_char_pretty"]
        pub fn pretty(name: *const u8, length: usize, strip: bool, output: *mut u8) -> usize;
        /// `Logger::put`'s line into `capacity` bytes of `output` (null only at zero);
        /// `error` is errno after it, EINTR before.
        #[link_name = "swr_oracle_utils_signed_char_log_line"]
        pub fn log_line(
            output: *mut u8,
            capacity: usize,
            date: *const c_char,
            date_precision: c_int,
            symbol: c_char,
            pid: c_int,
            worker: c_int,
            level: c_int,
            content: *const c_char,
            content_precision: c_int,
            error: *mut c_int,
        ) -> usize;
        /// `Logger::put`'s `<.microseconds>` suffix, as [`log_line`].
        #[link_name = "swr_oracle_utils_signed_char_log_micros"]
        pub fn log_micros(
            output: *mut u8,
            capacity: usize,
            micros: i64,
            error: *mut c_int,
        ) -> usize;
    }
}
