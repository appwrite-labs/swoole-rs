//! Header and URL decisions for the existing native PHP request callbacks.
use swoole_rs_locale::{case_prefix, is_space, strncasecmp_equal};

pub(crate) fn case_equal(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len() && case_prefix(left, right)
}

/// What the parser does with a request header beyond storing it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RequestHeader {
    Append,
    Cookie,
    Upgrade,
    Form,
    Multipart,
    OtherContentType,
    Compression,
}

/// The header's action and known key index. Method is the pinned llhttp method
/// number. Classify the name once for both the action and the array insertion.
#[must_use]
#[inline(always)]
pub fn request_header(
    name: &[u8],
    value: &[u8],
    method: u8,
    cookie: bool,
    compression: bool,
) -> (RequestHeader, Option<usize>) {
    let key = request_key(name);
    let action = match key {
        // DELETE, POST, PUT, PATCH and QUERY carry form bodies.
        Some(3) if matches!(method, 0 | 3 | 4 | 28 | 46) => {
            if case_prefix(value, b"application/x-www-form-urlencoded") {
                RequestHeader::Form
            } else if case_prefix(value, b"multipart/form-data") {
                RequestHeader::Multipart
            } else {
                RequestHeader::OtherContentType
            }
        }
        Some(7) if compression => RequestHeader::Compression,
        None if cookie && case_equal(name, b"cookie") => RequestHeader::Cookie,
        None if case_equal(name, b"upgrade") => RequestHeader::Upgrade,
        _ => RequestHeader::Append,
    };
    (action, key)
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
#[inline(always)]
pub fn request_key(name: &[u8]) -> Option<usize> {
    // Keep each expected name constant so the locale comparison can fold its
    // length and word masks instead of dispatching through a dynamic slice.
    match name.len() {
        4 => case_equal(name, b"host").then_some(0),
        6 => case_equal(name, b"accept").then_some(2),
        10 => {
            if case_equal(name, b"user-agent") {
                Some(1)
            } else {
                case_equal(name, b"connection").then_some(6)
            }
        }
        12 => case_equal(name, b"content-type").then_some(3),
        13 => case_equal(name, b"authorization").then_some(5),
        14 => case_equal(name, b"content-length").then_some(4),
        15 => case_equal(name, b"accept-encoding").then_some(7),
        _ => None,
    }
}

/// `swoole_http_token_list_contains_value`'s fast path: `SW_STRCASEEQ` with
/// `sizeof(value) - 1` on a pointer, so a span of exactly that many bytes compares
/// that prefix of `value`. `None` asks for [`token_list_contains`].
#[must_use]
pub fn token_list_fast(input: &[u8], value: &core::ffi::CStr) -> Option<bool> {
    const FAST: usize = size_of::<*const u8>() - 1;
    if input.is_empty() {
        Some(false)
    } else if input.len() == FAST && strncasecmp_equal(input, value.to_bytes_with_nul(), FAST) {
        Some(true)
    } else {
        None
    }
}

/// `swoole_http_token_list_contains_value`'s scan: the comma-separated tokens
/// before the first NUL, each trimmed of `isspace`, compared case-insensitively.
#[must_use]
pub fn token_list_contains(input: &[u8], value: &[u8]) -> bool {
    let input = input.split(|&c| c == 0).next().unwrap_or_default();
    input
        .split(|&c| c == b',')
        .filter(|token| !token.is_empty())
        .any(|mut token| {
            while let [first, rest @ ..] = token
                && is_space(*first)
            {
                token = rest;
            }
            while let [rest @ .., last] = token
                && is_space(*last)
            {
                token = rest;
            }
            case_equal(token, value)
        })
}

/// The request target's path, and its query after a `?` (empty without one).
#[must_use]
pub fn request_url(input: &[u8]) -> (&[u8], &[u8]) {
    let mut parts = input.splitn(2, |&c| c == b'?');
    (
        parts.next().unwrap_or_default(),
        parts.next().unwrap_or_default(),
    )
}

/// `http_trim_double_quote`: leading quotes go while they are fewer than what
/// remains after them (the original shrinking loop bound, not `trim_matches`),
/// then trailing ones.
#[must_use]
pub fn trim_quotes(input: &[u8]) -> &[u8] {
    let offset = input
        .iter()
        .take_while(|&&byte| byte == b'"')
        .count()
        .min(input.len().div_ceil(2));
    let mut rest = input.get(offset..).unwrap_or_default();
    while let [kept @ .., b'"'] = rest {
        rest = kept;
    }
    rest
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
    #[cfg_attr(miri, ignore = "libc's strncasecmp is a foreign call")]
    fn request_key_dispatch_matches_the_ordered_locale_comparisons() {
        let reference = |name: &[u8]| REQUEST_KEYS.iter().position(|key| case_equal(name, key));
        for (index, key) in REQUEST_KEYS.iter().enumerate() {
            assert_eq!(request_key(key), Some(index));
            assert_eq!(request_key(&key.to_ascii_uppercase()), Some(index));
            for length in 0..=key.len() {
                let prefix = &key[..length];
                assert_eq!(request_key(prefix), reference(prefix));
            }
            for at in 0..key.len() {
                for byte in 0..=u8::MAX {
                    let mut name = key.to_vec();
                    name[at] = byte;
                    assert_eq!(request_key(&name), reference(&name), "{name:?}");
                }
            }
        }
        assert_eq!(
            request_header(b"Cookie", b"a=b", 1, true, false).0,
            RequestHeader::Cookie
        );
        assert_eq!(
            request_header(b"Cookie", b"a=b", 1, false, false).0,
            RequestHeader::Append
        );
        assert_eq!(
            request_header(b"Upgrade", b"websocket", 1, false, false).0,
            RequestHeader::Upgrade
        );
        assert_eq!(
            request_header(b"Accept-Encoding", b"gzip", 1, false, true),
            (RequestHeader::Compression, Some(7))
        );
        assert_eq!(
            request_header(b"Accept-Encoding", b"gzip", 1, false, false),
            (RequestHeader::Append, Some(7))
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "libc's strncasecmp and isspace are foreign calls")]
    fn token_lists_keep_the_pinned_fast_path_and_scan() {
        assert_eq!(token_list_fast(b"", c"websocket"), Some(false));
        let fast = size_of::<*const u8>() - 1;
        let prefix = &b"WebSocket"[..fast];
        assert_eq!(token_list_fast(prefix, c"websocket"), Some(true));
        assert_eq!(token_list_fast(b"websocket", c"websocket"), None);
        assert!(token_list_contains(b"h2c, \tWebSocket \t", b"websocket"));
        assert!(token_list_contains(b",,websocket,", b"websocket"));
        assert!(!token_list_contains(b"web socket", b"websocket"));
        assert!(!token_list_contains(b"h2c\0, websocket", b"websocket"));
        assert!(token_list_contains(b"h2c,  ,x", b""));
    }

    #[test]
    fn urls_split_at_the_first_question_mark() {
        assert_eq!(request_url(b"/a?b?c"), (&b"/a"[..], &b"b?c"[..]));
        assert_eq!(request_url(b"/a?"), (&b"/a"[..], &b""[..]));
        assert_eq!(request_url(b"/a"), (&b"/a"[..], &b""[..]));
    }

    #[test]
    fn quote_trimming_keeps_the_shrinking_bound() {
        assert_eq!(trim_quotes(b"\"\"\"a"), b"\"a");
        assert_eq!(trim_quotes(b"\"file\""), b"file");
        assert_eq!(trim_quotes(b"\"\""), b"");
        assert_eq!(trim_quotes(b"\""), b"");
        assert_eq!(trim_quotes(b"plain"), b"plain");
    }
}
