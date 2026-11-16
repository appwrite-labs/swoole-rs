//! Multipart upload accounting and result policy.
use crate::multipart_parser::ParseError;
/// How a multipart parse fell short.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MultipartError {
    /// The parser failed.
    Parser,
    /// It processed less than the input; `excepted` keeps the request.
    Short { excepted: bool },
}

/// Classify the parser result: processing the whole input clears the buffer.
/// # Errors
/// What to log, and for a short parse whether the request is still taken.
pub fn multipart_result(
    processed: Result<usize, ParseError>,
    length: usize,
    excepted: bool,
) -> Result<(), MultipartError> {
    let processed = processed.map_err(|_| MultipartError::Parser)?;
    if processed == length {
        Ok(())
    } else {
        Err(MultipartError::Short { excepted })
    }
}
/// Whether a part's header `name` is `Content-Disposition`.
#[must_use]
pub fn multipart_disposition(name: &[u8]) -> bool {
    crate::case_equal(name, b"content-disposition")
}
/// Consume the destructive cookie iterator over `data` through the first filename:
/// whether there is one. The legacy callback stops even for a filename with no '='
/// or an empty value.
/// # Errors
/// The iterator's status without a NUL.
pub fn multipart_filename(data: &mut [u8]) -> Result<bool, i32> {
    let mut cursor = 0;
    loop {
        match crate::headers::cookie_next(data, &mut cursor) {
            Ok(Some(cookie)) => {
                let key = data.get(cookie.key).unwrap_or_default();
                if crate::case_equal(key, b"filename") {
                    return Ok(true);
                }
            }
            Ok(None) => return Ok(false),
            Err(status) => return Err(status),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::INVALID;
    #[test]
    fn filename_stops_with_identical_destructive_cookie_state() {
        for input in [
            &b"form-data; name=x; filename=one; filename=two\0"[..],
            b"FiLeNaMe;next=x\0",
            b"filename =x;other=y\0",
            b"=bad; filename=\0",
            b"filename\0;ignored=value",
            b"name=x\0",
            b"\0",
        ] {
            let mut expected = input.to_vec();
            let mut actual = input.to_vec();
            let mut cursor = 0;
            let result = loop {
                match crate::headers::cookie_next(&mut expected, &mut cursor) {
                    Ok(Some(out)) => {
                        if expected[out.key].eq_ignore_ascii_case(b"filename") {
                            break Ok(true);
                        }
                    }
                    Ok(None) => break Ok(false),
                    Err(status) => break Err(status),
                }
            };
            assert_eq!(multipart_filename(&mut actual), result);
            assert_eq!(actual, expected);
        }
        assert!(multipart_disposition(b"Content-Disposition"));
        assert!(!multipart_disposition(b"content-disposition "));
        assert_eq!(multipart_filename(&mut []), Err(INVALID));
        let mut missing_nul = *b"filename";
        assert_eq!(multipart_filename(&mut missing_nul), Err(INVALID));
    }
    #[test]
    fn parser_results() {
        assert_eq!(
            multipart_result(Err(ParseError::Callback), usize::MAX, true),
            Err(MultipartError::Parser)
        );
        assert_eq!(
            multipart_result(Ok(1), 2, false),
            Err(MultipartError::Short { excepted: false })
        );
        assert_eq!(
            multipart_result(Ok(1), 2, true),
            Err(MultipartError::Short { excepted: true })
        );
        assert_eq!(multipart_result(Ok(2), 2, false), Ok(()));
    }
}
