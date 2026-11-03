//! Multipart upload accounting and result policy; native retains parser/stdio.
/// Classify the mature parser's return after its native execution has ended.
/// -1=log parser error,0=log short result/reject,1=log short result/excepted,
/// 2=clear input/success. Unsigned cast matches the pinned `ssize_t/size_t` check.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_http_multipart_result(
    processed: i64,
    length: usize,
    excepted: u32,
) -> i32 {
    if processed < 0 {
        -1
    } else if usize::try_from(processed) != Ok(length) {
        i32::from(excepted != 0)
    } else {
        2
    }
}
/// Classify the complete native header name before scratch allocation can move it.
/// # Safety
/// The input span is readable for this call. No reference survives the return.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_http_multipart_disposition(data: *const u8, length: usize) -> i32 {
    // SAFETY: the caller lends `length` initialized bytes at `data` (null only when empty).
    match unsafe { crate::bytes(data, length) } {
        Ok(name) => i32::from(crate::case_equal(name, b"content-disposition")),
        Err(status) => status,
    }
}
/// Consume the existing destructive cookie iterator through the first filename.
/// The legacy callback stops even for a filename with no '=' or an empty value.
/// Native file creation runs only after this function returns.
/// # Safety
/// Scratch is writable through capacity<=`isize::MAX` and initialized through a
/// reachable NUL terminator. No reference or owner is retained across native work.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_http_multipart_filename(data: *mut u8, capacity: usize) -> i32 {
    let mut cursor = 0;
    let mut cookie = crate::headers::HttpCookie::default();
    loop {
        // SAFETY: `data` holds `capacity` writable bytes with a reachable NUL (see the contract).
        let status = unsafe {
            crate::headers::swr_v1_http_cookie_next(
                data,
                capacity,
                &raw mut cursor,
                &raw mut cookie,
            )
        };
        if status != 1 {
            return status;
        }
        // SAFETY: the parser returned a key span inside the `capacity` initialized bytes.
        let key = unsafe {
            core::slice::from_raw_parts(data.wrapping_add(cookie.key_offset), cookie.key_length)
        };
        if crate::case_equal(key, b"filename") {
            return 1;
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
            let mut out = crate::headers::HttpCookie::default();
            let result = loop {
                let status = unsafe {
                    crate::headers::swr_v1_http_cookie_next(
                        expected.as_mut_ptr(),
                        expected.len(),
                        &raw mut cursor,
                        &raw mut out,
                    )
                };
                if status != 1 {
                    break status;
                }
                if expected[out.key_offset..out.key_offset + out.key_length]
                    .eq_ignore_ascii_case(b"filename")
                {
                    break 1;
                }
            };
            assert_eq!(
                unsafe { swr_v1_http_multipart_filename(actual.as_mut_ptr(), actual.len()) },
                result
            );
            assert_eq!(actual, expected);
        }
        assert_eq!(
            unsafe { swr_v1_http_multipart_disposition(b"Content-Disposition".as_ptr(), 19) },
            1
        );
        assert_eq!(
            unsafe { swr_v1_http_multipart_disposition(b"content-disposition ".as_ptr(), 20) },
            0
        );
        assert_eq!(
            unsafe { swr_v1_http_multipart_filename(core::ptr::null_mut(), 0) },
            INVALID
        );
        assert_eq!(
            unsafe { swr_v1_http_multipart_disposition(core::ptr::null(), 1) },
            INVALID
        );
        let mut missing_nul = *b"filename";
        assert_eq!(
            unsafe { swr_v1_http_multipart_filename(missing_nul.as_mut_ptr(), missing_nul.len()) },
            INVALID
        );
    }
    #[test]
    fn parser_results() {
        assert_eq!(swr_v1_http_multipart_result(-1, usize::MAX, 1), -1);
        assert_eq!(swr_v1_http_multipart_result(1, 2, 0), 0);
        assert_eq!(swr_v1_http_multipart_result(1, 2, 1), 1);
        assert_eq!(swr_v1_http_multipart_result(2, 2, 0), 2);
    }
}
