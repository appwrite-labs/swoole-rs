//! Directory-index markup and filtering; native retains directory IO and storage.
use crate::proxy::{HttpProxyText, Prefix};
use crate::{INVALID, OVERFLOW, bytes, c_text};
use std::{mem::MaybeUninit, slice};

#[repr(C)]
pub struct HttpIndexPage {
    pub path: HttpProxyText,
    pub entry: HttpProxyText,
    pub css: HttpProxyText,
    pub powered: HttpProxyText,
    pub part: u32,
}
/// Encode one page part:0=open,1=directory entry,2=close. Zero capacity queries
/// required bytes; other capacities receive the prefix without a NUL terminator.
/// Entry filtering uses full native strings; displayed %s values stop at NUL.
/// Legacy markup is intentionally unescaped. No URL/HTML policy change is made.
/// # Safety
/// The POD and input spans are readable/initialized. Output capacity and written
/// are writable, aligned where required, and disjoint from inputs/each other.
/// No allocation, callback or borrow escapes. Output may be uninitialized.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_http_index_page(
    page: *const HttpIndexPage,
    output: *mut u8,
    capacity: usize,
    written: *mut usize,
) -> i32 {
    if page.is_null() || written.is_null() || (capacity != 0 && output.is_null()) {
        return INVALID;
    }
    if capacity > isize::MAX as usize {
        return OVERFLOW;
    }
    // SAFETY: `page` is non-null (checked above), aligned and initialized per the caller contract.
    let page = unsafe { &*page };
    if page.part > 2 {
        return INVALID;
    }
    // Every page span is readable and initialized (see the contract).
    let inputs = [
        // SAFETY: as above.
        unsafe { bytes(page.path.data, page.path.length) },
        // SAFETY: as above.
        unsafe { bytes(page.entry.data, page.entry.length) },
        // SAFETY: as above.
        unsafe { bytes(page.css.data, page.css.length) },
        // SAFETY: as above.
        unsafe { bytes(page.powered.data, page.powered.length) },
    ];
    let [path, entry, css, powered] = match inputs {
        [Ok(a), Ok(b), Ok(c), Ok(d)] => [a, b, c, d],
        values => return values.into_iter().find_map(Result::err).unwrap_or(INVALID),
    };
    let skip = page.part == 1 && (entry == b"." || path == b"/" && entry == b"..");
    let pieces: &[&[u8]] = match page.part {
        0 => &[
            b"<html>\n<head>\n\t<meta charset='UTF-8'>\n<title>Index of ",
            c_text(path),
            b"</title></head>\n<body>\n",
            css,
            b"<h1>Index of ",
            c_text(path),
            b"</h1>\t<ul>\n",
        ],
        1 if !skip => &[
            b"\t\t<li><a href=",
            c_text(path),
            c_text(entry),
            b">",
            c_text(entry),
            b"</a></li>\n",
        ],
        2 => &[b"\t</ul>\n", powered, b"</body>\n</html>\n"],
        _ => &[],
    };
    // Native format_impl's result is int. Reject unsupported huge fragments
    // before mutation; ordinary page totals remain native size_t.
    if pieces
        .iter()
        .try_fold(0usize, |sum, piece| sum.checked_add(piece.len()))
        .is_none_or(|n| n > i32::MAX as usize)
    {
        return OVERFLOW;
    }
    let buffer = if capacity == 0 {
        &mut []
    } else {
        // SAFETY: non-null when `capacity != 0` (checked above), `capacity <= isize::MAX`,
        // writable and disjoint from the inputs.
        unsafe { slice::from_raw_parts_mut(output.cast::<MaybeUninit<u8>>(), capacity) }
    };
    let mut sink = Prefix {
        output: buffer,
        length: 0,
    };
    for piece in pieces {
        // The preflight sum guarantees append cannot overflow.
        if sink.append(piece).is_err() {
            return OVERFLOW;
        }
    }
    // SAFETY: `written` is non-null (checked above), aligned and writable per the caller contract.
    unsafe { written.write(sink.length) };
    0
}

// One complete append operation for both real callers. The renderer is always
// Rust code; it invokes no external callback or Native allocator/effect. Only
// caller-owned backing is written, without reading its uninitialized suffix.
// SAFETY: each public adapter supplies independent valid input/output extents.
pub(crate) unsafe fn emit_page(
    input: *const HttpIndexPage,
    queried: Option<usize>,
    buffer: *mut u8,
    length: usize,
    capacity: usize,
    output: *mut crate::HttpContextPageBuffer,
    mut render: impl FnMut(*mut u8, usize, *mut usize) -> i32,
) -> i32 {
    // SAFETY: `output` is null or live, aligned and exclusively lent for this call.
    let Some(output) = (unsafe { output.as_mut() }) else {
        return INVALID;
    };
    *output = crate::HttpContextPageBuffer::default();
    // SAFETY: `input` is null or live and aligned for this call.
    let Some(input) = (unsafe { input.as_ref() }) else {
        return INVALID;
    };
    if input.part > 2 || length > capacity || capacity > isize::MAX as usize {
        return INVALID;
    }
    let count = if let Some(count) = queried {
        count
    } else {
        let mut count = 0;
        let result = render(std::ptr::null_mut(), 0, &raw mut count);
        if result < 0 {
            return result;
        }
        count
    };
    if count == 0 {
        return 0;
    }
    let terminator = usize::from(input.part != 2);
    let Some(required) = length
        .checked_add(count)
        .and_then(|value| value.checked_add(terminator))
    else {
        return OVERFLOW;
    };
    output.required = required;
    if required > capacity {
        return if input.part == 2 || capacity == 0 {
            1
        } else {
            2
        };
    }
    if buffer.is_null() {
        return INVALID;
    }
    let mut written = 0;
    // In range: `length + count + terminator == required <= capacity`.
    let result = render(buffer.wrapping_add(length), count, &raw mut written);
    if result < 0 {
        return result;
    }
    let Some(published) = length.checked_add(written).filter(|&value| {
        value
            .checked_add(terminator)
            .is_some_and(|end| end <= capacity)
    }) else {
        return OVERFLOW;
    };
    output.length = published;
    if terminator != 0 {
        // SAFETY: `published + terminator <= capacity`, inside the writable buffer.
        unsafe { buffer.wrapping_add(published).write(0) };
    }
    3
}

#[cfg(test)]
mod tests {
    use super::*;
    /// The emit protocol over a queried legacy page part.
    unsafe fn emit(
        page: *const HttpIndexPage,
        queried: usize,
        buffer: *mut u8,
        length: usize,
        capacity: usize,
        output: *mut crate::HttpContextPageBuffer,
    ) -> i32 {
        let render =
            |data, size, written| unsafe { swr_v1_http_index_page(page, data, size, written) };
        unsafe {
            emit_page(
                page,
                Some(queried),
                buffer,
                length,
                capacity,
                output,
                render,
            )
        }
    }
    fn span(input: &[u8]) -> HttpProxyText {
        HttpProxyText {
            data: input.as_ptr(),
            length: input.len(),
        }
    }
    #[test]
    fn legacy_append_reuses_complete_buffer_policy_and_preserves_canaries() {
        for part in 0..3 {
            let page = HttpIndexPage {
                path: span(b"/ordinary/"),
                entry: span(b"a.txt"),
                css: span(b"css"),
                powered: span(b"powered"),
                part,
            };
            let mut count = 0;
            assert_eq!(
                unsafe {
                    swr_v1_http_index_page(&raw const page, std::ptr::null_mut(), 0, &raw mut count)
                },
                0
            );
            let mut expected = vec![0; count];
            assert_eq!(
                unsafe {
                    swr_v1_http_index_page(
                        &raw const page,
                        expected.as_mut_ptr(),
                        count,
                        &raw mut count,
                    )
                },
                0
            );
            let mut output = b"prefix".to_vec();
            let mut result = crate::HttpContextPageBuffer::default();
            assert_eq!(
                unsafe {
                    emit(
                        &raw const page,
                        count,
                        output.as_mut_ptr(),
                        6,
                        6,
                        &raw mut result,
                    )
                },
                if part == 2 { 1 } else { 2 }
            );
            assert_eq!(output, b"prefix");
            let terminator = usize::from(part != 2);
            assert_eq!(result.required, 6 + count + terminator);
            output.resize(result.required + 17, 0xcc); // Native growth occurs after return.
            let capacity = output.len();
            assert_eq!(
                unsafe {
                    emit(
                        &raw const page,
                        count,
                        output.as_mut_ptr(),
                        6,
                        capacity,
                        &raw mut result,
                    )
                },
                3
            );
            assert_eq!(&output[..6], b"prefix");
            assert_eq!(result.length, 6 + count);
            assert_eq!(&output[6..result.length], expected);
            if terminator != 0 {
                assert_eq!(output[result.length], 0);
            }
            assert!(output[result.required..].iter().all(|&byte| byte == 0xcc));
        }
    }
    #[test]
    fn native_filtering_and_c_string_display_are_distinct() {
        let mut page = HttpIndexPage {
            path: span(b"/"),
            entry: span(b".."),
            css: span(b""),
            powered: span(b""),
            part: 1,
        };
        let mut length = 99;
        assert_eq!(
            unsafe {
                swr_v1_http_index_page(&raw const page, std::ptr::null_mut(), 0, &raw mut length)
            },
            0
        );
        assert_eq!(length, 0);
        page.entry = span(b"..\0name");
        let expected = b"\t\t<li><a href=/..>..</a></li>\n";
        for capacity in 0..expected.len() + 2 {
            let mut output = vec![0xcc; capacity + 1];
            assert_eq!(
                unsafe {
                    swr_v1_http_index_page(
                        &raw const page,
                        output.as_mut_ptr(),
                        capacity,
                        &raw mut length,
                    )
                },
                0
            );
            assert_eq!(length, expected.len());
            let prefix = capacity.min(expected.len());
            assert_eq!(&output[..prefix], &expected[..prefix]);
            assert_eq!(output[capacity], 0xcc);
        }
    }
}
