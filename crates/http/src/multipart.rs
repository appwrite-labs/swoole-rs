//! Multipart upload accounting and result policy; native retains parser/stdio.
/// Classify the mature parser's return after its native execution has ended.
/// -1=log parser error,0=log short result/reject,1=log short result/excepted,
/// 2=clear input/success. Unsigned cast matches the pinned `ssize_t/size_t` check.
#[must_use]
pub fn multipart_result(processed: i64, length: usize, excepted: u32) -> i32 {
    if processed < 0 {
        -1
    } else if usize::try_from(processed) != Ok(length) {
        i32::from(excepted != 0)
    } else {
        2
    }
}
/// Whether a part's header `name` is `Content-Disposition`.
#[must_use]
pub fn multipart_disposition(name: &[u8]) -> bool {
    crate::case_equal(name, b"content-disposition")
}
/// Consume the destructive cookie iterator over `data` through the first filename:
/// 1 when found, 0 when not, negative without a NUL. The legacy callback stops even
/// for a filename with no '=' or an empty value.
pub fn multipart_filename(data: &mut [u8]) -> i32 {
    let mut cursor = 0;
    loop {
        match crate::headers::cookie_next(data, &mut cursor) {
            Ok(Some(cookie)) => {
                let key = data
                    .get(cookie.key_offset..cookie.key_offset + cookie.key_length)
                    .unwrap_or_default();
                if crate::case_equal(key, b"filename") {
                    return 1;
                }
            }
            Ok(None) => return 0,
            Err(status) => return status,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::INVALID;
    #[test]
    fn filename_stops_with_identical_destructive_cookie_state() {
        for input in [
            &b"form-data; name=x; filename=one; filename=two\0"[..],
            b"FiLeNaMe;next=x\0",
            b"filename =x;other=y\0",
            b"=bad; filename=\0",
            b"filename\0;ignored=value",
            b"name=x\0",
            b"\0",
        ] {
            let mut expected = input.to_vec();
            let mut actual = input.to_vec();
            let mut cursor = 0;
            let result = loop {
                match crate::headers::cookie_next(&mut expected, &mut cursor) {
                    Ok(Some(out)) => {
                        if expected[out.key_offset..out.key_offset + out.key_length]
                            .eq_ignore_ascii_case(b"filename")
                        {
                            break 1;
                        }
                    }
                    Ok(None) => break 0,
                    Err(status) => break status,
                }
            };
            assert_eq!(multipart_filename(&mut actual), result);
            assert_eq!(actual, expected);
        }
        assert!(multipart_disposition(b"Content-Disposition"));
        assert!(!multipart_disposition(b"content-disposition "));
        assert_eq!(multipart_filename(&mut []), INVALID);
        let mut missing_nul = *b"filename";
        assert_eq!(multipart_filename(&mut missing_nul), INVALID);
    }
    #[test]
    fn parser_results() {
        assert_eq!(multipart_result(-1, usize::MAX, 1), -1);
        assert_eq!(multipart_result(1, 2, 0), 0);
        assert_eq!(multipart_result(1, 2, 1), 1);
        assert_eq!(multipart_result(2, 2, 0), 2);
    }
}
