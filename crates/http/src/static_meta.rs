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
    let mut bytes = Vec::new();
    append_parts(&mut bytes, parts, isize::MAX as usize)?;
    Ok(bytes)
}

/// Append one complete fragment, leaving existing bytes intact if it cannot fit.
#[cfg_attr(
    protocol_buffer_lint,
    expect(
        clippy::disallowed_methods,
        reason = "reserve the checked fragment length before copying exactly those bytes"
    )
)]
pub(crate) fn append_parts(
    bytes: &mut Vec<u8>,
    parts: &[&[u8]],
    max_fragment: usize,
) -> Result<(), i32> {
    let length = parts
        .iter()
        .try_fold(0usize, |n, part| n.checked_add(part.len()))
        .filter(|&n| n <= max_fragment)
        .ok_or(crate::OVERFLOW)?;
    if bytes.is_empty() {
        bytes.try_reserve_exact(length)
    } else {
        bytes.try_reserve(length)
    }
    .map_err(|_| ALLOCATION)?;
    for part in parts {
        bytes.extend_from_slice(part);
    }
    Ok(())
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
    fn fragment_limit_is_checked_before_any_part_is_appended() {
        let mut out = b"prefix".to_vec();
        assert_eq!(
            append_parts(&mut out, &[b"ab", b"cd"], 3),
            Err(crate::OVERFLOW)
        );
        assert_eq!(out, b"prefix");
        assert_eq!(append_parts(&mut out, &[b"ab", b"cd"], 4), Ok(()));
        assert_eq!(out, b"prefixabcd");
    }

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
