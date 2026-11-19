//! `get_status_message`: the status line for a code.
use std::borrow::Cow;

/// Upstream's `list_of_status_code`: every code with a message, then -1.
pub const STATUS_CODES: [i32; 128] = {
    let mut codes = [0; 128];
    let mut remaining = codes.as_mut_slice();
    let mut code = 100;
    while code < 600 {
        if crate::maps::status(code).is_some() {
            let (next, tail) = remaining.split_first_mut().expect("status list fits");
            *next = code;
            remaining = tail;
        }
        code += 1;
    }
    *remaining.first_mut().expect("space for the sentinel") = -1;
    codes
};

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
