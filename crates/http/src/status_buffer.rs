//! `get_status_message`: the status line for a code.
use std::borrow::Cow;

/// Upstream's `list_of_status_code`: every code with a message, then -1.
pub const STATUS_CODES: [i32; 128] = [
    100, 101, 102, 103, 200, 201, 202, 203, 204, 205, 206, 207, 208, 226, 300, 301, 302, 303, 304,
    305, 306, 307, 308, 400, 401, 402, 403, 404, 405, 406, 407, 408, 409, 410, 411, 412, 413, 414,
    415, 416, 417, 418, 421, 422, 423, 424, 425, 426, 428, 429, 431, 451, 500, 501, 502, 503, 504,
    505, 506, 507, 508, 510, 511, -1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];

/// The status line for `code` without its NUL: a known message, else
/// `"<code> Unknown Status"`.
#[must_use]
pub fn status_message(code: i32) -> Cow<'static, [u8]> {
    if let Some(known) = crate::maps::status(code) {
        return Cow::Borrowed(known.to_bytes());
    }
    let mut unknown = [0; 128];
    crate::dispatch::unknown_status(code, &mut unknown);
    Cow::Owned(crate::c_text(&unknown).to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn known_messages_are_borrowed_and_unknown_ones_formatted() {
        assert!(matches!(status_message(200), Cow::Borrowed(b"200 OK")));
        assert!(matches!(status_message(0), Cow::Borrowed(b"200 OK")));
        assert_eq!(&*status_message(-701), b"-701 Unknown Status");
        assert_eq!(&*status_message(i32::MIN), b"-2147483648 Unknown Status");
        for code in STATUS_CODES.into_iter().take_while(|&code| code != -1) {
            assert!(status_message(code).starts_with(format!("{code} ").as_bytes()));
        }
    }
}
