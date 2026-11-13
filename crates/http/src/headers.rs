//! Header metadata and cookie tokenization with native work between pure steps.
use super::{HttpSpan, INVALID, tail};
fn space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t'..=b'\r')
}
fn starts(input: &[u8], expected: &[u8]) -> bool {
    super::case_prefix(input, expected)
}
fn number(input: &[u8]) -> (u64, u32) {
    let mut at = 0;
    while input.get(at).is_some_and(|&v| space(v)) {
        at += 1;
    }
    let negative = input.get(at) == Some(&b'-');
    if negative || input.get(at) == Some(&b'+') {
        at += 1;
    }
    let digits = at;
    let mut value = Some(0u64);
    while let Some(byte @ b'0'..=b'9') = input.get(at).copied() {
        value = value.and_then(|n| n.checked_mul(10)?.checked_add(u64::from(byte - b'0')));
        at += 1;
    }
    if at == digits {
        return (0, 2);
    }
    match value {
        Some(value) => (
            if negative {
                value.wrapping_neg()
            } else {
                value
            },
            0,
        ),
        None => (u64::MAX, 1),
    }
}
#[repr(C)]
#[derive(Default)]
pub struct HttpHeaderInfo {
    pub cursor: usize,
    pub content_length: u64,
    /// known1,keepalive2,chunked4,transfer-encoding8,parsed16,codings ending in
    /// chunked32,a coding after chunked64. Bits only become set.
    pub flags: u32,
    /// Bit1=range error seen,bit2=last numeric error had no digits. Native
    /// reproduces libc errno policy after return, before native allocation.
    pub number_errors: u32,
}
/// `parse_header_info` over the header block ending at `end`, from `state.cursor`:
/// negative when malformed, 0 once done, 1 with a multipart boundary in `multipart`.
pub fn header_info(
    input: &[u8],
    end: usize,
    state: &mut HttpHeaderInfo,
    multipart: &mut HttpSpan,
) -> i32 {
    let length = input.len();
    *multipart = HttpSpan::default();
    state.number_errors = 0;
    let Some(head) = input.get(..end) else {
        return INVALID;
    };
    if state.cursor > length || (state.cursor < 2 && state.cursor < end) {
        return INVALID;
    }
    let mut at = state.cursor;
    while at < end {
        // The next line start: just past a `\n` at `at - 1` or later (`at >= 2` here).
        let newline = input
            .get(at - 1..end - 1)
            .and_then(|line| memchr::memchr(b'\n', line));
        let Some(newline) = newline else {
            at = end;
            break;
        };
        at += newline;
        if input.get(at - 2) == Some(&b'\r') {
            let line = tail(head, at);
            let kind = if starts(line, b"Content-Length:") {
                Some((15, 1))
            } else if starts(line, b"Connection:") {
                Some((11, 2))
            } else if starts(line, b"Transfer-Encoding:") {
                Some((18, 3))
            } else if starts(line, b"Content-Type:") {
                Some((13, 4))
            } else {
                None
            };
            if let Some((skip, kind)) = kind {
                at += skip;
                while input.get(at) == Some(&b' ') {
                    at += 1;
                }
                state.cursor = at + 1;
                match kind {
                    1 => {
                        let (value, overflow) = number(tail(input, at));
                        state.content_length = value;
                        state.flags |= 1;
                        if overflow == 1 {
                            state.number_errors = (state.number_errors | 1) & !2;
                        } else if overflow == 2 {
                            state.number_errors |= 2;
                        }
                    }
                    2 => {
                        if starts(tail(head, at), b"keep-alive") {
                            state.flags |= 2;
                        }
                    }
                    3 => {
                        // The list of codings, up to the line's CR.
                        let value = input.get(at..end + 2).unwrap_or_default();
                        let length = memchr::memchr(b'\r', value).unwrap_or(value.len());
                        state.flags |= 8;
                        let codings = value.get(..length).unwrap_or_default();
                        for mut coding in codings.split(|&b| b == b',') {
                            while let [b' ' | b'\t', rest @ ..] = coding {
                                coding = rest;
                            }
                            while let [rest @ .., b' '] = coding {
                                coding = rest;
                            }
                            if state.flags & 32 != 0 {
                                state.flags |= 64;
                            }
                            if coding.eq_ignore_ascii_case(b"chunked") {
                                state.flags |= 32;
                            }
                        }
                        at += length;
                    }
                    4 => {
                        if starts(tail(head, at), b"multipart/form-data") {
                            let Some(count) =
                                tail(input, at).iter().position(|&b| matches!(b, b'\r' | 0))
                            else {
                                return INVALID;
                            };
                            if input.get(at + count) == Some(&0) {
                                return INVALID;
                            }
                            let begin = at + 19;
                            *multipart = HttpSpan {
                                offset: begin,
                                length: at + count - begin,
                            };
                            return 1;
                        }
                    }
                    _ => unreachable!(),
                }
            }
        }
        at += 1;
    }
    state.cursor = at;
    // Transfer-Encoding frames the body only when chunked is its final coding.
    if state.flags & 8 != 0 {
        if state.flags & 96 != 32 {
            state.flags |= 16;
            return INVALID;
        }
        state.flags |= 4;
    }
    state.flags |= 16;
    0
}
#[repr(C)]
#[derive(Default)]
pub struct HttpCookie {
    pub key_offset: usize,
    pub key_length: usize,
    pub value_offset: usize,
    pub value_length: usize,
    pub has_value: u32,
}
/// The byte at `at`, which must lie before the buffer's end.
fn get(data: &[u8], at: usize) -> Result<u8, i32> {
    data.get(at).copied().ok_or(INVALID)
}
/// Tokenize the next cookie of `data` from `cursor`, as legacy `strtok_r`: the
/// delimiter after it and its '=' become NULs. Reads stop at the NUL that ends the
/// cookies; None there.
///
/// # Errors
/// `INVALID` when the buffer ends before that NUL.
pub fn cookie_next(data: &mut [u8], cursor: &mut usize) -> Result<Option<HttpCookie>, i32> {
    loop {
        while get(data, *cursor)? == b';' {
            *cursor += 1;
        }
        if get(data, *cursor)? == 0 {
            return Ok(None);
        }
        let mut key = *cursor;
        let mut end = key;
        while !matches!(get(data, end)?, b';' | 0) {
            end += 1;
        }
        *cursor = end;
        if let Some(delimiter @ b';') = data.get_mut(end) {
            *delimiter = 0;
            *cursor += 1;
        }
        let equal = data
            .get(key..end)
            .and_then(|token| token.iter().position(|&byte| byte == b'='))
            .map(|at| key + at);
        while key < end && space(get(data, key)?) {
            key += 1;
        }
        if key == end || equal == Some(key) {
            continue;
        }
        let (key_length, value_offset, value_length) = if let Some(equal) = equal {
            if let Some(byte) = data.get_mut(equal) {
                *byte = 0;
            }
            (equal - key, equal + 1, end - equal - 1)
        } else {
            (end - key, 0, 0)
        };
        return Ok(Some(HttpCookie {
            key_offset: key,
            key_length,
            value_offset,
            value_length,
            has_value: u32::from(equal.is_some()),
        }));
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decimal_matches_unsigned_legacy_policy_and_errno_signal() {
        for (input, expected) in [
            (&b" -1"[..], (u64::MAX, 0)),
            (&b"+42\r"[..], (42, 0)),
            (&b"x"[..], (0, 2)),
            (&b"18446744073709551616"[..], (u64::MAX, 1)),
            (&b"-18446744073709551615"[..], (1, 0)),
        ] {
            assert_eq!(number(input), expected);
        }
    }
    #[test]
    fn metadata_stops_at_native_allocation_and_finishes_flags_after_resume() {
        let input=b"GET / HTTP/1.1\r\nContent-Length: 0\r\nContent-Type: multipart/form-data; boundary=a\r\nTransfer-Encoding: chunked\r\n\r\n";
        let mut state = HttpHeaderInfo {
            cursor: 16,
            ..Default::default()
        };
        let mut span = HttpSpan::default();
        assert_eq!(
            header_info(input, input.len() - 4, &mut state, &mut span),
            1
        );
        assert_eq!((state.flags, state.content_length), (1, 0));
        assert_eq!(
            &input[span.offset..span.offset + span.length],
            b"; boundary=a"
        );
        assert_eq!(
            header_info(input, input.len() - 4, &mut state, &mut span),
            0
        );
        assert_eq!(state.flags, 61);
    }
    #[test]
    fn only_lines_after_crlf_before_end_count() {
        let scan = |input: &[u8], cursor: usize, end: usize| {
            let mut state = HttpHeaderInfo {
                cursor,
                ..Default::default()
            };
            let mut span = HttpSpan::default();
            assert_eq!(header_info(input, end, &mut state, &mut span), 0);
            (state.cursor, state.flags, state.content_length)
        };
        let input = b"GET / HTTP/1.1\nContent-Length: 5\r\nX: a\rContent-Length: 7\r\nConnection: keep-alive\r\n\r\n";
        assert_eq!(
            scan(input, 2, input.len()),
            (input.len(), 18, 0),
            "LF or CR alone starts no line"
        );
        let input = b"GET / HTTP/1.1\r\nHost: a\r\nContent-Length: 12\r\n\r\n";
        assert_eq!(scan(input, 2, input.len()), (input.len(), 17, 12));
        assert_eq!(
            scan(input, 25, input.len()),
            (input.len(), 17, 12),
            "cursor at the line start"
        );
        assert_eq!(
            scan(input, 26, input.len()),
            (input.len(), 16, 0),
            "cursor past it"
        );
        assert_eq!(scan(input, 2, 25), (25, 16, 0), "the line starts at end");
        assert_eq!(scan(input, 2, 26), (26, 16, 0), "the name runs past end");
        assert_eq!(scan(input, 2, 41), (42, 17, 12), "the name ends before end");
    }
    #[test]
    fn cookie_tokens_keep_leading_trim_trailing_spaces_and_value_bytes() {
        let mut data = b";; a =x=y; =bad;novalue;\tlast=\0".to_vec();
        let mut cursor = 0;
        let mut result = Vec::new();
        while let Some(out) = cookie_next(&mut data, &mut cursor).unwrap() {
            result.push((
                data[out.key_offset..out.key_offset + out.key_length].to_vec(),
                if out.has_value != 0 {
                    data[out.value_offset..out.value_offset + out.value_length].to_vec()
                } else {
                    Vec::new()
                },
            ));
        }
        assert_eq!(
            result,
            vec![
                (b"a ".to_vec(), b"x=y".to_vec()),
                (b"novalue".to_vec(), vec![]),
                (b"last".to_vec(), vec![])
            ]
        );
    }
}
