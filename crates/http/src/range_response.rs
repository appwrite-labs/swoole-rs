//! Range response assembly/commit policy. Native owns containers and date/IO work.
use crate::proxy::{HttpProxyText, Prefix};
use crate::{INVALID, OVERFLOW, bytes, c_text};
use std::{fmt::Write, mem::MaybeUninit, slice};

#[repr(C)]
pub struct HttpRangeFormat {
    pub boundary: HttpProxyText,
    pub mime: HttpProxyText,
    pub offset: usize,
    pub length: usize,
    pub filesize: usize,
    pub part: u32,
    pub separator: u32,
}

fn render(
    input: &HttpRangeFormat,
    boundary: &[u8],
    mime: &[u8],
    out: &mut Prefix<'_>,
) -> std::fmt::Result {
    if input.part == 1 {
        out.append(b"\r\n--")?;
        out.append(boundary)?;
        return out.append(b"--\r\n");
    }
    if input.separator != 0 {
        out.append(b"\r\n")?;
    }
    out.append(b"--")?;
    out.append(boundary)?;
    out.append(b"\r\nContent-Type: ")?;
    out.append(mime)?;
    write!(
        out,
        "\r\nContent-Range: bytes {}-{}/{}\r\n\r\n",
        input.offset,
        input.offset.wrapping_add(input.length).wrapping_sub(1),
        input.filesize
    )
}

// Count the exact wire representation before touching output, preserving the
// formatter's transactional overflow boundary without formatting every number
// twice. The integer count includes zero and the wrapping last-byte index.
fn rendered_length(input: &HttpRangeFormat, boundary: &[u8], mime: &[u8]) -> Option<usize> {
    let fixed = if input.part == 1 {
        b"\r\n----\r\n".len()
    } else {
        let digits = |value: usize| value.checked_ilog10().map_or(1, |n| n as usize + 1);
        b"--\r\nContent-Type: \r\nContent-Range: bytes -/\r\n\r\n".len()
            + usize::from(input.separator != 0) * 2
            + digits(input.offset)
            + digits(input.offset.wrapping_add(input.length).wrapping_sub(1))
            + digits(input.filesize)
    };
    boundary.len().checked_add(mime.len())?.checked_add(fixed)
}

/// Format0=a snprintf-style range header,1=a binary-safe final boundary string.
/// Capacity0 queries size; otherwise write the available prefix and final NUL.
/// Return1 requests native EOVERFLOW for a header count beyond libc int;
/// preserve the snprintf prefix, but publish the pinned empty C-string/count0.
/// # Safety
/// Input/spans are readable/initialized. Output capacity and written are writable,
/// disjoint from all input/state. No allocation, callback or retained borrow.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_http_range_format(
    input: *const HttpRangeFormat,
    output: *mut u8,
    capacity: usize,
    written: *mut usize,
) -> i32 {
    if input.is_null() || written.is_null() || (capacity != 0 && output.is_null()) {
        return INVALID;
    }
    if capacity > isize::MAX as usize {
        return OVERFLOW;
    }
    // SAFETY: `input` is non-null (checked above), aligned and initialized per the caller contract.
    let input = unsafe { &*input };
    if input.part > 1 {
        return INVALID;
    }
    // SAFETY: the boundary span names `length` initialized bytes.
    let boundary = match unsafe { bytes(input.boundary.data, input.boundary.length) } {
        Ok(value) => value,
        Err(error) => return error,
    };
    if input.part == 0 && capacity > i32::MAX as usize {
        return OVERFLOW;
    }
    let (boundary, mime) = if input.part == 0 {
        // SAFETY: the MIME span names `length` initialized bytes.
        let mime = match unsafe { bytes(input.mime.data, input.mime.length) } {
            Ok(value) => value,
            Err(error) => return error,
        };
        (c_text(boundary), c_text(mime))
    } else {
        (boundary, &[][..])
    };
    let Some(required) =
        rendered_length(input, boundary, mime).filter(|&length| isize::try_from(length).is_ok())
    else {
        return OVERFLOW;
    };
    let libc_overflow = input.part == 0 && required > i32::MAX as usize;
    let count = if capacity == 0 {
        if libc_overflow { usize::MAX } else { required }
    } else {
        // SAFETY: non-null when `capacity != 0` (checked above), writable, disjoint from inputs.
        let output_slice =
            unsafe { slice::from_raw_parts_mut(output.cast::<MaybeUninit<u8>>(), capacity - 1) };
        let mut sink = Prefix {
            output: output_slice,
            length: 0,
        };
        if render(input, boundary, mime, &mut sink).is_err() {
            return OVERFLOW;
        }
        let count = sink.length.min(capacity - 1);
        // SAFETY: `count <= capacity - 1`, inside the writable output.
        unsafe { output.wrapping_add(count).write(0) };
        if libc_overflow {
            // SAFETY: `output` is non-null (checked above), aligned and writable per the caller contract.
            unsafe { output.write(0) };
            0
        } else {
            count
        }
    };
    // SAFETY: `written` is non-null (checked above), aligned and writable per the caller contract.
    unsafe { written.write(count) };
    i32::from(libc_overflow)
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HttpRangeFinish {
    pub content_length: usize,
    pub last_length: usize,
    pub task_count: usize,
    pub filesize: usize,
    pub end_length: usize,
    pub phase: u32,
    pub status: i32,
    pub if_range: u32,
    pub modified: u32,
}

impl HttpRangeFinish {
    pub(crate) fn step(&mut self) -> i32 {
        if self.phase > 6 || (self.last_length != 0 && self.task_count == 0) {
            return INVALID;
        }
        self.next()
    }
    fn check_date(&mut self) -> i32 {
        if self.if_range != 0 {
            self.phase = 3;
            3
        } else {
            self.phase = 6;
            0
        }
    }
    fn next(&mut self) -> i32 {
        match self.phase {
            0 if self.last_length == 0 => {
                self.content_length = self.filesize;
                self.phase = 1;
                1
            }
            0 if self.task_count == 1 => {
                self.content_length = self.last_length;
                self.status = 206;
                self.check_date()
            }
            0 => {
                self.phase = 2;
                2
            }
            1 => self.check_date(),
            2 => {
                self.content_length = self.content_length.wrapping_add(self.end_length);
                self.status = 206;
                self.check_date()
            }
            3 if self.modified != 0 => {
                self.phase = 4;
                4
            }
            3 => {
                self.phase = 6;
                0
            }
            4 => {
                self.content_length = self.filesize;
                self.phase = 5;
                1
            }
            5 => {
                self.status = 200;
                self.phase = 6;
                0
            }
            _ => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn text(value: &[u8]) -> HttpProxyText {
        HttpProxyText {
            data: value.as_ptr(),
            length: value.len(),
        }
    }
    #[test]
    fn binary_end_and_c_string_header_are_different() {
        let mut input = HttpRangeFormat {
            boundary: text(b"ab\0cd"),
            mime: text(b"type\0ignored"),
            offset: 3,
            length: 5,
            filesize: 10,
            part: 0,
            separator: 1,
        };
        let mut output = [0xa5; 128];
        let mut n = 0;
        assert_eq!(
            unsafe {
                swr_v1_http_range_format(
                    &raw const input,
                    output.as_mut_ptr(),
                    output.len(),
                    &raw mut n,
                )
            },
            0
        );
        assert_eq!(
            &output[..n],
            b"\r\n--ab\r\nContent-Type: type\r\nContent-Range: bytes 3-7/10\r\n\r\n"
        );
        input.part = 1;
        assert_eq!(
            unsafe {
                swr_v1_http_range_format(
                    &raw const input,
                    output.as_mut_ptr(),
                    output.len(),
                    &raw mut n,
                )
            },
            0
        );
        assert_eq!(&output[..n], b"\r\n--ab\0cd--\r\n");
    }
    #[test]
    fn positive_wire_size_and_all_initialized_prefix_capacities() {
        for offset in [0usize, 9, 10, 99, 100, usize::MAX] {
            for length in [0usize, 1, 17] {
                for filesize in [0usize, 10, usize::MAX] {
                    for part in [0, 1] {
                        for separator in [0, 1] {
                            let input = HttpRangeFormat {
                                boundary: text(b"boundary"),
                                mime: text(b"text/plain"),
                                offset,
                                length,
                                filesize,
                                part,
                                separator,
                            };
                            let expected = if part == 1 {
                                "\r\n--boundary--\r\n".to_string()
                            } else {
                                format!(
                                    "{}--boundary\r\nContent-Type: text/plain\r\nContent-Range: bytes {}-{}/{}\r\n\r\n",
                                    if separator == 0 { "" } else { "\r\n" },
                                    offset,
                                    offset.wrapping_add(length).wrapping_sub(1),
                                    filesize
                                )
                            };
                            let mut written = usize::MAX;
                            assert_eq!(
                                unsafe {
                                    swr_v1_http_range_format(
                                        &raw const input,
                                        std::ptr::null_mut(),
                                        0,
                                        &raw mut written,
                                    )
                                },
                                0
                            );
                            assert_eq!(written, expected.len());
                            for capacity in 1..=expected.len() + 2 {
                                let mut output = vec![0xa5; capacity + 2];
                                assert_eq!(
                                    unsafe {
                                        swr_v1_http_range_format(
                                            &raw const input,
                                            output.as_mut_ptr(),
                                            capacity,
                                            &raw mut written,
                                        )
                                    },
                                    0
                                );
                                let take = expected.len().min(capacity - 1);
                                assert_eq!(written, take);
                                assert_eq!(&output[..take], &expected.as_bytes()[..take]);
                                assert_eq!(output[take], 0);
                                assert!(output[take + 1..].iter().all(|&byte| byte == 0xa5));
                            }
                        }
                    }
                }
            }
        }
    }
    #[test]
    fn invalid_metadata_preserves_storage_and_state() {
        let input = HttpRangeFormat {
            boundary: text(b"b"),
            mime: text(b"m"),
            offset: 0,
            length: 1,
            filesize: 1,
            part: 2,
            separator: 0,
        };
        let mut storage = [0xa5; 16];
        let mut written = 99;
        assert_eq!(
            unsafe {
                swr_v1_http_range_format(
                    &raw const input,
                    storage.as_mut_ptr(),
                    storage.len(),
                    &raw mut written,
                )
            },
            INVALID
        );
        assert_eq!(storage, [0xa5; 16]);
        assert_eq!(written, 99);
        let mut state = HttpRangeFinish {
            phase: 7,
            status: 299,
            ..Default::default()
        };
        let before = state;
        assert_eq!(state.step(), INVALID);
        assert_eq!(state, before);
    }
    #[test]
    fn finalization_does_not_publish_success_before_native_push() {
        let mut state = HttpRangeFinish {
            content_length: 123,
            last_length: 2,
            task_count: 2,
            filesize: 100,
            status: 299,
            if_range: 1,
            ..Default::default()
        };
        assert_eq!(state.step(), 2);
        assert_eq!((state.status, state.content_length), (299, 123));
        state.end_length = 9;
        assert_eq!(state.step(), 3);
        assert_eq!((state.status, state.content_length), (206, 132));
        state.modified = 1;
        assert_eq!(state.step(), 4);
        assert_eq!((state.status, state.content_length), (206, 132));
        assert_eq!(state.step(), 1);
        assert_eq!((state.status, state.content_length), (206, 100));
        assert_eq!(state.step(), 0);
        assert_eq!(state.status, 200);
    }
}
