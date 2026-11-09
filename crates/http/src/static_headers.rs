//! Static response header encoding. Native retains allocation, dates and IO.
use crate::{INVALID, OVERFLOW, c_text};
use std::io::Write;

/// A static response's head; each text is read as `%s`, up to its first NUL.
pub struct HttpStaticHeaders<'a> {
    pub status: &'a [u8],
    pub date: &'a [u8],
    pub modified: &'a [u8],
    pub content_type: &'a [u8],
    pub software: &'a [u8],
    pub page: &'a [u8],
    pub content_length: i64,
    pub range_offset: i64,
    pub range_length: usize,
    pub filesize: usize,
    pub variant: u32,
    pub keep_alive: bool,
    pub range_kind: u32,
}
/// Encode 0=404, 1=304, 2=autoindex, 3=file. Range kind: 0=none, 1=Content-Range,
/// 2=Accept-Ranges. The whole head, which callers cut as their `snprintf` buffer does.
///
/// # Errors
/// `INVALID` for an unknown variant or range kind, `OVERFLOW` for a head beyond
/// libc's `int`.
pub fn static_headers(header: &HttpStaticHeaders<'_>) -> Result<Vec<u8>, i32> {
    if header.variant > 3 || header.range_kind > 2 {
        return Err(INVALID);
    }
    let texts = [
        header.status,
        header.date,
        header.modified,
        header.content_type,
        header.software,
        header.page,
    ]
    .map(c_text);
    // A conservative bound includes every fixed header and decimal field. Huge
    // unsupported spans fail before formatting rather than overflowing libc int.
    let Some(bound) = texts
        .into_iter()
        .try_fold(512usize, |sum, value| sum.checked_add(value.len()))
        .filter(|&n| i32::try_from(n).is_ok())
    else {
        return Err(OVERFLOW);
    };
    let [status, date, modified, content_type, software, page] = texts;
    let mut sink = Vec::with_capacity(bound);
    let result = (|| {
        sink.write_all(b"HTTP/1.1 ")?;
        sink.write_all(match header.variant {
            1 => b"304 Not Modified",
            2 => b"200 OK",
            _ => status,
        })?;
        sink.write_all(b"\r\n")?;
        if header.variant == 0 {
            sink.write_all(b"Server: ")?;
            sink.write_all(software)?;
            write!(
                sink,
                "\r\nContent-Length: {}\r\n\r\n",
                header.content_length
            )?;
            return sink.write_all(page);
        }
        sink.write_all(if header.keep_alive {
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
            sink.write_all(if header.variant == 2 {
                b"text/html"
            } else {
                content_type
            })?;
            sink.write_all(b"\r\n")?;
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
                    sink.write_all(b"Accept-Ranges: bytes\r\n")?;
                }
            }
        }
        sink.write_all(b"Date: ")?;
        sink.write_all(date)?;
        sink.write_all(b"\r\nLast-Modified: ")?;
        sink.write_all(modified)?;
        sink.write_all(b"\r\nServer: ")?;
        sink.write_all(software)?;
        sink.write_all(b"\r\n\r\n")
    })();
    result.map_err(|_| OVERFLOW)?;
    Ok(sink)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn header() -> HttpStaticHeaders<'static> {
        HttpStaticHeaders {
            status: b"206 Partial Content",
            date: b"today",
            modified: b"yesterday",
            content_type: b"text/plain",
            software: b"Swoole",
            page: b"",
            content_length: 0,
            range_offset: 0,
            range_length: 0,
            filesize: 0,
            variant: 3,
            keep_alive: true,
            range_kind: 1,
        }
    }
    #[test]
    fn legacy_empty_range_and_c_string_texts() {
        let head = static_headers(&header()).unwrap();
        let expected = format!(
            "HTTP/1.1 206 Partial Content\r\nConnection: keep-alive\r\nContent-Length: 0\r\n\
             Content-Type: text/plain\r\nContent-Range: bytes 0-{}/0\r\nDate: today\r\n\
             Last-Modified: yesterday\r\nServer: Swoole\r\n\r\n",
            usize::MAX
        );
        assert_eq!(String::from_utf8(head).unwrap(), expected);
        let cut = HttpStaticHeaders {
            status: b"404 Not Found\0ignored",
            page: b"page",
            content_length: 4,
            variant: 0,
            ..header()
        };
        assert_eq!(
            static_headers(&cut).unwrap(),
            b"HTTP/1.1 404 Not Found\r\nServer: Swoole\r\nContent-Length: 4\r\n\r\npage"
        );
    }
    #[test]
    fn invalid_metadata_is_refused() {
        for variant in [4, u32::MAX] {
            let h = HttpStaticHeaders {
                variant,
                ..header()
            };
            assert_eq!(static_headers(&h), Err(INVALID));
        }
        let h = HttpStaticHeaders {
            range_kind: 3,
            ..header()
        };
        assert_eq!(static_headers(&h), Err(INVALID));
    }
}
