//! What the HTTP/2 server (crates/phphttp2) reads and does to its streams'
//! contexts.
use crate::{
    context::{self, swr_v1_phphttp_context_free},
    header,
    host::Context,
    parse::{self, CAPABILITIES},
    send,
};
use std::{ffi::CStr, sync::atomic::Ordering::Relaxed};
use swoole_rs_zend::sys::zend_string;

/// What the HTTP/2 server reads and writes of a context.
#[derive(Clone, Copy)]
pub enum Field {
    End,
    SendHeader,
    SendChunked,
    Status,
    ParseBody,
    Form,
    Multipart,
    CoSocket,
    AcceptCompression,
    EnableCompression,
    ContentCompressed,
    CompressionTypes,
    StreamId,
}

/// Built with `SW_HAVE_COMPRESSION`.
fn compression() -> bool {
    CAPABILITIES.load(Relaxed) & 8 != 0
}

/// The context's `field` (0 for a compression field without compression).
/// # Safety
/// `ctx` is a live context.
#[must_use]
pub unsafe fn get(ctx: *mut Context, field: Field) -> u32 {
    let state = send::state(ctx);
    match field {
        Field::End => state.end.into(),
        Field::SendHeader => state.send_header.into(),
        Field::SendChunked => state.send_chunked.into(),
        #[expect(clippy::cast_sign_loss, reason = "upstream's uint32_t return")]
        Field::Status => state.response.status as u32,
        Field::ParseBody => state.parse_body.into(),
        Field::Form => state.request.post_form_urlencoded.into(),
        Field::Multipart => u32::from(!state.mt_parser.is_null()),
        Field::CoSocket => u32::from(context::co_socket(state)),
        Field::StreamId => state.stream_id,
        _ if !compression() => 0,
        Field::AcceptCompression => state.accept_compression.into(),
        Field::EnableCompression => state.enable_compression.into(),
        Field::ContentCompressed => state.content_compressed.into(),
        Field::CompressionTypes => u32::from(context::compression_type(state, None)),
    }
}

/// Set the context's `field` (`End`, `SendHeader`, `SendChunked`, `Status`,
/// `Form` or, with compression, `AcceptCompression`).
/// # Safety
/// `ctx` is a live context.
#[expect(
    clippy::cast_possible_truncation,
    reason = "upstream's narrower fields"
)]
pub unsafe fn set(ctx: *mut Context, field: Field, value: u32) {
    let state = send::state(ctx);
    match field {
        Field::End => state.end = value as u8,
        Field::SendHeader => state.send_header = value as u8,
        Field::SendChunked => state.send_chunked = value as u8,
        #[expect(clippy::cast_possible_wrap, reason = "upstream's int status")]
        Field::Status => state.response.status = value as i32,
        Field::Form => state.request.post_form_urlencoded = value as u8,
        Field::AcceptCompression if compression() => state.accept_compression = value as u8,
        _ => {}
    }
}

/// `ctx->send`.
/// # Safety
/// `ctx` is a live, bound context.
pub unsafe fn send(ctx: *mut Context, data: &[u8]) -> bool {
    context::send(ctx, data)
}

/// `ctx->close`.
/// # Safety
/// `ctx` is a live, bound context.
pub unsafe fn close(ctx: *mut Context) -> bool {
    context::close(ctx)
}

/// `compression_types` lists the response's content type.
/// # Safety
/// `ctx` is a live context.
#[must_use]
pub unsafe fn compressible(ctx: *mut Context, kind: &[u8]) -> bool {
    compression() && context::compression_type(send::state(ctx), Some(kind))
}

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

/// `delete default_ctx`: a server connection's settings context, which no
/// object or stream holds, goes with its session.
/// # Safety
/// `ctx` is a live context nothing else references, released once.
pub unsafe fn release_default(ctx: *mut Context) {
    // SAFETY: per the caller.
    unsafe { swr_v1_phphttp_context_free(ctx) };
}
