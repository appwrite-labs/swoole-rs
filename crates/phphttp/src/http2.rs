//! What the HTTP/2 server (crates/phphttp2) reads and does to its streams'
//! contexts.
use crate::{
    context::{self, free},
    header,
    host::Context,
    parse::{self, CAPABILITIES},
    send,
};
use std::{ffi::CStr, sync::atomic::Ordering::Relaxed};
use swoole_rs_zend::sys::{swr_zend_http_estrndup, swr_zend_treat_string, zend_string, zval};

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

/// The request arrays and the response object the HTTP/2 server reads.
#[derive(Clone, Copy)]
pub enum Value {
    Server,
    Header,
    Response,
    Get,
    Post,
    Cookie,
}

/// `$request->server`, `header`, `get`, `post` or `cookie` (made on first
/// use), or the response object.
/// # Safety
/// `ctx` is a live context with its objects.
#[must_use]
pub unsafe fn value(ctx: *mut Context, which: Value) -> *mut zval {
    let state = send::state(ctx);
    let property = match which {
        Value::Response => return state.response.zobject.ptr,
        Value::Server => parse::Property::Server,
        Value::Header => parse::Property::Header,
        Value::Get => parse::Property::Get,
        Value::Post => parse::Property::Post,
        Value::Cookie => parse::Property::Cookie,
    };
    parse::property(state, property)
}

/// The request body's bytes so far (none without a body).
/// # Safety
/// `ctx` is a live context; the bytes live until the next append.
#[must_use]
pub unsafe fn data<'a>(ctx: *mut Context) -> &'a [u8] {
    send::state(ctx)
        .request
        .h2_data
        .as_deref()
        .map_or(&[], Vec::as_slice)
}

/// Start the request body (`SW_HTTP2_DATA_BUFFER_SIZE` reserved), unless
/// it has started.
/// # Safety
/// `ctx` is a live context.
pub unsafe fn data_start(ctx: *mut Context) {
    send::state(ctx)
        .request
        .h2_data
        .get_or_insert_with(|| Box::new(Vec::with_capacity(DATA_BUFFER_SIZE)));
}

/// Append a DATA frame's payload to the request body.
/// # Safety
/// `ctx` is a live context.
pub unsafe fn data_append(ctx: *mut Context, payload: &[u8]) {
    // SAFETY: per the caller.
    unsafe { data_start(ctx) };
    if let Some(data) = send::state(ctx).request.h2_data.as_mut() {
        data.extend_from_slice(payload);
    }
}

/// `parse_multipart_data` over the whole body.
/// # Safety
/// `ctx` is a live context with a multipart parser.
pub unsafe fn multipart(ctx: *mut Context) -> bool {
    let state = send::state(ctx);
    let data = state.request.h2_data.take().unwrap_or_default();
    let parsed = parse::multipart_parse(state, data.as_ptr(), data.len());
    send::state(ctx).request.h2_data = Some(data);
    parsed
}

/// `SW_HTTP2_DATA_BUFFER_SIZE`.
const DATA_BUFFER_SIZE: usize = 8192;

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
    unsafe { free(ctx) };
}

/// The static handler answers the request: no server hook counts it.
/// # Safety
/// `ctx` is a live context.
pub unsafe fn unhook(ctx: *mut Context) {
    let state = send::state(ctx);
    state.before_request = 0;
    state.after_response = 0;
}

/// `delete default_ctx`: a server connection's settings context, which no
/// object or stream holds, goes with its session.
/// # Safety
/// `ctx` is a live context nothing else references, released once.
pub unsafe fn release_default(ctx: *mut Context) {
    // SAFETY: per the caller.
    unsafe { free(ctx) };
}

/// `swoole_http_parse_cookie`: a `cookie` header's pairs into the request's
/// `$cookie`.
/// # Safety
/// `array` is the request's live cookie array.
pub unsafe fn cookie(array: *mut zval, value: &[u8]) {
    parse::parse_cookie(array, value.as_ptr(), value.len());
}

/// A server connection's settings context (`default_ctx`): the server's
/// settings, for session `fd`, kept alive and counted by the request hooks.
/// # Safety
/// `server` is the live server.
#[must_use]
pub unsafe fn server_context(server: *mut std::ffi::c_void, fd: i64) -> *mut Context {
    let ctx = context::alloc();
    // SAFETY: a new context and the live server.
    unsafe { context::init(ctx, server) };
    let state = send::state(ctx);
    state.fd = fd;
    state.http2 = 1;
    state.keepalive = 1;
    state.before_request = 1;
    ctx
}

/// A stream's context: a copy of `default_ctx`'s settings, with its request
/// and response objects and the request's `streamId`.
/// # Safety
/// `default_ctx` is a live context.
#[must_use]
pub unsafe fn stream_context(default_ctx: *mut Context, fd: i64, stream_id: u32) -> *mut Context {
    let ctx = context::context_new(fd);
    // SAFETY: two live contexts.
    unsafe { context::copy_settings(ctx, default_ctx) };
    let state = send::state(ctx);
    state.http2 = 1;
    state.stream_id = stream_id;
    state.keepalive = 1;
    if let Some(request) = state.request.zobject.props() {
        request.long(c"streamId", i64::from(stream_id));
    }
    ctx
}

/// `ctx->request.path`: the `:path` up to its query.
/// # Safety
/// `ctx` is a live context without a path.
pub unsafe fn set_path(ctx: *mut Context, path: &[u8]) {
    let state = send::state(ctx);
    // SAFETY: the path's bytes, copied into request memory the context frees.
    state.request.path = unsafe { swr_zend_http_estrndup(path.as_ptr().cast(), path.len()) };
    state.request.path_len = u32::try_from(path.len()).unwrap_or(u32::MAX);
}

/// A `multipart/form-data` content type's boundary starts the request's
/// multipart parser; false when it has none.
/// # Safety
/// `ctx` is a live context.
pub unsafe fn multipart_start(ctx: *mut Context, content_type: &[u8]) -> bool {
    let (mut boundary, mut length) = (std::ptr::null_mut(), 0);
    // SAFETY: a live context, the value's bytes and local outputs.
    let found = unsafe {
        parse::multipart_boundary_of(
            ctx.cast(),
            content_type.as_ptr().cast(),
            content_type.len(),
            b"multipart/form-data".len(),
            &raw mut boundary,
            &raw mut length,
        )
    };
    // SAFETY: the boundary inside the value.
    if !found || !unsafe { parse::init_multipart(ctx.cast(), boundary, length) } {
        return false;
    }
    send::state(ctx).parser.data = ctx.cast();
    true
}

/// `sapi_module.treat_data(PARSE_STRING, ...)`: a query string's pairs into `array`.
/// # Safety
/// `array` is a live array.
pub unsafe fn treat_query(array: *mut zval, query: &[u8]) {
    // SAFETY: the query's bytes, copied into request memory the SAPI frees.
    let text = unsafe { swr_zend_http_estrndup(query.as_ptr().cast(), query.len()) };
    // SAFETY: a fresh copy and a live array.
    unsafe { swr_zend_treat_string(text, array) };
}
