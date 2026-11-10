//! Remaining HTTP-local dispatch/index decisions. Execution stays native.
use std::io::Write;

/// 1=WebSocket,2=HTTP2,0=protocol error. WebSocket takes precedence.
#[must_use]
pub fn transport_route(websocket_status: u32, http2: u32) -> i32 {
    if websocket_status >= 2 {
        1
    } else if http2 != 0 {
        2
    } else {
        0
    }
}
/// The unknown-status TLS fallback, NUL-terminated, as native snprintf wrote it.
pub(crate) fn unknown_status(code: i32, output: &mut [u8; 128]) {
    let mut sink = &mut output[..];
    // At most 27 bytes: any `i32`, the suffix and the NUL always fit.
    let Ok(()) = write!(sink, "{code} Unknown Status\0") else {
        return;
    };
}
