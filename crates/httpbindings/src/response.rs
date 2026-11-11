//! Response header policy; crates/phphttp builds the header and runs the codecs.
use crate::request::case_equal;

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

/// Native codec selection before any allocation/backend call. Capabilities are
/// zlib/Brotli/Zstd bits; unsupported methods retain the native warning path.
#[must_use]
pub fn swr_v1_httpbindings_codec(
    method: i32,
    capabilities: u32,
    accepted: u32,
    length: usize,
    minimum: u32,
) -> i32 {
    if accepted == 0 || length < minimum as usize {
        return 0;
    }
    let required = match method {
        1 | 2 => 1,
        3 => 2,
        4 => 4,
        _ => return -1,
    };
    if capabilities & required != 0 {
        method
    } else {
        -1
    }
}

/// Preserve codec-specific native level policy. Zstd normalizes a captured local
/// level; Brotli/zlib native callers publish the result to their original field.
/// Brotli retains min-first and Zstd max-first order, with no clamp panic across C ABI.
#[must_use]
pub fn swr_v1_httpbindings_codec_level(method: i32, level: i32, minimum: i32, maximum: i32) -> i32 {
    match method {
        1 | 2 => match level {
            i32::MIN..=-1 => -1,
            0 => 1,
            value => value.min(9),
        },
        3 => {
            if level < minimum {
                minimum
            } else if level > maximum {
                maximum
            } else {
                level
            }
        }
        4 => {
            if level > maximum {
                maximum
            } else if level < minimum {
                minimum
            } else {
                level
            }
        }
        _ => level,
    }
}
