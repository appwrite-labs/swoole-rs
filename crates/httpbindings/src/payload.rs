//! Request payload options and content-encoding policy; PHP effects remain native.
use crate::Text;
use crate::request::{bytes, case_equal};

/// Native compiled capabilities: bit0 compression options, bit1 zlib options.
/// Returns0 for an unknown key,1..7 for the original native option assignments.
/// # Safety
/// Key is initialized and readable for its declared length for this call.
#[must_use]
pub unsafe fn swr_v1_httpbindings_request_option(key: Text, capabilities: u32) -> i32 {
    const OPTIONS: [(&[u8], u32); 7] = [
        (b"parse_cookie", 0),
        (b"parse_body", 0),
        (b"parse_files", 0),
        (b"enable_compression", 1),
        (b"compression_level", 1),
        (b"websocket_compression", 2),
        (b"upload_tmp_dir", 0),
    ];
    // SAFETY: `key` spans `length` initialized bytes (null only when empty) for this call.
    let Some(key) = (unsafe { bytes(key) }) else {
        return -1;
    };
    OPTIONS
        .iter()
        .position(|&(name, required)| capabilities & required == required && case_equal(key, name))
        .and_then(|index| i32::try_from(index + 1).ok())
        .unwrap_or(0)
}

/// Exact case-sensitive native substring preference, including matches inside
/// tokens and q=0. Caller truncates the input length to native `uint32_t` first.
/// Capabilities: bit0 zlib, bit1 Brotli, bit2 Zstd. Return native method0..4.
/// # Safety
/// Encoding is an initialized borrowed span, readable only for this call.
#[must_use]
pub unsafe fn swr_v1_httpbindings_compression(encoding: Text, capabilities: u32) -> i32 {
    const METHODS: [(u32, i32, &[u8]); 4] = [
        (2, 3, b"br"),
        (1, 1, b"gzip"),
        (1, 2, b"deflate"),
        (4, 4, b"zstd"),
    ];
    // SAFETY: `encoding` spans `length` initialized bytes (null only when empty) for this call.
    let Some(encoding) = (unsafe { bytes(encoding) }) else {
        return -1;
    };
    METHODS
        .iter()
        .find(|&&(capability, _, token)| {
            capabilities & capability != 0
                && encoding.windows(token.len()).any(|window| window == token)
        })
        .map_or(0, |&(_, method, _)| method)
}

/// The immutable encoding label or an absent span for unknown/uncompiled methods.
#[must_use]
pub fn swr_v1_httpbindings_encoding(method: i32, capabilities: u32) -> Text {
    let (required, text): (u32, &[u8]) = match method {
        1 => (1, b"gzip\0"),
        2 => (1, b"deflate\0"),
        3 => (2, b"br\0"),
        4 => (4, b"zstd\0"),
        _ => return Text::default(),
    };
    if capabilities & required == 0 {
        return Text::default();
    }
    Text {
        data: text.as_ptr(),
        length: text.len() - 1,
    }
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

/// Leading CR/LF accepted only at the original first multipart-body fragment.
/// # Safety
/// Body is an initialized readable span.
#[expect(
    clippy::must_use_candidate,
    reason = "callers may ignore the status: a rejected span leaves the prefix as it was"
)]
pub unsafe fn swr_v1_httpbindings_body_prefix(
    body: Text,
    beginning: u32,
    output: Option<&mut usize>,
) -> i32 {
    // SAFETY: `body` spans `length` initialized bytes (null only when empty) for this call.
    let body = unsafe { bytes(body) };
    let (Some(body), Some(output)) = (body, output) else {
        return -1;
    };
    *output = if beginning == 0 {
        0
    } else {
        body.iter()
            .take_while(|&&byte| matches!(byte, b'\r' | b'\n'))
            .count()
    };
    0
}
