//! Response header policy; crates/phphttp builds the header and runs the codecs.
use crate::{Compression, request::case_equal};

/// `swHttpHeaderFlag`: the known response headers' flags.
pub const SERVER: i32 = 2;
pub const CONNECTION: i32 = 4;
pub const CONTENT_LENGTH: i32 = 8;
pub const DATE: i32 = 16;
pub const CONTENT_TYPE: i32 = 32;
pub const TRANSFER_ENCODING: i32 = 64;
pub const CONTENT_ENCODING: i32 = 256;

/// The known-header flag of a response header name; 0 for a custom one.
#[must_use]
pub fn response_header(name: &[u8]) -> i32 {
    const HEADERS: [(&[u8], i32); 7] = [
        (b"Server", SERVER),
        (b"Connection", CONNECTION),
        (b"Date", DATE),
        (b"Content-Length", CONTENT_LENGTH),
        (b"Content-Type", CONTENT_TYPE),
        (b"Transfer-Encoding", TRANSFER_ENCODING),
        (b"Content-Encoding", CONTENT_ENCODING),
    ];
    HEADERS
        .iter()
        .find(|&&(key, _)| case_equal(name, key))
        .map_or(0, |&(_, flag)| flag)
}

/// Trim the six C-locale whitespace bytes from the end of a response header value.
#[must_use]
pub fn trim_header_end(value: &[u8]) -> &[u8] {
    let end = value
        .iter()
        .rposition(|&b| !matches!(b, b' ' | b'\t'..=b'\r'));
    end.and_then(|i| value.get(..=i)).unwrap_or_default()
}

/// Swoole reports the first invalid header byte in input order.
#[must_use]
pub fn header_warning(value: &[u8]) -> Option<&'static str> {
    value.iter().find_map(|byte| match byte {
        b'\r' | b'\n' => {
            Some("Header may not contain more than a single header, new line detected")
        }
        0 => Some("Header may not contain NUL bytes"),
        _ => None,
    })
}

/// Native codec selection before any allocation/backend call. Capabilities are
/// zlib/Brotli/Zstd bits. `None` skips compression before consulting the method.
///
/// # Errors
/// The requested method is unavailable, retaining the caller's warning path.
pub fn codec(
    method: Compression,
    capabilities: u32,
    accepted: bool,
    length: usize,
    minimum: u32,
) -> Result<Option<Compression>, Compression> {
    if !accepted || length < minimum as usize {
        return Ok(None);
    }
    let required = match method {
        Compression::Gzip | Compression::Deflate => 1,
        Compression::Brotli => 2,
        Compression::Zstd => 4,
        Compression::None => return Err(method),
    };
    if capabilities & required != 0 {
        Ok(Some(method))
    } else {
        Err(method)
    }
}

/// Preserve codec-specific native level policy. Zstd normalizes a captured local
/// level; Brotli/zlib native callers publish the result to their original field.
/// Brotli retains min-first and Zstd max-first order even when the bounds are inverted.
#[must_use]
pub fn codec_level(method: Compression, level: i32, minimum: i32, maximum: i32) -> i32 {
    match method {
        Compression::Gzip | Compression::Deflate => match level {
            i32::MIN..=-1 => -1,
            0 => 1,
            value => value.min(9),
        },
        Compression::Brotli => {
            if level < minimum {
                minimum
            } else if level > maximum {
                maximum
            } else {
                level
            }
        }
        Compression::Zstd => {
            if level > maximum {
                maximum
            } else if level < minimum {
                minimum
            } else {
                level
            }
        }
        Compression::None => level,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn response_header_trims_only_trailing_c_whitespace() {
        let spaces: Vec<_> = (0..=255)
            .filter(|&b| trim_header_end(&[b]).is_empty())
            .collect();
        assert_eq!(spaces, b"\t\n\x0b\x0c\r ", "C locale");
        assert_eq!(
            trim_header_end(b" a b \t\r\n"),
            b" a b",
            "keep leading space"
        );
        assert!(trim_header_end(b"  ").is_empty(), "all whitespace");
    }

    #[test]
    fn response_header_reports_the_first_invalid_byte() {
        let newline = Some("Header may not contain more than a single header, new line detected");
        assert_eq!(header_warning(b"a\r\0"), newline, "CR first");
        assert_eq!(header_warning(b"a\n\0"), newline, "LF first");
        assert_eq!(
            header_warning(b"a\0\n"),
            Some("Header may not contain NUL bytes"),
            "NUL first"
        );
        assert_eq!(header_warning(b"a\xff"), None, "opaque bytes");
    }

    #[test]
    fn selection_skips_before_checking_availability() {
        for (method, required) in [
            (Compression::None, 0),
            (Compression::Gzip, 1),
            (Compression::Deflate, 1),
            (Compression::Brotli, 2),
            (Compression::Zstd, 4),
        ] {
            for capabilities in 0..8 {
                assert_eq!(codec(method, capabilities, false, 100, 10), Ok(None));
                assert_eq!(codec(method, capabilities, true, 9, 10), Ok(None));
                let expected = if capabilities & required == 0 {
                    Err(method)
                } else {
                    Ok(Some(method))
                };
                assert_eq!(codec(method, capabilities, true, 10, 10), expected);
                assert_eq!(
                    crate::encoding(method, capabilities).is_some(),
                    expected.is_ok()
                );
            }
        }
    }

    #[test]
    fn levels_keep_defaults_and_inverted_bound_order() {
        for method in [Compression::Gzip, Compression::Deflate] {
            for (input, expected) in [
                (i32::MIN, -1),
                (-2, -1),
                (-1, -1),
                (0, 1),
                (1, 1),
                (9, 9),
                (10, 9),
                (i32::MAX, 9),
            ] {
                assert_eq!(codec_level(method, input, 0, 0), expected);
            }
        }
        assert_eq!(codec_level(Compression::Brotli, 3, 11, 0), 11);
        assert_eq!(codec_level(Compression::Zstd, 3, 11, 0), 0);
        assert_eq!(codec_level(Compression::Zstd, -3, -7, 22), -3);
        assert_eq!(codec_level(Compression::None, -17, 0, 9), -17);
    }
}
