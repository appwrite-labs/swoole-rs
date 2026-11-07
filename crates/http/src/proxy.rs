//! CONNECT request formatting and the pinned proxy response state machine.
use crate::{INVALID, OVERFLOW, bytes, c_text, tail};
use std::{
    fmt::{self, Write},
    mem::MaybeUninit,
    slice,
};

#[repr(C)]
#[derive(Clone, Copy)]
pub struct HttpProxyText {
    pub data: *const u8,
    pub length: usize,
}
#[repr(C)]
pub struct HttpProxyRequest {
    pub target: HttpProxyText,
    pub host: HttpProxyText,
    pub version: HttpProxyText,
    pub authorization: HttpProxyText,
    pub port: i32,
    pub authenticated: u32,
}
/// snprintf-style byte sink: count everything; write only the available prefix.
pub(super) struct Prefix<'a> {
    pub(super) output: &'a mut [MaybeUninit<u8>],
    pub(super) length: usize,
}
impl Prefix<'_> {
    pub(super) fn append(&mut self, input: &[u8]) -> fmt::Result {
        for (destination, byte) in self.output.iter_mut().skip(self.length).zip(input) {
            destination.write(*byte);
        }
        self.length = self.length.checked_add(input.len()).ok_or(fmt::Error)?;
        Ok(())
    }
}
impl Write for Prefix<'_> {
    fn write_str(&mut self, input: &str) -> fmt::Result {
        self.append(input.as_bytes())
    }
}
unsafe fn text<'a>(value: HttpProxyText) -> Result<&'a [u8], i32> {
    // Native %.*s stops at NUL even though std::string may contain binary bytes.
    if value.length > isize::MAX as usize {
        return Err(OVERFLOW);
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap,
        reason = "native %.*s takes an int precision"
    )]
    let precision = value.length as i32;
    let length = if precision < 0 {
        value.length
    } else {
        precision.unsigned_abs() as usize
    };
    // SAFETY: callers pass a text span whose `length` (or precision) bytes are initialized.
    let input = unsafe { bytes(value.data, length) }?;
    Ok(c_text(input))
}
/// Format the pinned fixed256-byte username:password staging buffer. Native
/// base64/return-string ownership follows after this call. The useful prefix
/// never exceeds255 bytes, so credentials beyond it need not be scanned.
/// # Safety
/// credentials points to two initialized spans (username,password), output is
/// writable for256 bytes and written is writable/aligned. Outputs and inputs are
/// mutually disjoint. Nothing is allocated, called back or retained.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_http_proxy_credentials(
    credentials: *const HttpProxyText,
    output: *mut u8,
    written: *mut usize,
) -> i32 {
    if credentials.is_null() || output.is_null() || written.is_null() {
        return INVALID;
    }
    // SAFETY: non-null (checked above); the caller lends two initialized spans.
    let credentials = unsafe { &*credentials.cast::<[HttpProxyText; 2]>() };
    for value in credentials {
        if value.length > isize::MAX as usize {
            return OVERFLOW;
        }
        if value.data.is_null() && value.length != 0 {
            return INVALID;
        }
        if (value.data as usize).checked_add(value.length).is_none() {
            return OVERFLOW;
        }
    }
    let field_limit = |value: &HttpProxyText| {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_possible_wrap,
            reason = "native %.*s takes an int precision"
        )]
        let precision = value.length as i32;
        if precision < 0 {
            value.length
        } else {
            precision.unsigned_abs() as usize
        }
    };
    // Only a potentially int-overflowing formatted count needs full C-string
    // lengths. Native snprintf would reject that count even with a short buffer.
    if credentials
        .iter()
        .try_fold(1usize, |sum, value| sum.checked_add(field_limit(value)))
        .is_none_or(|length| length > i32::MAX as usize)
    {
        // Both credential spans are initialized (see the contract).
        // SAFETY: as above.
        let user = unsafe { text(credentials[0]) };
        // SAFETY: as above.
        let password = unsafe { text(credentials[1]) };
        let lengths = [user, password];
        let [Ok(user), Ok(password)] = lengths else {
            return OVERFLOW;
        };
        if user
            .len()
            .checked_add(password.len())
            .and_then(|length| length.checked_add(1))
            .is_none_or(|length| length > i32::MAX as usize)
        {
            // SAFETY: both are non-null (checked above), writable and disjoint.
            unsafe { output.write(0) };
            // SAFETY: as above.
            unsafe { written.write(0) };
            return 1; // Native publishes the platform EOVERFLOW errno.
        }
    }
    // SAFETY: non-null (checked above); the caller lends 256 writable bytes, disjoint from inputs.
    let buffer = unsafe { slice::from_raw_parts_mut(output.cast::<MaybeUninit<u8>>(), 255) };
    let mut sink = Prefix {
        output: buffer,
        length: 0,
    };
    for (index, value) in credentials.iter().enumerate() {
        if index == 1 && sink.length < 255 {
            #[expect(
                clippy::let_underscore_must_use,
                reason = "the 255-byte cap keeps the length from overflowing"
            )]
            let _ = sink.append(b":");
        }
        let limit = field_limit(value).min(255 - sink.length);
        // Validated metadata and a capped native precision bound the only read.
        // SAFETY: validated span metadata; `limit` never exceeds the native precision.
        let input = match unsafe { bytes(value.data, limit) } {
            Ok(input) => input,
            Err(error) => return error,
        };
        let input = c_text(input);
        #[expect(
            clippy::let_underscore_must_use,
            reason = "the 255-byte cap keeps the length from overflowing"
        )]
        let _ = sink.append(input);
    }
    // SAFETY: `sink.length <= 255`, inside the 256 writable output bytes.
    unsafe { output.wrapping_add(sink.length).write(0) };
    // SAFETY: non-null (checked above), aligned and writable.
    unsafe { written.write(sink.length) };
    0
}

/// Complete credential formatting and Base64 serialization in caller storage.
/// Native string construction and errno publication follow after return.
/// # Safety
/// Same input contract as `swr_v1_http_proxy_credentials`; output is writable
/// for 512 bytes, written is aligned/writable, and all outputs are disjoint.
/// No allocation, callback, pointer or borrow escapes this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v2_http_proxy_authorization(
    credentials: *const HttpProxyText,
    output: *mut u8,
    written: *mut usize,
) -> i32 {
    if output.is_null() || written.is_null() {
        return INVALID;
    }
    let mut plain = [0u8; 256];
    let mut count = 0;
    // SAFETY: forwards the caller's credentials; `plain` and `count` are disjoint locals.
    let status =
        unsafe { swr_v1_http_proxy_credentials(credentials, plain.as_mut_ptr(), &raw mut count) };
    if status < 0 {
        return status;
    }
    // At most 255 bytes plus terminator: the fixed 512-byte output is ample.
    // SAFETY: `count <= 255` initialized bytes in `plain`; `output` is non-null with 512
    // writable bytes, enough for the encoding plus NUL, and disjoint from `plain`.
    let encoded = unsafe { swoole_rs_base64::swr_v1_base64_encode(plain.as_ptr(), count, output) };
    // SAFETY: `written` is non-null (checked above), aligned and writable per the caller contract.
    unsafe { written.write(encoded) };
    status
}

/// Format a native-owned CONNECT request, returning snprintf's clamped length.
/// Capacity zero queries the full length. Any nonzero capacity includes the NUL.
/// # Safety
/// Request and length output are aligned/readable/writable respectively. Text
/// spans are initialized and disjoint from writable output capacity and length.
/// Output may be uninitialized. No allocation, callback, or borrow escapes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_http_proxy_pack(
    request: *const HttpProxyRequest,
    output: *mut u8,
    capacity: usize,
    written: *mut usize,
) -> i32 {
    if request.is_null() || written.is_null() || (capacity != 0 && output.is_null()) {
        return INVALID;
    }
    if capacity > i32::MAX as usize {
        return OVERFLOW;
    }
    // SAFETY: `request` is non-null (checked above), aligned and initialized per the caller contract.
    let request = unsafe { &*request };
    // Every request text span is initialized (see the contract).
    let inputs = [
        // SAFETY: as above.
        unsafe { text(request.target) },
        // SAFETY: as above.
        unsafe { text(request.host) },
        // SAFETY: as above.
        unsafe { text(request.version) },
        // SAFETY: as above.
        unsafe { text(request.authorization) },
    ];
    let [target, host, version, authorization] = match inputs {
        [Ok(a), Ok(b), Ok(c), Ok(d)] => [a, b, c, d],
        values => return values.into_iter().find_map(Result::err).unwrap_or(INVALID),
    };
    // libc's formatted-result type is int. Reject larger results/capacities
    // before writing, rather than reproducing signed narrowing of huge buffers.
    let digits = request.port.unsigned_abs().checked_ilog10().unwrap_or(0) as usize
        + 1
        + usize::from(request.port < 0);
    let fixed = b"CONNECT : HTTP/1.1\r\nHost: :\r\nUser-Agent: Swoole/\r\nProxy-Connection: Keep-Alive\r\n\r\n".len() + 2 * digits;
    let lengths = [
        target.len(),
        host.len(),
        version.len(),
        if request.authenticated == 0 {
            0
        } else {
            b"Proxy-Authorization: Basic ".len() + authorization.len() + 2
        },
    ];
    if lengths
        .into_iter()
        .try_fold(fixed, usize::checked_add)
        .is_none_or(|n| n > i32::MAX as usize)
    {
        return OVERFLOW;
    }
    let buffer = if capacity == 0 {
        &mut []
    } else {
        // SAFETY: non-null when `capacity != 0` (checked above), `capacity <= i32::MAX`, writable.
        unsafe { slice::from_raw_parts_mut(output.cast::<MaybeUninit<u8>>(), capacity - 1) }
    };
    let mut prefix = Prefix {
        output: buffer,
        length: 0,
    };
    let result = (|| {
        prefix.append(b"CONNECT ")?;
        prefix.append(target)?;
        write!(prefix, ":{} HTTP/1.1\r\nHost: ", request.port)?;
        prefix.append(host)?;
        write!(prefix, ":{}\r\nUser-Agent: Swoole/", request.port)?;
        prefix.append(version)?;
        prefix.append(b"\r\nProxy-Connection: Keep-Alive\r\n")?;
        if request.authenticated != 0 {
            prefix.append(b"Proxy-Authorization: Basic ")?;
            prefix.append(authorization)?;
            prefix.append(b"\r\n")?;
        }
        prefix.append(b"\r\n")
    })();
    if result.is_err() {
        return OVERFLOW;
    }
    let length = if capacity == 0 {
        prefix.length
    } else {
        prefix.length.min(capacity - 1)
    };
    if capacity != 0 {
        // SAFETY: `length <= capacity - 1`, inside the writable output.
        unsafe { output.wrapping_add(length).write(0) };
    }
    // SAFETY: `written` is non-null (checked above), aligned and writable per the caller contract.
    unsafe { written.write(length) };
    0
}
/// Pinned handshake: 1 accepted, -2 rejected status (native sets last-error),
/// 0 other/incomplete response; minimum length and cursor skips are intentional.
/// # Safety
/// Input is initialized/readable for length; no borrow escapes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_http_proxy_handshake(data: *const u8, length: usize) -> i32 {
    // SAFETY: the caller lends `length` initialized bytes at `data` (null only when empty).
    let Ok(input) = (unsafe { bytes(data, length) }) else {
        return INVALID;
    };
    if input.len() < b"HTTP/1.1 200 Connection established\r\n".len() {
        return 0;
    }
    if !crate::case_prefix(input, b"HTTP/1.1") && !crate::case_prefix(input, b"HTTP/1.0") {
        return 0;
    }
    // The original loop skips index 8 after recognizing HTTP/1.x.
    let Some(cursor) = tail(input, 9)
        .iter()
        .position(|b| !matches!(b, b' ' | b'\t' | b'\r' | b'\n' | 0x0b | 0x0c))
    else {
        return 0;
    };
    let cursor = cursor + 9;
    if !tail(input, cursor).starts_with(b"200") {
        return -2;
    }
    i32::from(input.len() - cursor > 4)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn span(value: &[u8]) -> HttpProxyText {
        HttpProxyText {
            data: value.as_ptr(),
            length: value.len(),
        }
    }
    #[test]
    fn credentials_preserve_fixed_prefix_and_native_nul_rules() {
        for size in [0, 1, 253, 254, 255, 256, 1024] {
            let username = vec![b'u'; size];
            let password = b"password\0ignored";
            let credentials = [span(&username), span(password)];
            let mut output = [0xcc; 257];
            let mut written = 77;
            assert_eq!(
                unsafe {
                    swr_v1_http_proxy_credentials(
                        credentials.as_ptr(),
                        output.as_mut_ptr(),
                        &raw mut written,
                    )
                },
                0
            );
            let mut expected = username.clone();
            expected.push(b':');
            expected.extend_from_slice(b"password");
            expected.truncate(255);
            assert_eq!(written, expected.len());
            assert_eq!(&output[..written], expected);
            assert_eq!(output[written], 0);
            assert_eq!(output[256], 0xcc);
        }
        let credentials = [
            HttpProxyText {
                data: std::ptr::null(),
                length: 1,
            },
            span(b"valid"),
        ];
        let mut output = [0xcc; 256];
        let mut written = 77;
        assert_eq!(
            unsafe {
                swr_v1_http_proxy_credentials(
                    credentials.as_ptr(),
                    output.as_mut_ptr(),
                    &raw mut written,
                )
            },
            INVALID
        );
        assert_eq!(output, [0xcc; 256]);
        assert_eq!(written, 77);
    }
    #[test]
    fn authorization_reuses_codec_and_keeps_output_bounds() {
        let credentials = [span(b"user"), span(b"password")];
        let mut output = [0xcc; 513];
        let mut count = 91;
        assert_eq!(
            unsafe {
                swr_v2_http_proxy_authorization(
                    credentials.as_ptr(),
                    output.as_mut_ptr(),
                    &raw mut count,
                )
            },
            0
        );
        assert_eq!(&output[..count], b"dXNlcjpwYXNzd29yZA==");
        assert_eq!(output[count], 0);
        assert_eq!(output[512], 0xcc);
        let empty = [span(b""), span(b"")];
        assert_eq!(
            unsafe {
                swr_v2_http_proxy_authorization(empty.as_ptr(), output.as_mut_ptr(), &raw mut count)
            },
            0
        );
        assert_eq!(&output[..count], b"Og==");
    }
    #[test]
    fn pack_prefix_and_query() {
        let request = HttpProxyRequest {
            target: span(b"t\0ignored"),
            host: span(b"h"),
            version: span(b"6.2.2"),
            authorization: span(b"YWJj"),
            port: i32::MIN,
            authenticated: 1,
        };
        let expected=b"CONNECT t:-2147483648 HTTP/1.1\r\nHost: h:-2147483648\r\nUser-Agent: Swoole/6.2.2\r\nProxy-Connection: Keep-Alive\r\nProxy-Authorization: Basic YWJj\r\n\r\n";
        let mut count = 0;
        assert_eq!(
            unsafe {
                swr_v1_http_proxy_pack(&raw const request, std::ptr::null_mut(), 0, &raw mut count)
            },
            0
        );
        assert_eq!(count, expected.len());
        for capacity in 1..=expected.len() + 2 {
            let mut buffer = vec![0xcc; capacity + 1];
            assert_eq!(
                unsafe {
                    swr_v1_http_proxy_pack(
                        &raw const request,
                        buffer.as_mut_ptr(),
                        capacity,
                        &raw mut count,
                    )
                },
                0
            );
            assert_eq!(count, expected.len().min(capacity - 1));
            assert_eq!(&buffer[..count], &expected[..count]);
            assert_eq!(buffer[count], 0);
            assert_eq!(buffer[capacity], 0xcc);
        }
    }
    #[test]
    fn invalid_capacity_does_not_access_or_mutate_output() {
        let request = HttpProxyRequest {
            target: span(b"t"),
            host: span(b"h"),
            version: span(b"v"),
            authorization: span(b""),
            port: 80,
            authenticated: 0,
        };
        let mut output = [0xcc; 4];
        let mut count = 91;
        assert_eq!(
            unsafe {
                swr_v1_http_proxy_pack(
                    &raw const request,
                    output.as_mut_ptr(),
                    usize::MAX,
                    &raw mut count,
                )
            },
            OVERFLOW
        );
        assert_eq!(output, [0xcc; 4]);
        assert_eq!(count, 91);
        assert_eq!(
            unsafe {
                swr_v1_http_proxy_pack(&raw const request, std::ptr::null_mut(), 1, &raw mut count)
            },
            INVALID
        );
        assert_eq!(count, 91);
        let mut huge = request;
        huge.target.length = isize::MAX as usize + 1;
        assert_eq!(
            unsafe {
                swr_v1_http_proxy_pack(
                    &raw const huge,
                    output.as_mut_ptr(),
                    output.len(),
                    &raw mut count,
                )
            },
            OVERFLOW
        );
        assert_eq!(output, [0xcc; 4]);
        assert_eq!(count, 91);
        assert_eq!(
            unsafe { swr_v1_http_proxy_handshake(std::ptr::null(), 0) },
            0
        );
    }
    #[test]
    fn handshake_cursor_and_error_policy() {
        let check =
            |input: &[u8]| unsafe { swr_v1_http_proxy_handshake(input.as_ptr(), input.len()) };
        assert_eq!(check(b"HTTP/1.1 200 Connection established\r\n"), 1);
        assert_eq!(check(b"http/1.0X200 Connection established\r\n"), 1);
        assert_eq!(check(b"HTTP/1.1 407 Proxy authorization needed\r\n"), -2);
        assert_eq!(check(b"HTTP/1.1 200"), 0);
        assert_eq!(check(b"NOT HTTP 200 Connection established\r\n"), 0);
        let mut input = b"HTTP/1.1 ".to_vec();
        input.resize(36, b' ');
        assert_eq!(check(&input), 0);
        input.extend_from_slice(b"200");
        assert_eq!(check(&input), 0);
        input.push(b'!');
        assert_eq!(check(&input), 0);
        input.push(b'!');
        assert_eq!(check(&input), 1);
    }
}
