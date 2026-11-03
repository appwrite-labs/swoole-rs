//! What the HTTP/2 server (crates/phphttp2) reads and does to its streams'
//! contexts.
use crate::{context::swr_v1_phphttp_context_free, header, host::Context, parse, send};
use std::ffi::CStr;
use swoole_rs_zend::sys::zend_string;

/// The response's `Date` value, cached per second.
#[must_use]
pub fn date() -> *mut zend_string {
    header::date()
}

/// `Context::set_compression_method` from a request's `accept-encoding`.
/// # Safety
/// `ctx` is a live context.
pub unsafe fn compression_method(ctx: *mut Context, accept_encoding: &[u8]) {
    parse::compression_method(
        send::state(ctx),
        accept_encoding.as_ptr(),
        accept_encoding.len(),
    );
}

/// `Context::compress`: the encoding's name and the compressed body, or
/// `None` to send `body` as it is. The bytes live until the next compression.
/// # Safety
/// `ctx` is a live context.
pub unsafe fn compress<'a>(ctx: *mut Context, body: &[u8]) -> Option<(&'static CStr, &'a [u8])> {
    header::compress_body(send::state(ctx), body)
}

/// The body the last [`compress`] made.
/// # Safety
/// `ctx` is a live context.
#[must_use]
pub unsafe fn compressed<'a>(ctx: *mut Context) -> &'a [u8] {
    header::compressed(send::state(ctx))
}

/// The connection's session id.
/// # Safety
/// `ctx` is a live context.
#[must_use]
pub unsafe fn fd(ctx: *mut Context) -> i64 {
    send::state(ctx).fd
}

/// The context's HTTP/2 stream, 0 once it is released.
/// # Safety
/// `ctx` is a live context.
#[must_use]
pub unsafe fn stream_id(ctx: *mut Context) -> u32 {
    send::state(ctx).stream_id
}

/// `~Stream`: the stream lets go of its context, which is complete.
/// # Safety
/// `ctx` is a live stream context, released once.
pub unsafe fn release_stream(ctx: *mut Context) {
    let state = send::state(ctx);
    state.stream_id = 0;
    state.end = 1;
    // SAFETY: per the caller.
    unsafe { swr_v1_phphttp_context_free(ctx) };
}
