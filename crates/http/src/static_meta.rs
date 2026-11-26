//! Inline static handler policy; native owns strings, RNG and MIME references.
pub(crate) const BOUNDARY: &[u8] =
    swoole_rs_corebase::constants::SW_HTTP_SERVER_BOUNDARY_PREKEY.as_bytes();
const ALLOCATION: i32 = -3;

/// The lazy boundary's random length while the boundary is still empty
/// (`length == 0`), else `None`.
#[must_use]
pub(crate) fn static_boundary(length: usize) -> Option<usize> {
    (length == 0).then_some(crate::static_context::BOUNDARY_SIZE - BOUNDARY.len() - 1)
}
/// Whether the response is multipart: more than one task.
#[must_use]
pub(crate) fn static_multipart(tasks: usize) -> bool {
    tasks > 1
}
/// The binary-safe multipart content type over `boundary`, excluding any NUL.
pub(crate) fn static_content_type(boundary: &[u8]) -> Result<Vec<u8>, i32> {
    joined(&[b"multipart/byteranges; boundary=", boundary])
}

/// Allocate the exact combined length before copying any of the parts.
pub(crate) fn joined(parts: &[&[u8]]) -> Result<Vec<u8>, i32> {
    let length = parts
        .iter()
        .try_fold(0usize, |n, part| n.checked_add(part.len()))
        .filter(|&n| isize::try_from(n).is_ok())
        .ok_or(crate::OVERFLOW)?;
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(length).map_err(|_| ALLOCATION)?;
    for part in parts {
        bytes.extend_from_slice(part);
    }
    Ok(bytes)
}
/// Publish 404 only for the terminal location match; no native callback occurs.
pub(crate) fn static_catch(last: bool, status: &mut i32) -> bool {
    if !last {
        return false;
    }
    *status = crate::status::NOT_FOUND;
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
