//! The C ABI's contracts: errors leave outputs alone, nulls and overflows are refused.
use super::*;
use crate::http_server::{get_method_string, get_status_message};
use std::mem::MaybeUninit;
/// `swoole_rs_http::url_encode` into `output`, which holds only bytes.
fn encode_into(input: &[u8], output: &mut [u8]) -> Result<usize, i32> {
    let slots: Vec<_> = output.iter().map(|&byte| MaybeUninit::new(byte)).collect();
    let mut slots = slots;
    let result = swoole_rs_http::url_encode(input, &mut slots);
    // SAFETY: every slot started initialized, and the encoder writes only bytes.
    let bytes = slots.iter().map(|slot| unsafe { slot.assume_init() });
    output
        .iter_mut()
        .zip(bytes)
        .for_each(|(to, from)| *to = from);
    result
}
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
            let capacity = swoole_rs_http::url_capacity(&input).unwrap();
            assert_eq!(capacity, expected.len());
            let mut output = vec![0xa5; capacity + 3];
            let written = encode_into(&input, &mut output[..capacity]).unwrap();
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
    let length = encode_into(&input, &mut encoded).unwrap();
    let length = swoole_rs_http::url_decode(&mut encoded[..length]);
    assert_eq!(&encoded[..length], input);
    let mut malformed = b"%+%0%GG%a0+%00!".to_vec();
    let length = swoole_rs_http::url_decode(&mut malformed);
    assert_eq!(&malformed[..length], b"% %0%GG\xa0 \0!");
}
#[test]
fn errors_are_atomic_and_empty_encoding_has_terminator_storage() {
    let mut output = [99; 4];
    assert_eq!(encode_into(b"!!", &mut output), Err(INVALID));
    assert_eq!(output, [99; 4]);
    assert_eq!(swoole_rs_http::url_capacity(&[]), Ok(1));
    assert_eq!(encode_into(&[], &mut output[..1]), Ok(0));
    assert_eq!(output[0], 0);
}
#[test]
fn extreme_method_ids_do_not_panic() {
    assert!(get_method_string(i32::MIN).is_null());
    assert!(get_method_string(i32::MAX).is_null());
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
        let found = swoole_rs_http::boundary(input, offset).unwrap();
        assert_eq!(i32::from(found.is_some()), i32::from(expected.is_some()));
        if let Some(expected) = expected {
            let span = found.unwrap();
            assert_eq!(&input[span.offset..span.offset + span.length], expected);
        }
    }
}
#[test]
fn known_identity_and_repeated_unknown_backing_are_preserved() {
    let text = |pointer| unsafe { std::ffi::CStr::from_ptr(pointer) }.to_bytes();
    let first = get_status_message(-701);
    assert_eq!(text(first), b"-701 Unknown Status");
    assert_eq!(text(get_status_message(200)), b"200 OK");
    assert_eq!(
        get_status_message(200),
        get_status_message(0),
        "known messages are the table's"
    );
    assert_eq!(text(first), b"-701 Unknown Status");
    let next = get_status_message(-702);
    assert_eq!(first, next);
    assert_eq!(text(first), b"-702 Unknown Status");
}
#[test]
fn multipart_exports_refuse_nulls_and_unterminated_scratch() {
    assert_eq!(
        i32::from(swoole_rs_http::multipart_disposition(
            b"Content-Disposition"
        )),
        1
    );
    assert_eq!(swoole_rs_http::multipart_filename(&mut []), INVALID);
    // A null span, as the removed exports' callers could pass, is refused where the
    // remaining ones borrow it.
    assert_eq!(
        // SAFETY: a null span is refused before it is read.
        unsafe { bytes(core::ptr::null(), 1) },
        Err(INVALID)
    );
    let mut missing_nul = *b"filename";
    assert_eq!(
        swoole_rs_http::multipart_filename(&mut missing_nul),
        INVALID
    );
    let mut scratch = *b"name=x; filename=y\0";
    assert_eq!(swoole_rs_http::multipart_filename(&mut scratch), 1);
    assert_eq!(swoole_rs_http::multipart_result(2, 2, 0), 2);
}
