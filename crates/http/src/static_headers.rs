//! Encode a static HTTP/1 response head using the selected representation's metadata.
use crate::static_meta::append_parts;

pub struct HttpStaticHeaders<'a> {
    pub status: i32,
    pub date: &'a [u8],
    pub modified: &'a [u8],
    pub etag: &'a [u8],
    pub content_type: &'a [u8],
    pub content_length: usize,
    pub keep_alive: bool,
    pub extra: &'a [u8],
}

/// Encode the complete response, including its final blank line.
///
/// # Errors
/// Returns an allocation or size error if the header cannot fit.
pub fn static_headers(header: &HttpStaticHeaders<'_>) -> Result<Vec<u8>, i32> {
    let mut out = Vec::new();
    let mut append = |parts: &[&[u8]]| append_parts(&mut out, parts, i32::MAX as usize);
    append(&[
        b"HTTP/1.1 ",
        &crate::status_message(header.status),
        b"\r\nConnection: ",
        if header.keep_alive {
            b"keep-alive"
        } else {
            b"close"
        },
        b"\r\nDate: ",
        crate::c_text(header.date),
        b"\r\nServer: ",
        swoole_rs_corebase::constants::SW_HTTP_SERVER_SOFTWARE.as_bytes(),
        b"\r\n",
    ])?;
    if header.status != crate::status::NOT_MODIFIED {
        append(&[
            b"Content-Length: ",
            header.content_length.to_string().as_bytes(),
            b"\r\nContent-Type: ",
            crate::c_text(header.content_type),
            b"\r\n",
        ])?;
    }
    if !matches!(header.status, 400 | 404) {
        append(&[b"Last-Modified: ", crate::c_text(header.modified), b"\r\n"])?;
    }
    if !header.etag.is_empty() {
        append(&[b"ETag: ", header.etag, b"\r\n"])?;
    }
    append(&[header.extra, b"\r\n"])?;
    Ok(out)
}
