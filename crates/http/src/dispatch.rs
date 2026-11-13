//! Remaining HTTP-local dispatch/index decisions. Execution stays native.
use std::io::Write;

/// The unknown-status TLS fallback, NUL-terminated, as native snprintf wrote it.
pub(crate) fn unknown_status(code: i32, output: &mut [u8; 128]) {
    let mut sink = &mut output[..];
    // At most 27 bytes: any `i32`, the suffix and the NUL always fit.
    let Ok(()) = write!(sink, "{code} Unknown Status\0") else {
        return;
    };
}
