//! Inline static handler policy; native owns strings, RNG and MIME references.
use crate::c_text;
pub(crate) const BOUNDARY: &[u8] = b"SwooleBoundary\0";
const CONTENT_TYPE: &[u8] = b"multipart/byteranges; boundary=\0";
const ALLOCATION: i32 = -3;

/// The lazy boundary's random length while the boundary is still empty
/// (`length == 0`), else `None`.
#[must_use]
pub(crate) fn static_boundary(length: usize) -> Option<usize> {
    (length == 0).then_some(39 - BOUNDARY.len())
}
/// Whether the response is multipart: more than one task.
#[must_use]
pub(crate) fn static_multipart(tasks: usize) -> bool {
    tasks > 1
}
/// The binary-safe multipart content type over `boundary`, excluding any NUL.
pub(crate) fn static_content_type(boundary: &[u8]) -> Result<Vec<u8>, i32> {
    let prefix = c_text(CONTENT_TYPE);
    let mut value = Vec::new();
    value
        .try_reserve_exact(prefix.len() + boundary.len())
        .map_err(|_| ALLOCATION)?;
    value.extend_from_slice(prefix);
    value.extend_from_slice(boundary);
    Ok(value)
}
/// Publish 404 only for the terminal location match; no native callback occurs.
pub(crate) fn static_catch(last: bool, status: &mut i32) -> bool {
    if !last {
        return false;
    }
    *status = 404;
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn binary_content_type_keeps_the_whole_boundary() {
        assert_eq!(
            static_content_type(b"a\0b").unwrap(),
            b"multipart/byteranges; boundary=a\0b"
        );
        assert_eq!(static_boundary(0), Some(24));
        assert_eq!(static_boundary(1), None);
        assert!(!static_multipart(1));
        assert!(static_multipart(2));
        let mut status = 200;
        assert!(!static_catch(false, &mut status));
        assert_eq!(status, 200);
        assert!(static_catch(true, &mut status));
        assert_eq!(status, 404);
    }
}
