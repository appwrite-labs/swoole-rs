//! Range response assembly/commit policy. Native owns containers and date/IO work.
use crate::{INVALID, OVERFLOW, c_text};
use std::fmt::{self, Write};

pub(crate) struct HttpRangeFormat<'a> {
    pub(crate) boundary: &'a [u8],
    pub(crate) mime: &'a [u8],
    pub(crate) offset: usize,
    pub(crate) length: usize,
    pub(crate) filesize: usize,
    pub(crate) part: u32,
    pub(crate) separator: u32,
}

/// The prefix of a rendering that fits `output`, and the whole rendering's length.
struct Prefix<'a> {
    output: &'a mut [u8],
    length: usize,
}
impl Prefix<'_> {
    fn append(&mut self, input: &[u8]) -> fmt::Result {
        for (destination, byte) in self.output.iter_mut().skip(self.length).zip(input) {
            *destination = *byte;
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

fn render(
    input: &HttpRangeFormat<'_>,
    boundary: &[u8],
    mime: &[u8],
    out: &mut Prefix<'_>,
) -> fmt::Result {
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
fn rendered_length(input: &HttpRangeFormat<'_>, boundary: &[u8], mime: &[u8]) -> Option<usize> {
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
/// An empty `output` queries the size; otherwise write the prefix that fits and a
/// final NUL. Return1 requests native EOVERFLOW for a header count beyond libc int;
/// preserve the snprintf prefix, but publish the pinned empty C-string/count0.
pub(crate) fn range_format(
    input: &HttpRangeFormat<'_>,
    output: &mut [u8],
    written: &mut usize,
) -> i32 {
    if input.part > 1 {
        return INVALID;
    }
    if input.part == 0 && output.len() > i32::MAX as usize {
        return OVERFLOW;
    }
    let (boundary, mime) = if input.part == 0 {
        (c_text(input.boundary), c_text(input.mime))
    } else {
        (input.boundary, &[][..])
    };
    let Some(required) =
        rendered_length(input, boundary, mime).filter(|&length| isize::try_from(length).is_ok())
    else {
        return OVERFLOW;
    };
    let libc_overflow = input.part == 0 && required > i32::MAX as usize;
    let count = if let Some((_, room)) = output.split_last_mut() {
        let mut sink = Prefix {
            output: room,
            length: 0,
        };
        if render(input, boundary, mime, &mut sink).is_err() {
            return OVERFLOW;
        }
        let count = sink.length.min(output.len() - 1);
        if let Some(nul) = output.get_mut(count) {
            *nul = 0;
        }
        if libc_overflow {
            if let Some(first) = output.first_mut() {
                *first = 0;
            }
            0
        } else {
            count
        }
    } else if libc_overflow {
        usize::MAX
    } else {
        required
    };
    *written = count;
    i32::from(libc_overflow)
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct HttpRangeFinish {
    pub(crate) content_length: usize,
    pub(crate) last_length: usize,
    pub(crate) task_count: usize,
    pub(crate) filesize: usize,
    pub(crate) end_length: usize,
    pub(crate) phase: u32,
    pub(crate) status: i32,
    pub(crate) if_range: u32,
    pub(crate) modified: u32,
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
    #[test]
    fn binary_end_and_c_string_header_are_different() {
        let mut input = HttpRangeFormat {
            boundary: b"ab\0cd",
            mime: b"type\0ignored",
            offset: 3,
            length: 5,
            filesize: 10,
            part: 0,
            separator: 1,
        };
        let mut output = [0xa5; 128];
        let mut n = 0;
        assert_eq!(range_format(&input, &mut output, &mut n), 0);
        assert_eq!(
            &output[..n],
            b"\r\n--ab\r\nContent-Type: type\r\nContent-Range: bytes 3-7/10\r\n\r\n"
        );
        input.part = 1;
        assert_eq!(range_format(&input, &mut output, &mut n), 0);
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
                                boundary: b"boundary",
                                mime: b"text/plain",
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
                            assert_eq!(range_format(&input, &mut [], &mut written), 0);
                            assert_eq!(written, expected.len());
                            for capacity in 1..=expected.len() + 2 {
                                let mut output = vec![0xa5; capacity + 2];
                                assert_eq!(
                                    range_format(&input, &mut output[..capacity], &mut written),
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
            boundary: b"b",
            mime: b"m",
            offset: 0,
            length: 1,
            filesize: 1,
            part: 2,
            separator: 0,
        };
        let mut storage = [0xa5; 16];
        let mut written = 99;
        assert_eq!(range_format(&input, &mut storage, &mut written), INVALID);
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
