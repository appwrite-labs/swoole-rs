//! What the HTTP/2 server (crates/phphttp2) reads and does to its streams'
//! contexts.
use crate::{
    context::{self, free},
    header,
    host::Context,
    parse::{self, CAPABILITIES},
    send,
};
use std::{
    ffi::{CStr, c_int, c_void},
    sync::{OnceLock, atomic::Ordering::Relaxed},
};
use swoole_rs_phpserver::RecvData;
use swoole_rs_server::view;
use swoole_rs_zend::sys::{swr_zend_http_estrndup, swr_zend_treat_string, zend_string, zval};

/// A live context the HTTP/2 server holds: a connection's settings context
/// or a stream's. Only [`Ctx::server_context`], [`Ctx::stream_context`] and
/// the server's C entry points (whose callers promise a live context) make
/// one; what the server reads and does over it is safe, until a `release_*`
/// ends it.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct Ctx(*mut Context);

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

/// The HTTP/2 server's entry points that this crate's HTTP/1 code calls.
/// crates/phphttp2 depends on this crate, so it registers them at MINIT. A
/// bailout in one waits for the caller.
pub struct Server {
    /// `swoole_http2_server_onReceive`: an async server's data on an HTTP/2
    /// connection.
    /// # Safety
    /// The live server, the session's connection (or null) and its `RecvData`.
    pub on_receive: for<'a> unsafe fn(view::Server<'a>, *mut c_void, &RecvData) -> c_int,
    /// The static file handler for the live server: true once it answered.
    pub static_file: fn(*mut c_void, Ctx) -> bool,
    /// `Response::end`: the stream's last frames.
    pub respond: fn(Ctx, &[u8]) -> bool,
    /// `Response::write`: a DATA frame.
    pub write: fn(Ctx, &[u8]) -> bool,
    /// `Response::ping`.
    pub ping: fn(Ctx) -> bool,
    /// `Response::sendfile` of a path, from `offset` for `length` bytes.
    pub send_file: fn(Ctx, &[u8], i64, usize) -> bool,
    /// `Response::goaway` with its code and debug data.
    pub goaway: fn(Ctx, i64, &[u8]) -> bool,
    /// `RST_STREAM` with the stream id and error code.
    pub reset: fn(Ctx, u32, u32),
    /// A coroutine connection's session (fd, default context, max body size,
    /// coroutine, handler and its data).
    /// # Safety
    /// The context outlives the session; the handler may be called with a
    /// `SessionRequest` and the data.
    pub session_new: unsafe fn(i64, Ctx, u32, bool, unsafe extern "C" fn(*mut c_void), *mut c_void),
    /// One whole frame for the session of an fd; `SW_ERR` on failure.
    /// # Safety
    /// The frame's bytes, as long as its header says.
    pub session_receive: unsafe fn(i64, *const u8) -> c_int,
    /// The session of an fd goes.
    pub session_free: fn(i64),
}

static SERVER: OnceLock<&'static Server> = OnceLock::new();

/// crates/phphttp2's MINIT: its entry points, kept for the process.
pub fn register(server: &'static Server) {
    SERVER.get_or_init(|| server);
}

/// The registered HTTP/2 server; None before its MINIT.
pub(crate) fn server() -> Option<&'static Server> {
    SERVER.get().copied()
}

/// The response's `Date` value, cached per second.
#[must_use]
pub fn date() -> *mut zend_string {
    header::date()
}

/// `SW_HTTP2_DATA_BUFFER_SIZE`.
const DATA_BUFFER_SIZE: usize = 8192;

/// Built with `SW_HAVE_COMPRESSION`.
fn compression() -> bool {
    CAPABILITIES.load(Relaxed) & 8 != 0
}

impl Ctx {
    /// The handle of one of this crate's live HTTP/2 contexts.
    pub(crate) fn of(ctx: *mut Context) -> Self {
        Self(ctx)
    }

    /// `Context::set_header(key, value, false)` of a string value.
    pub fn set_header(self, key: &[u8], value: &[u8]) {
        crate::response::set_header_bytes(self.0, key, value);
    }

    /// `$request->server`, `header`, `get`, `post` or `cookie` (made on first
    /// use), or the response object.
    #[must_use]
    pub fn value(self, which: Value) -> *mut zval {
        let state = send::state(self.0);
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
    /// The bytes are read before the next append.
    #[must_use]
    pub unsafe fn data<'a>(self) -> &'a [u8] {
        send::state(self.0)
            .request
            .h2_data
            .as_deref()
            .map_or(&[], Vec::as_slice)
    }

    /// Start the request body (`SW_HTTP2_DATA_BUFFER_SIZE` reserved), unless
    /// it has started.
    pub fn data_start(self) {
        send::state(self.0)
            .request
            .h2_data
            .get_or_insert_with(|| Box::new(Vec::with_capacity(DATA_BUFFER_SIZE)));
    }

    /// Append a DATA frame's payload to the request body.
    pub fn data_append(self, payload: &[u8]) {
        self.data_start();
        if let Some(data) = send::state(self.0).request.h2_data.as_mut() {
            data.extend_from_slice(payload);
        }
    }

    /// `parse_multipart_data` over the whole body.
    #[must_use]
    pub fn multipart(self) -> bool {
        let state = send::state(self.0);
        let data = state.request.h2_data.take().unwrap_or_default();
        let parsed = parse::multipart_parse(state, &data);
        send::state(self.0).request.h2_data = Some(data);
        parsed
    }

    /// The context's `field` (0 for a compression field without compression).
    #[must_use]
    pub fn get(self, field: Field) -> u32 {
        let state = send::state(self.0);
        match field {
            Field::End => state.end.into(),
            Field::SendHeader => state.send_header.into(),
            Field::SendChunked => state.send_chunked.into(),
            #[expect(clippy::cast_sign_loss, reason = "upstream's uint32_t return")]
            Field::Status => state.response.status as u32,
            Field::ParseBody => state.parse_body.into(),
            Field::Form => state.request.post_form_urlencoded.into(),
            Field::Multipart => u32::from(state.mt_parser.is_some()),
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
    #[expect(
        clippy::cast_possible_truncation,
        reason = "upstream's narrower fields"
    )]
    pub fn set(self, field: Field, value: u32) {
        let state = send::state(self.0);
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
    #[must_use]
    pub fn send(self, data: &[u8]) -> bool {
        context::send(self.0, data)
    }

    /// `ctx->close`.
    pub fn close(self) {
        context::close(self.0);
    }

    /// `compression_types` lists the response's content type.
    #[must_use]
    pub fn compressible(self, kind: &[u8]) -> bool {
        compression() && context::compression_type(send::state(self.0), Some(kind))
    }

    /// `Context::set_compression_method` from a request's `accept-encoding`.
    pub fn compression_method(self, accept_encoding: &[u8]) {
        parse::compression_method(send::state(self.0), accept_encoding);
    }

    /// `Context::compress`: the encoding's name and the compressed body, or
    /// `None` to send `body` as it is. The bytes live until the next compression.
    /// # Safety
    /// The bytes are read before the next compression.
    #[must_use]
    pub unsafe fn compress<'a>(self, body: &[u8]) -> Option<(&'static CStr, &'a [u8])> {
        header::compress_body(send::state(self.0), body)
    }

    /// The body the last [`Ctx::compress`] made.
    /// # Safety
    /// The bytes are read before the next compression.
    #[must_use]
    pub unsafe fn compressed<'a>(self) -> &'a [u8] {
        header::compressed(send::state(self.0))
    }

    /// The connection's session id.
    #[must_use]
    pub fn fd(self) -> i64 {
        send::state(self.0).fd
    }

    /// `~Stream`: the stream lets go of its context, which is complete.
    /// # Safety
    /// A stream's context, released once; no copy of the handle is used after.
    pub unsafe fn release_stream(self) {
        let state = send::state(self.0);
        state.stream_id = 0;
        state.end = 1;
        // SAFETY: per the caller.
        unsafe { free(self.0) };
    }

    /// The static handler answers the request: no server hook counts it.
    pub fn unhook(self) {
        let state = send::state(self.0);
        state.before_request = 0;
        state.after_response = 0;
    }

    /// `delete default_ctx`: a server connection's settings context, which no
    /// object or stream holds, goes with its session.
    /// # Safety
    /// Nothing else references the context, and no copy of the handle is used after.
    pub unsafe fn release_default(self) {
        // SAFETY: per the caller.
        unsafe { free(self.0) };
    }

    /// `swoole_http_parse_cookie`: a `cookie` header's pairs into the request's
    /// `$cookie`.
    pub fn cookie(self, value: &[u8]) {
        parse::parse_cookie(self.value(Value::Cookie), value);
    }

    /// A server connection's settings context (`default_ctx`): the server's
    /// settings, for session `fd`, kept alive and counted by the request hooks.
    #[must_use]
    pub fn server_context(server: view::Server<'_>, fd: i64) -> Self {
        let ctx = context::alloc();
        // SAFETY: the new context.
        unsafe { context::init(ctx, server) };
        let state = send::state(ctx);
        state.fd = fd;
        state.http2 = 1;
        state.keepalive = 1;
        state.before_request = 1;
        Self(ctx)
    }

    /// A stream's context: a copy of this settings context's, with its request
    /// and response objects and the request's `streamId`.
    #[must_use]
    pub fn stream_context(self, fd: i64, stream_id: u32) -> Self {
        let ctx = context::context_new(fd);
        // SAFETY: two live contexts.
        unsafe { context::copy_settings(ctx, self.0) };
        let state = send::state(ctx);
        state.http2 = 1;
        state.stream_id = stream_id;
        state.keepalive = 1;
        if let Some(request) = state.request.zobject.props() {
            request.long(c"streamId", i64::from(stream_id));
        }
        Self(ctx)
    }

    /// `ctx->request.path`: the `:path` up to its query.
    pub fn set_path(self, path: &[u8]) {
        let state = send::state(self.0);
        // SAFETY: the path's bytes, copied into request memory the context frees.
        state.request.path = unsafe { swr_zend_http_estrndup(path.as_ptr().cast(), path.len()) };
        state.request.path_len = u32::try_from(path.len()).unwrap_or(u32::MAX);
    }

    /// A `multipart/form-data` content type's boundary starts the request's
    /// multipart parser; false when it has none.
    #[must_use]
    pub fn multipart_start(self, content_type: &[u8]) -> bool {
        parse::multipart_start(send::state(self.0), content_type)
    }

    /// `sapi_module.treat_data(PARSE_STRING, ...)`: a query string's pairs into
    /// the request's `which` array.
    pub fn treat_query(self, which: Value, query: &[u8]) {
        let array = self.value(which);
        // SAFETY: the query's bytes, copied into request memory the SAPI frees.
        let text = unsafe { swr_zend_http_estrndup(query.as_ptr().cast(), query.len()) };
        // SAFETY: a fresh copy and the request's live array.
        unsafe { swr_zend_treat_string(text, array) };
    }
}
