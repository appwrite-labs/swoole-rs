//! The C ABI's contracts: errors leave outputs alone, nulls and overflows are refused.
use super::*;
#[test]
fn positive_url_encoding_capacity_and_bytes_cover_loop_boundaries() {
    for length in [0, 1, 7, 8, 15, 16, 31, 32, 63, 64, 127, 128, 255, 256, 1024] {
        for escaped in [false, true] {
            let mut input = b"Az09-._~".repeat(length);
            if escaped {
                input.extend_from_slice(b"/+%\0\xff");
            }
            let mut expected = b"Az09-._~".repeat(length);
            if escaped {
                expected.extend_from_slice(b"%2F%2B%25%00%FF");
            }
            expected.push(0);
            let mut capacity = 0;
            assert_eq!(
                unsafe {
                    swr_v1_http_url_capacity(input.as_ptr(), input.len(), Some(&mut capacity))
                },
                0
            );
            assert_eq!(capacity, expected.len());
            let mut output = vec![0xa5; capacity + 3];
            let mut written = 999;
            assert_eq!(
                unsafe {
                    swr_v1_http_url_encode(
                        input.as_ptr(),
                        input.len(),
                        output.as_mut_ptr(),
                        capacity,
                        Some(&mut written),
                    )
                },
                0
            );
            assert_eq!(written + 1, capacity);
            assert_eq!(&output[..capacity], expected);
            assert_eq!(&output[capacity..], &[0xa5; 3]);
        }
    }
}
#[test]
fn url_roundtrips_all_bytes_and_handles_malformed_escapes() {
    let input: Vec<u8> = (0..=255).collect();
    let mut encoded = vec![0; swoole_rs_http::url_capacity(&input).unwrap()];
    let mut length = 0;
    unsafe {
        assert_eq!(
            swr_v1_http_url_encode(
                input.as_ptr(),
                input.len(),
                encoded.as_mut_ptr(),
                encoded.len(),
                Some(&mut length)
            ),
            0
        );
        assert_eq!(
            swr_v1_http_url_decode(encoded.as_mut_ptr(), length, Some(&mut length)),
            0
        );
    }
    assert_eq!(&encoded[..length], input);
    assert_eq!(encoded[length], 0);
    let mut malformed = b"%+%0%GG%a0+%00!\0".to_vec();
    unsafe {
        assert_eq!(
            swr_v1_http_url_decode(
                malformed.as_mut_ptr(),
                malformed.len() - 1,
                Some(&mut length)
            ),
            0
        );
    }
    assert_eq!(&malformed[..length], b"% %0%GG\xa0 \0!");
}
#[test]
fn errors_are_atomic_and_empty_encoding_has_terminator_storage() {
    let mut output = [99; 4];
    let mut length = 7;
    unsafe {
        assert_eq!(
            swr_v1_http_url_encode(b"!!".as_ptr(), 2, output.as_mut_ptr(), 4, Some(&mut length)),
            INVALID
        );
        assert_eq!(output, [99; 4]);
        assert_eq!(length, 0);
        assert_eq!(
            swr_v1_http_url_capacity(std::ptr::null(), 0, Some(&mut length)),
            0
        );
        assert_eq!(length, 1);
        assert_eq!(
            swr_v1_http_url_encode(
                std::ptr::null(),
                0,
                output.as_mut_ptr(),
                1,
                Some(&mut length)
            ),
            0
        );
        assert_eq!(output[0], 0);
    }
}
#[test]
fn ffi_rejects_overflow_before_access_and_extreme_method_ids_do_not_panic() {
    let mut written = 9;
    unsafe {
        assert_eq!(
            swr_v1_http_url_capacity(std::ptr::null(), usize::MAX, Some(&mut written)),
            OVERFLOW
        );
        assert_eq!(written, 0);
        assert_eq!(
            swr_v1_http_url_decode(std::ptr::null_mut(), usize::MAX, Some(&mut written)),
            OVERFLOW
        );
        assert_eq!(written, 0);
        assert_eq!(swr_v1_http_url_capacity(std::ptr::null(), 0, None), INVALID);
    }
    assert!(swr_v1_http_method_string(i32::MIN).is_null());
    assert!(swr_v1_http_method_string(i32::MAX).is_null());
}
#[test]
fn boundary_preserves_quotes_spaces_and_parameter_rules() {
    for (input, offset, expected) in [
        (&b"x; boundary=\"abc\"; foo=x"[..], 0, Some(&b"abc"[..])),
        (&b"BounDary=a "[..], 0, Some(&b"a "[..])),
        (&b"boundary=\"\""[..], 0, None),
        (&b"x=a"[..], 0, None),
        (&b"boundary=x"[..], 99, None),
    ] {
        let mut result = HttpSpan::default();
        let status =
            unsafe { swr_v1_http_boundary(input.as_ptr(), input.len(), offset, Some(&mut result)) };
        assert_eq!(status, i32::from(expected.is_some()));
        if let Some(expected) = expected {
            assert_eq!(
                &input[result.offset..result.offset + result.length],
                expected
            );
        }
    }
}
#[test]
fn known_identity_and_repeated_unknown_backing_are_preserved() {
    let text = |pointer| unsafe { std::ffi::CStr::from_ptr(pointer) }.to_bytes();
    let first = swr_v2_http_status_message(-701);
    assert_eq!(text(first), b"-701 Unknown Status");
    assert_eq!(text(swr_v2_http_status_message(200)), b"200 OK");
    assert_eq!(
        swr_v2_http_status_message(200),
        swr_v2_http_status_message(0),
        "known messages are the table's"
    );
    assert_eq!(text(first), b"-701 Unknown Status");
    let next = swr_v2_http_status_message(-702);
    assert_eq!(first, next);
    assert_eq!(text(first), b"-702 Unknown Status");
}
#[test]
fn multipart_exports_refuse_nulls_and_unterminated_scratch() {
    assert_eq!(
        unsafe { swr_v1_http_multipart_disposition(b"Content-Disposition".as_ptr(), 19) },
        1
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
    let mut scratch = *b"name=x; filename=y\0";
    assert_eq!(
        unsafe { swr_v1_http_multipart_filename(scratch.as_mut_ptr(), scratch.len()) },
        1
    );
    assert_eq!(swr_v1_http_multipart_result(2, 2, 0), 2);
}
