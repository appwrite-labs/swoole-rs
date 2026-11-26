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
    pub(crate) separator: bool,
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

pub(crate) fn closing_boundary(boundary: &[u8]) -> [&[u8]; 3] {
    [b"\r\n--", boundary, b"--\r\n"]
}

fn render(
    input: &HttpRangeFormat<'_>,
    boundary: &[u8],
    mime: &[u8],
    out: &mut Prefix<'_>,
) -> fmt::Result {
    if input.part == 1 {
        return closing_boundary(boundary)
            .into_iter()
            .try_for_each(|part| out.append(part));
    }
    if input.separator {
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
            + usize::from(input.separator) * 2
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
) -> Result<bool, i32> {
    if input.part > 1 {
        return Err(INVALID);
    }
    if input.part == 0 && output.len() > i32::MAX as usize {
        return Err(OVERFLOW);
    }
    let (boundary, mime) = if input.part == 0 {
        (c_text(input.boundary), c_text(input.mime))
    } else {
        (input.boundary, &[][..])
    };
    let Some(required) =
        rendered_length(input, boundary, mime).filter(|&length| isize::try_from(length).is_ok())
    else {
        return Err(OVERFLOW);
    };
    let libc_overflow = input.part == 0 && required > i32::MAX as usize;
    let count = if let Some((_, room)) = output.split_last_mut() {
        let mut sink = Prefix {
            output: room,
            length: 0,
        };
        if render(input, boundary, mime, &mut sink).is_err() {
            return Err(OVERFLOW);
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
    Ok(libc_overflow)
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
            separator: true,
        };
        let mut output = [0xa5; 128];
        let mut n = 0;
        assert_eq!(range_format(&input, &mut output, &mut n), Ok(false));
        assert_eq!(
            &output[..n],
            b"\r\n--ab\r\nContent-Type: type\r\nContent-Range: bytes 3-7/10\r\n\r\n"
        );
        input.part = 1;
        assert_eq!(range_format(&input, &mut output, &mut n), Ok(false));
        assert_eq!(&output[..n], b"\r\n--ab\0cd--\r\n");
    }
    #[test]
    fn positive_wire_size_and_all_initialized_prefix_capacities() {
        for offset in [0usize, 9, 10, 99, 100, usize::MAX] {
            for length in [0usize, 1, 17] {
                for filesize in [0usize, 10, usize::MAX] {
                    for part in [0, 1] {
                        for separator in [false, true] {
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
                                    if separator { "\r\n" } else { "" },
                                    offset,
                                    offset.wrapping_add(length).wrapping_sub(1),
                                    filesize
                                )
                            };
                            let mut written = usize::MAX;
                            assert_eq!(range_format(&input, &mut [], &mut written), Ok(false));
                            assert_eq!(written, expected.len());
                            for capacity in 1..=expected.len() + 2 {
                                let mut output = vec![0xa5; capacity + 2];
                                assert_eq!(
                                    range_format(&input, &mut output[..capacity], &mut written),
                                    Ok(false)
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
            separator: false,
        };
        let mut storage = [0xa5; 16];
        let mut written = 99;
        assert_eq!(
            range_format(&input, &mut storage, &mut written),
            Err(INVALID)
        );
        assert_eq!(storage, [0xa5; 16]);
        assert_eq!(written, 99);
    }
}
