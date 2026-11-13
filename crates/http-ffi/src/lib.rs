//! `src/protocol/http.cc`'s helpers under their C++ symbols ([`http_server`]), the
//! request reader's state ([`HttpRequest`]) and the HTTP proxy records, over crates/http's
//! safe API.
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]
use std::slice;
use swoole_rs_http::STATUS_CODES;

pub mod http_server;
mod owned_proxy;
pub use owned_proxy::*;
mod request;
pub use request::{HttpForm, HttpNative, HttpRequest, HttpString, request_free, request_new};
#[cfg(test)]
mod pin_helpers;

const INVALID: i32 = -1;
const OVERFLOW: i32 = -2;

/// `length` initialized bytes at `data`, which may be null only when empty.
///
/// # Safety
/// A nonempty span is readable and initialized for its length; no borrow escapes.
unsafe fn bytes<'a>(data: *const u8, length: usize) -> Result<&'a [u8], i32> {
    if length > isize::MAX as usize {
        return Err(OVERFLOW);
    }
    if length == 0 {
        return Ok(&[]);
    }
    if data.is_null() {
        return Err(INVALID);
    }
    // SAFETY: non-null, `length <= isize::MAX`; callers lend that many initialized bytes.
    Ok(unsafe { slice::from_raw_parts(data, length) })
}

/// Upstream's `list_of_status_code`, which native code may write.
#[used]
#[unsafe(export_name = "_ZN6swoole11http_server19list_of_status_codeE")]
pub static mut NATIVE_STATUS_CODES: [i32; 128] = STATUS_CODES;

#[cfg(test)]
mod tests;
