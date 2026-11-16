//! Request payload options and content-encoding policy; PHP effects remain native.
use crate::request::case_equal;
use core::ffi::CStr;

/// A negotiated HTTP content coding. Zero is a valid no-coding value because
/// the PHP HTTP context initializes its storage with zeroes.
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Compression {
    #[default]
    None = 0,
    Gzip = 1,
    Deflate = 2,
    Brotli = 3,
    Zstd = 4,
}

/// A `Request::create` option.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestOption {
    ParseCookie,
    ParseBody,
    ParseFiles,
    EnableCompression,
    CompressionLevel,
    WebsocketCompression,
    UploadTmpDir,
}

/// The option `key` names (case-insensitively), `None` for an unknown key or one the
/// build lacks. Native compiled capabilities: bit0 compression options, bit1 zlib
/// options.
#[must_use]
pub fn request_option(key: &[u8], capabilities: u32) -> Option<RequestOption> {
    const OPTIONS: [(&[u8], u32, RequestOption); 7] = [
        (b"parse_cookie", 0, RequestOption::ParseCookie),
        (b"parse_body", 0, RequestOption::ParseBody),
        (b"parse_files", 0, RequestOption::ParseFiles),
        (b"enable_compression", 1, RequestOption::EnableCompression),
        (b"compression_level", 1, RequestOption::CompressionLevel),
        (
            b"websocket_compression",
            2,
            RequestOption::WebsocketCompression,
        ),
        (b"upload_tmp_dir", 0, RequestOption::UploadTmpDir),
    ];
    OPTIONS
        .iter()
        .find(|&&(name, required, _)| capabilities & required == required && case_equal(key, name))
        .map(|&(_, _, option)| option)
}

/// Exact case-sensitive native substring preference, including matches inside
/// tokens and q=0. Caller truncates the input length to native `uint32_t` first.
/// Capabilities: bit0 zlib, bit1 Brotli, bit2 Zstd.
#[must_use]
pub fn compression(encoding: &[u8], capabilities: u32) -> Compression {
    // (found bit, capability, native method, token), in order of preference.
    const METHODS: [(u32, u32, Compression, &[u8]); 4] = [
        (1, 2, Compression::Brotli, b"br"),
        (2, 1, Compression::Gzip, b"gzip"),
        (4, 1, Compression::Deflate, b"deflate"),
        (8, 4, Compression::Zstd, b"zstd"),
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
        .map_or(Compression::None, |&(_, _, method, _)| method)
}

/// The encoding label, or `None` for no coding or an uncompiled method.
#[must_use]
pub fn encoding(method: Compression, capabilities: u32) -> Option<&'static CStr> {
    let (required, text) = match method {
        Compression::Gzip => (1, c"gzip"),
        Compression::Deflate => (1, c"deflate"),
        Compression::Brotli => (2, c"br"),
        Compression::Zstd => (4, c"zstd"),
        Compression::None => return None,
    };
    (capabilities & required != 0).then_some(text)
}

/// The body a completed urlencoded form is parsed from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormSource {
    /// The chunked body the parser buffered.
    Chunked,
    /// The request's own body span.
    Body,
}

/// Which body, if any, a completed request parses as a form. Flags are chunked buffer,
/// parse-body, form-urlencoded, receive-chunked and body pointer, respectively.
#[must_use]
pub const fn form_source(flags: u32) -> Option<FormSource> {
    if flags & 6 != 6 {
        None
    } else if flags & 1 != 0 {
        Some(FormSource::Chunked)
    } else if flags & 24 == 16 {
        Some(FormSource::Body)
    } else {
        None
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
                    compression(header, capabilities) as i32,
                    by_substring(header, capabilities),
                    "{:?} with {capabilities}",
                    String::from_utf8_lossy(header)
                );
            }
        }
    }
}
