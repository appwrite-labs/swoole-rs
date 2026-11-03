//! Header metadata and cookie tokenization with native work between pure steps.
use super::{HttpSpan, INVALID};
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
    /// known1,keepalive2,chunked4,nobody8,parsed16. Bits only become set.
    pub flags: u32,
    /// Bit1=range error seen,bit2=last numeric error had no digits. Native
    /// reproduces libc errno policy after return, before native allocation.
    pub number_errors: u32,
}
pub(crate) fn header_info(
    input: &[u8],
    end: usize,
    state: &mut HttpHeaderInfo,
    multipart: &mut HttpSpan,
) -> i32 {
    let length = input.len();
    *multipart = HttpSpan::default();
    state.number_errors = 0;
    if state.cursor > length || end > length || (state.cursor < 2 && state.cursor < end) {
        return INVALID;
    }
    let mut at = state.cursor;
    while at < end {
        if input[at - 2..at] == *b"\r\n" {
            let kind = if starts(&input[at..end], b"Content-Length:") {
                Some((15, 1))
            } else if starts(&input[at..end], b"Connection:") {
                Some((11, 2))
            } else if starts(&input[at..end], b"Transfer-Encoding:") {
                Some((18, 3))
            } else if starts(&input[at..end], b"Content-Type:") {
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
                        let (value, overflow) = number(&input[at..]);
                        state.content_length = value;
                        state.flags |= 1;
                        if overflow == 1 {
                            state.number_errors = (state.number_errors | 1) & !2;
                        } else if overflow == 2 {
                            state.number_errors |= 2;
                        }
                    }
                    2 => {
                        if at <= end && starts(&input[at..end], b"keep-alive") {
                            state.flags |= 2;
                        }
                    }
                    3 => {
                        if at <= end && starts(&input[at..end], b"chunked") {
                            state.flags |= 4;
                        }
                    }
                    4 => {
                        if at <= end && starts(&input[at..end], b"multipart/form-data") {
                            let Some(count) =
                                input[at..].iter().position(|&b| matches!(b, b'\r' | 0))
                            else {
                                return INVALID;
                            };
                            if input[at + count] == 0 {
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
        state.cursor = at;
    }
    state.flags |= 16;
    if state.flags & 5 == 5 && state.content_length == 0 {
        state.flags |= 8;
    }
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
unsafe fn get(data: *const u8, at: usize, capacity: usize) -> Result<u8, i32> {
    if at >= capacity {
        return Err(INVALID);
    }
    // SAFETY: `at < capacity`; callers read only bytes up to the initialized NUL.
    Ok(unsafe { data.wrapping_add(at).read() })
}
unsafe fn cookie(
    data: *mut u8,
    capacity: usize,
    cursor: &mut usize,
) -> Result<Option<HttpCookie>, i32> {
    // Every `get` below reads an index that `get` itself bounds by `capacity`, scanning no
    // further than the initialized NUL reachable from the cursor; each write lands on an
    // index `get` has just read.
    loop {
        // SAFETY: as above.
        while unsafe { get(data, *cursor, capacity)? } == b';' {
            *cursor += 1;
        }
        // SAFETY: as above.
        if unsafe { get(data, *cursor, capacity)? } == 0 {
            return Ok(None);
        }
        let mut key = *cursor;
        let mut end = key;
        // SAFETY: as above.
        while !matches!(unsafe { get(data, end, capacity)? }, b';' | 0) {
            end += 1;
        }
        *cursor = end;
        // SAFETY: as above.
        if unsafe { get(data, end, capacity)? } == b';' {
            // SAFETY: as above; `end` was just read as a delimiter.
            unsafe { data.wrapping_add(end).write(0) };
            *cursor += 1;
        }
        let mut equal = None;
        for at in key..end {
            // SAFETY: as above.
            if unsafe { get(data, at, capacity)? } == b'=' {
                equal = Some(at);
                break;
            }
        }
        // SAFETY: as above.
        while key < end && space(unsafe { get(data, key, capacity)? }) {
            key += 1;
        }
        if key == end || equal == Some(key) {
            continue;
        }
        let (key_length, value_offset, value_length) = if let Some(equal) = equal {
            // SAFETY: as above; `equal` was read as '='.
            unsafe { data.wrapping_add(equal).write(0) };
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
/// Tokenize one cookie, then native invokes its callback after Rust returns.
/// Buffer capacity may contain uninitialized suffix bytes: reads stop at its
/// initialized NUL terminator. Delimiter/'=' mutations match legacy `strtok_r`.
/// # Safety
/// Buffer is writable through capacity<=`isize::MAX`, initialized through a NUL
/// reachable from cursor; cursor/output are valid/aligned/disjoint. Callback may
/// mutate the native buffer between calls, but never retains a Rust reference.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_http_cookie_next(
    data: *mut u8,
    capacity: usize,
    cursor: *mut usize,
    output: *mut HttpCookie,
) -> i32 {
    if output.is_null() || cursor.is_null() {
        return INVALID;
    }
    // SAFETY: `output` is non-null (checked above), aligned and writable.
    unsafe {
        output.write(HttpCookie::default());
    }
    if data.is_null() || capacity > isize::MAX as usize {
        return INVALID;
    }
    // SAFETY: `cursor` is non-null (checked above), aligned and disjoint from `data`.
    let cursor = unsafe { &mut *cursor };
    // SAFETY: `data` is non-null, writable through `capacity <= isize::MAX` and initialized
    // through a NUL reachable from the cursor.
    match unsafe { cookie(data, capacity, cursor) } {
        Ok(Some(value)) => {
            // SAFETY: `output` is non-null (checked above), aligned and writable.
            unsafe {
                output.write(value);
            }
            1
        }
        Ok(None) => 0,
        Err(status) => status,
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
        assert_eq!(state.flags, 29);
    }
    #[test]
    fn cookie_tokens_keep_leading_trim_trailing_spaces_and_value_bytes() {
        let mut data = b";; a =x=y; =bad;novalue;\tlast=\0".to_vec();
        let mut cursor = 0;
        let mut out = HttpCookie::default();
        let mut result = Vec::new();
        loop {
            let status = unsafe {
                swr_v1_http_cookie_next(
                    data.as_mut_ptr(),
                    data.len(),
                    &raw mut cursor,
                    &raw mut out,
                )
            };
            if status == 0 {
                break;
            }
            assert_eq!(status, 1);
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
