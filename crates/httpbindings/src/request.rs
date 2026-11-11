//! Header and URL decisions for the existing native PHP request callbacks.
use core::ffi::CStr;
pub(crate) use swoole_rs_locale::{case_equal, case_prefix};
use swoole_rs_locale::{isspace, strncasecmp_equal};

/// What the parser does with a request header beyond storing it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RequestHeader {
    Append,
    Cookie,
    Upgrade,
    Form,
    Multipart,
    Compression,
}

/// The header's action. Method is the pinned llhttp method number.
#[must_use]
pub fn request_header(
    name: &[u8],
    value: &[u8],
    method: u8,
    cookie: bool,
    compression: bool,
) -> RequestHeader {
    if cookie && case_equal(name, b"cookie") {
        return RequestHeader::Cookie;
    }
    if case_equal(name, b"upgrade") {
        return RequestHeader::Upgrade;
    }
    // DELETE, POST, PUT, PATCH and QUERY (llhttp 46, upstream cf66ff7e8) carry form bodies.
    if matches!(method, 0 | 3 | 4 | 28 | 46) && case_equal(name, b"content-type") {
        if case_prefix(value, b"application/x-www-form-urlencoded") {
            return RequestHeader::Form;
        }
        if case_prefix(value, b"multipart/form-data") {
            return RequestHeader::Multipart;
        }
    } else if compression && case_equal(name, b"accept-encoding") {
        return RequestHeader::Compression;
    }
    RequestHeader::Append
}

/// The known header keys, in the order `request_key` numbers them.
pub const REQUEST_KEYS: [&[u8]; 8] = [
    b"host",
    b"user-agent",
    b"accept",
    b"content-type",
    b"content-length",
    b"authorization",
    b"connection",
    b"accept-encoding",
];

/// The name's index in `REQUEST_KEYS`, or `None` for the unknown-name merge path.
#[must_use]
pub fn request_key(name: &[u8]) -> Option<usize> {
    REQUEST_KEYS.iter().position(|key| case_equal(name, key))
}

/// `swoole_http_token_list_contains_value`'s fast path: `Some(found)` without a
/// scan, `None` when [`token_list_contains`] must scan. It compares
/// `fast_extent` bytes, the pinned helper's `sizeof(pointer) - 1`, not `value`'s
/// length (with `strncasecmp`, so `value`'s NUL ends it); on 64-bit, `websock`
/// matches `websocket` through it.
#[must_use]
pub fn token_list_fast(input: &[u8], value: &CStr, fast_extent: usize) -> Option<bool> {
    if input.is_empty() {
        return Some(false);
    }
    let length = input.len();
    (length == fast_extent && strncasecmp_equal(input, value.to_bytes_with_nul(), length))
        .then_some(true)
}

/// The scan over the pinned helper's terminated copy: `strtok_r` up to the first
/// NUL at commas, each token trimmed of `isspace` and compared with `strncasecmp`.
/// Its writes into the copy land only behind the scan, so it reads `input` as is.
#[must_use]
pub fn token_list_contains(input: &[u8], value: &CStr) -> bool {
    let copy = input.split(|&c| c == 0).next().unwrap_or_default();
    copy.split(|&c| c == b',')
        .filter(|token| !token.is_empty())
        .any(|token| {
            let first = token.iter().position(|&c| !isspace(c));
            let last = token.iter().rposition(|&c| !isspace(c));
            let token = match (first, last) {
                (Some(first), Some(last)) => token.get(first..=last).unwrap_or_default(),
                _ => &[],
            };
            case_equal(token, value.to_bytes())
        })
}

#[derive(Default, Debug, PartialEq, Eq)]
pub struct UrlParts {
    pub path_length: usize,
    pub query_offset: usize,
    pub query_length: usize,
}

/// The path, and the query after the first `?`.
#[must_use]
pub fn swr_v1_httpbindings_request_url(input: &[u8]) -> UrlParts {
    let path_length = input.iter().position(|&c| c == b'?').unwrap_or(input.len());
    let query_offset = if path_length < input.len() {
        path_length + 1
    } else {
        input.len()
    };
    UrlParts {
        path_length,
        query_offset,
        query_length: input.len() - query_offset,
    }
}

/// `http_trim_double_quote`'s result. The original shrinking left-loop bound is
/// intentional; this is not `trim_matches`.
#[must_use]
pub fn swr_v1_httpbindings_trim_quotes(input: &[u8]) -> &[u8] {
    // Leading quotes go while they are fewer than what remains after them.
    let offset = input
        .iter()
        .take_while(|&&byte| byte == b'"')
        .count()
        .min(input.len().div_ceil(2));
    let rest = input.get(offset..).unwrap_or_default();
    let trailing = rest.iter().rev().take_while(|&&byte| byte == b'"').count();
    rest.get(..rest.len() - trailing).unwrap_or_default()
}

/// Canonical header case (`content-type` to `Content-Type`), keeping the
/// original state machine's leading and consecutive hyphen behaviour.
pub fn canonical_case(name: &mut [u8]) {
    let mut initial = true;
    for byte in name {
        if initial {
            *byte = byte.to_ascii_uppercase();
            initial = false;
        } else if *byte == b'-' {
            initial = true;
        } else {
            *byte = byte.to_ascii_lowercase();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_lists_keep_the_pinned_helpers_quirks() {
        let fast = size_of::<*const u8>() - 1;
        let contains = |input: &[u8]| {
            token_list_fast(input, c"websocket", fast)
                .unwrap_or_else(|| token_list_contains(input, c"websocket"))
        };
        assert!(!contains(b""));
        assert!(contains(b"WebSocket"));
        assert!(contains(b"h2c, \t websocket \x0b,x"));
        assert!(!contains(b"h2c,,web socket"));
        assert!(
            !contains(b"h2c\0, websocket"),
            "the copy ends at the first NUL"
        );
        if fast == 7 {
            assert!(contains(b"websock"), "the pointer-sized fast comparison");
        }
    }

    #[test]
    fn quote_trimming_keeps_the_shrinking_bound() {
        assert_eq!(swr_v1_httpbindings_trim_quotes(b"\"\"\"a"), b"\"a");
        assert_eq!(swr_v1_httpbindings_trim_quotes(b"\"a\"\""), b"a");
        assert_eq!(swr_v1_httpbindings_trim_quotes(b"\"\""), b"");
        assert_eq!(swr_v1_httpbindings_trim_quotes(b""), b"");
    }
}
