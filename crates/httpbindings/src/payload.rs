//! Request payload options and content-encoding policy; PHP effects remain native.
use crate::request::case_equal;
use core::ffi::CStr;

/// Native compiled capabilities: bit0 compression options, bit1 zlib options.
/// Returns 0 for an unknown key, 1..7 for the original native option assignments.
#[must_use]
pub fn swr_v1_httpbindings_request_option(key: &[u8], capabilities: u32) -> i32 {
    const OPTIONS: [(&[u8], u32); 7] = [
        (b"parse_cookie", 0),
        (b"parse_body", 0),
        (b"parse_files", 0),
        (b"enable_compression", 1),
        (b"compression_level", 1),
        (b"websocket_compression", 2),
        (b"upload_tmp_dir", 0),
    ];
    OPTIONS
        .iter()
        .position(|&(name, required)| capabilities & required == required && case_equal(key, name))
        .and_then(|index| i32::try_from(index + 1).ok())
        .unwrap_or(0)
}

/// Exact case-sensitive native substring preference, including matches inside
/// tokens and q=0. Caller truncates the input length to native `uint32_t` first.
/// Capabilities: bit0 zlib, bit1 Brotli, bit2 Zstd. Return native method0..4.
#[must_use]
pub fn swr_v1_httpbindings_compression(encoding: &[u8], capabilities: u32) -> i32 {
    const METHODS: [(u32, i32, &[u8]); 4] = [
        (2, 3, b"br"),
        (1, 1, b"gzip"),
        (1, 2, b"deflate"),
        (4, 4, b"zstd"),
    ];
    METHODS
        .iter()
        .find(|&&(capability, _, token)| {
            capabilities & capability != 0
                && encoding.windows(token.len()).any(|window| window == token)
        })
        .map_or(0, |&(_, method, _)| method)
}

/// The encoding label, empty for unknown/uncompiled methods.
#[must_use]
pub fn swr_v1_httpbindings_encoding(method: i32, capabilities: u32) -> &'static CStr {
    let (required, text) = match method {
        1 => (1, c"gzip"),
        2 => (1, c"deflate"),
        3 => (2, c"br"),
        4 => (4, c"zstd"),
        _ => return c"",
    };
    if capabilities & required == 0 {
        return c"";
    }
    text
}

/// Which native form parser effect follows completion. Flags are chunked buffer,
/// parse-body, form-urlencoded, receive-chunked and body pointer, respectively.
#[must_use]
pub fn swr_v1_httpbindings_form_source(flags: u32) -> i32 {
    if flags & 6 != 6 {
        0
    } else if flags & 1 != 0 {
        1
    } else if flags & 24 == 16 {
        2
    } else {
        0
    }
}

/// The leading CR/LF skipped, only at the original first multipart-body fragment.
#[must_use]
pub fn swr_v1_httpbindings_body_prefix(body: &[u8], beginning: bool) -> usize {
    if !beginning {
        return 0;
    }
    body.iter()
        .take_while(|&&byte| matches!(byte, b'\r' | b'\n'))
        .count()
}
