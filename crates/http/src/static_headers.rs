//! Static response header encoding. Native retains allocation, dates and IO.
use crate::{OVERFLOW, c_text};
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
    pub head: StaticHead,
    pub keep_alive: bool,
}

/// Which response a static head is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StaticHead {
    NotFound,
    NotModified,
    Autoindex,
    File(StaticRange),
}

/// The range header a file's head carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StaticRange {
    None,
    /// `Content-Range` for one range of the file.
    Content {
        offset: i64,
        length: usize,
        filesize: usize,
    },
    /// `Accept-Ranges`.
    Accept,
}

/// The whole head, which callers cut as their `snprintf` buffer does.
///
/// # Errors
/// `OVERFLOW` for a head beyond libc's `int`.
pub fn static_headers(header: &HttpStaticHeaders<'_>) -> Result<Vec<u8>, i32> {
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
        sink.write_all(match header.head {
            StaticHead::NotModified => b"304 Not Modified",
            StaticHead::Autoindex => b"200 OK",
            _ => status,
        })?;
        sink.write_all(b"\r\n")?;
        if header.head == StaticHead::NotFound {
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
        if header.head != StaticHead::NotModified {
            write!(
                sink,
                "Content-Length: {}\r\nContent-Type: ",
                header.content_length
            )?;
            sink.write_all(if header.head == StaticHead::Autoindex {
                b"text/html"
            } else {
                content_type
            })?;
            sink.write_all(b"\r\n")?;
            match header.head {
                StaticHead::File(StaticRange::Content {
                    offset,
                    length,
                    filesize,
                }) => {
                    #[expect(
                        clippy::cast_possible_truncation,
                        clippy::cast_sign_loss,
                        reason = "native size_t arithmetic on the off_t range offset"
                    )]
                    let end = length.wrapping_add(offset as usize).wrapping_sub(1);
                    write!(sink, "Content-Range: bytes {offset}-{end}/{filesize}\r\n")?;
                }
                StaticHead::File(StaticRange::Accept) => {
                    sink.write_all(b"Accept-Ranges: bytes\r\n")?;
                }
                _ => {}
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
            head: StaticHead::File(StaticRange::Content {
                offset: 0,
                length: 0,
                filesize: 0,
            }),
            keep_alive: true,
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
            head: StaticHead::NotFound,
            ..header()
        };
        assert_eq!(
            static_headers(&cut).unwrap(),
            b"HTTP/1.1 404 Not Found\r\nServer: Swoole\r\nContent-Length: 4\r\n\r\npage"
        );
    }
}
