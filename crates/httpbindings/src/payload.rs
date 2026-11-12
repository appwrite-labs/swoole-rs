//! Request payload options and content-encoding policy; PHP effects remain native.
use crate::request::case_equal;
use core::ffi::CStr;

/// Native compiled capabilities: bit0 compression options, bit1 zlib options.
/// Returns 0 for an unknown key, 1..7 for the original native option assignments.
#[must_use]
pub fn request_option(key: &[u8], capabilities: u32) -> i32 {
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
/// Capabilities: bit0 zlib, bit1 Brotli, bit2 Zstd. Return native method 0..4.
#[must_use]
pub fn compression(encoding: &[u8], capabilities: u32) -> i32 {
    // (found bit, capability, native method, token), in order of preference.
    const METHODS: [(u32, u32, i32, &[u8]); 4] = [
        (1, 2, 3, b"br"),
        (2, 1, 1, b"gzip"),
        (4, 1, 2, b"deflate"),
        (8, 4, 4, b"zstd"),
    ];
    // One pass: a token can only start at its first byte, which no other token shares.
    let mut found = 0;
    for (at, byte) in encoding.iter().enumerate() {
        let (bit, token): (u32, &[u8]) = match byte {
            b'b' => (1, b"br"),
            b'g' => (2, b"gzip"),
            b'd' => (4, b"deflate"),
            b'z' => (8, b"zstd"),
            _ => continue,
        };
        if encoding
            .get(at..)
            .is_some_and(|rest| rest.starts_with(token))
        {
            found |= bit;
        }
    }
    METHODS
        .iter()
        .find(|&&(bit, capability, _, _)| found & bit != 0 && capabilities & capability != 0)
        .map_or(0, |&(_, _, method, _)| method)
}

/// The encoding label, or `None` for unknown/uncompiled methods.
#[must_use]
pub fn encoding(method: i32, capabilities: u32) -> Option<&'static CStr> {
    let (required, text) = match method {
        1 => (1, c"gzip"),
        2 => (1, c"deflate"),
        3 => (2, c"br"),
        4 => (4, c"zstd"),
        _ => return None,
    };
    (capabilities & required != 0).then_some(text)
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

/// Leading CR/LF accepted only at the original first multipart-body fragment:
/// how many bytes of `body` to skip.
#[must_use]
pub fn body_prefix(body: &[u8], beginning: bool) -> usize {
    if beginning {
        body.iter()
            .take_while(|&&byte| matches!(byte, b'\r' | b'\n'))
            .count()
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::compression;

    /// The definition: the first of br, gzip, deflate and zstd that the build has
    /// and that occurs anywhere in the header.
    fn by_substring(encoding: &[u8], capabilities: u32) -> i32 {
        let methods: [(u32, i32, &[u8]); 4] = [
            (2, 3, b"br"),
            (1, 1, b"gzip"),
            (1, 2, b"deflate"),
            (4, 4, b"zstd"),
        ];
        methods
            .iter()
            .find(|&&(capability, _, token)| {
                capabilities & capability != 0
                    && encoding.windows(token.len()).any(|window| window == token)
            })
            .map_or(0, |&(_, method, _)| method)
    }

    #[test]
    fn compression_prefers_as_the_substring_search() {
        let headers: [&[u8]; 14] = [
            b"",
            b"identity",
            b"gzip, deflate, br, zstd",
            b"zstd, deflate",
            b"deflate;q=0",
            b"GZIP",
            b"xbrx",
            b"b",
            b"gzi",
            b"ggzip",
            b"deflatdeflate",
            b"zstzstd",
            b"bbr",
            b"gzipdeflatebrzstd",
        ];
        for header in headers {
            for capabilities in 0..8 {
                assert_eq!(
                    compression(header, capabilities),
                    by_substring(header, capabilities),
                    "{:?} with {capabilities}",
                    String::from_utf8_lossy(header)
                );
            }
        }
    }
}
