//! Static response header encoding. Native retains allocation, dates and IO.
use crate::proxy::{HttpProxyText, Prefix};
use crate::{INVALID, OVERFLOW, bytes, c_text};
use std::{fmt::Write, mem::MaybeUninit, slice};

#[repr(C)]
pub struct HttpStaticHeaders {
    pub status: HttpProxyText,
    pub date: HttpProxyText,
    pub modified: HttpProxyText,
    pub content_type: HttpProxyText,
    pub software: HttpProxyText,
    pub page: HttpProxyText,
    pub content_length: i64,
    pub range_offset: i64,
    pub range_length: usize,
    pub filesize: usize,
    pub variant: u32,
    pub keep_alive: u32,
    pub range_kind: u32,
}
unsafe fn text<'a>(value: HttpProxyText) -> Result<&'a [u8], i32> {
    // SAFETY: callers pass a header text span naming `length` initialized bytes.
    let input = unsafe { bytes(value.data, value.length) }?;
    Ok(c_text(input))
}
/// Encode 0=404, 1=304, 2=autoindex, 3=file. Range kind: 0=none,
/// 1=Content-Range, 2=Accept-Ranges. Native inputs are borrowed for this call only.
/// # Safety
/// POD/input spans are readable/initialized; output capacity and written are
/// writable and mutually disjoint from all inputs. Output may be uninitialized.
/// No native callback, allocation, panic or retained borrow occurs.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_http_static_headers(
    header: *const HttpStaticHeaders,
    output: *mut u8,
    capacity: usize,
    written: *mut usize,
) -> i32 {
    if header.is_null() || written.is_null() || (capacity != 0 && output.is_null()) {
        return INVALID;
    }
    if capacity > i32::MAX as usize {
        return OVERFLOW;
    }
    // SAFETY: `header` is non-null (checked above), aligned and initialized per the caller contract.
    let header = unsafe { &*header };
    if header.variant > 3 || header.range_kind > 2 {
        return INVALID;
    }
    // Every header span is initialized (see the contract).
    let values = [
        // SAFETY: as above.
        unsafe { text(header.status) },
        // SAFETY: as above.
        unsafe { text(header.date) },
        // SAFETY: as above.
        unsafe { text(header.modified) },
        // SAFETY: as above.
        unsafe { text(header.content_type) },
        // SAFETY: as above.
        unsafe { text(header.software) },
        // SAFETY: as above.
        unsafe { text(header.page) },
    ];
    let [status, date, modified, content_type, software, page] = match values {
        [Ok(a), Ok(b), Ok(c), Ok(d), Ok(e), Ok(f)] => [a, b, c, d, e, f],
        values => return values.into_iter().find_map(Result::err).unwrap_or(INVALID),
    };
    // A conservative bound includes every fixed header and decimal field. Huge
    // unsupported spans fail before mutation rather than overflowing libc int.
    if [status, date, modified, content_type, software, page]
        .into_iter()
        .try_fold(512usize, |sum, value| sum.checked_add(value.len()))
        .is_none_or(|n| n > i32::MAX as usize)
    {
        return OVERFLOW;
    }
    let buffer = if capacity == 0 {
        &mut []
    } else {
        // SAFETY: non-null when `capacity != 0` (checked above), writable, disjoint from inputs.
        unsafe { slice::from_raw_parts_mut(output.cast::<MaybeUninit<u8>>(), capacity - 1) }
    };
    let mut sink = Prefix {
        output: buffer,
        length: 0,
    };
    let result = (|| {
        sink.append(b"HTTP/1.1 ")?;
        sink.append(match header.variant {
            1 => b"304 Not Modified",
            2 => b"200 OK",
            _ => status,
        })?;
        sink.append(b"\r\n")?;
        if header.variant == 0 {
            sink.append(b"Server: ")?;
            sink.append(software)?;
            write!(
                sink,
                "\r\nContent-Length: {}\r\n\r\n",
                header.content_length
            )?;
            return sink.append(page);
        }
        sink.append(if header.keep_alive != 0 {
            b"Connection: keep-alive\r\n"
        } else {
            b"Connection: close\r\n"
        })?;
        if header.variant != 1 {
            write!(
                sink,
                "Content-Length: {}\r\nContent-Type: ",
                header.content_length
            )?;
            sink.append(if header.variant == 2 {
                b"text/html"
            } else {
                content_type
            })?;
            sink.append(b"\r\n")?;
            if header.variant == 3 {
                if header.range_kind == 1 {
                    #[expect(
                        clippy::cast_possible_truncation,
                        clippy::cast_sign_loss,
                        reason = "native size_t arithmetic on the off_t range offset"
                    )]
                    let end = header
                        .range_length
                        .wrapping_add(header.range_offset as usize)
                        .wrapping_sub(1);
                    write!(
                        sink,
                        "Content-Range: bytes {}-{}/{}\r\n",
                        header.range_offset, end, header.filesize
                    )?;
                } else if header.range_kind == 2 {
                    sink.append(b"Accept-Ranges: bytes\r\n")?;
                }
            }
        }
        sink.append(b"Date: ")?;
        sink.append(date)?;
        sink.append(b"\r\nLast-Modified: ")?;
        sink.append(modified)?;
        sink.append(b"\r\nServer: ")?;
        sink.append(software)?;
        sink.append(b"\r\n\r\n")
    })();
    if result.is_err() {
        return OVERFLOW;
    }
    let length = if capacity == 0 {
        sink.length
    } else {
        sink.length.min(capacity - 1)
    };
    if capacity != 0 {
        // SAFETY: `length <= capacity - 1`, inside the writable output.
        unsafe { output.wrapping_add(length).write(0) };
    }
    // SAFETY: `written` is non-null (checked above), aligned and writable per the caller contract.
    unsafe { written.write(length) };
    0
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
    fn header() -> HttpStaticHeaders {
        HttpStaticHeaders {
            status: span(b"206 Partial Content"),
            date: span(b"today"),
            modified: span(b"yesterday"),
            content_type: span(b"text/plain"),
            software: span(b"Swoole"),
            page: span(b""),
            content_length: 0,
            range_offset: 0,
            range_length: 0,
            filesize: 0,
            variant: 3,
            keep_alive: 1,
            range_kind: 1,
        }
    }
    #[test]
    fn exact_query_prefix_terminator_and_legacy_empty_range() {
        let h = header();
        let mut length = 0;
        assert_eq!(
            unsafe {
                swr_v1_http_static_headers(&raw const h, std::ptr::null_mut(), 0, &raw mut length)
            },
            0
        );
        let mut complete = vec![0; length + 1];
        assert_eq!(
            unsafe {
                swr_v1_http_static_headers(
                    &raw const h,
                    complete.as_mut_ptr(),
                    complete.len(),
                    &raw mut length,
                )
            },
            0
        );
        let expected = format!("Content-Range: bytes 0-{}/0\r\n", usize::MAX);
        assert!(String::from_utf8_lossy(&complete).contains(&expected));
        for capacity in 1..complete.len() + 2 {
            let mut output = vec![0xcc; capacity + 1];
            let mut written = 0;
            assert_eq!(
                unsafe {
                    swr_v1_http_static_headers(
                        &raw const h,
                        output.as_mut_ptr(),
                        capacity,
                        &raw mut written,
                    )
                },
                0
            );
            assert_eq!(written, length.min(capacity - 1));
            assert_eq!(&output[..written], &complete[..written]);
            assert_eq!(output[written], 0);
            assert_eq!(output[capacity], 0xcc);
        }
    }
    #[test]
    fn invalid_metadata_leaves_native_storage_unchanged() {
        let mut h = header();
        let mut output = [0xcc; 8];
        let mut count = 79;
        for variant in [4, u32::MAX] {
            h.variant = variant;
            assert_eq!(
                unsafe {
                    swr_v1_http_static_headers(
                        &raw const h,
                        output.as_mut_ptr(),
                        output.len(),
                        &raw mut count,
                    )
                },
                INVALID
            );
            assert_eq!(output, [0xcc; 8]);
            assert_eq!(count, 79);
        }
        h.variant = 0;
        assert_eq!(
            unsafe {
                swr_v1_http_static_headers(
                    &raw const h,
                    output.as_mut_ptr(),
                    usize::MAX,
                    &raw mut count,
                )
            },
            OVERFLOW
        );
        assert_eq!(output, [0xcc; 8]);
        assert_eq!(count, 79);
    }
}
