//! What the HTTP/2 server (crates/phphttp2) reads and does to its streams'
//! contexts.
use crate::{
    context::{self, free},
    header,
    parse::{self, CAPABILITIES},
};
use std::{
    ffi::{CStr, c_int},
    sync::{OnceLock, atomic::Ordering::Relaxed},
};
use swoole_rs_phpserver::RecvData;
use swoole_rs_server::{ConnectionRecord, view};
use swoole_rs_zend::{
    HeldArray, Owned,
    bailout::Bailout,
    sys::{swr_zend_treat_string, zval},
};

/// The HTTP/2 server's contexts: a connection's settings context
/// ([`Ctx::server_context`]) or a stream's ([`Ctx::stream_context`]), live
/// until a `release_*` ends them.
pub use crate::context::Ctx;

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

/// A response array property.
#[derive(Clone, Copy)]
pub enum ResponseArray {
    Header = 0,
    Cookie = 1,
    Trailer = 2,
}

/// The HTTP/2 server's entry points that this crate's HTTP/1 code calls.
/// crates/phphttp2 depends on this crate, so it registers them at MINIT. A
/// bailout in one waits for the caller.
pub struct Server {
    /// `swoole_http2_server_onReceive`: an async server's data on an HTTP/2
    /// connection, with the session's connection.
    pub on_receive: for<'a> fn(view::Server<'a>, &ConnectionRecord, &RecvData) -> c_int,
    /// The static file handler for the live server: true once it answered,
    /// or the bailout its work left pending.
    pub static_file: for<'a> fn(view::Server<'a>, Ctx) -> Result<bool, Bailout>,
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
    /// A connection's session (fd, default context, max body size, whether a
    /// coroutine server's, and the handler of each finished request). The
    /// context outlives the session.
    pub session_new: fn(i64, Ctx, u32, bool, Handler),
    /// One whole frame, at the start of the bytes, for the session of an fd;
    /// `SW_ERR` on failure.
    pub session_receive: fn(i64, &[u8]) -> c_int,
    /// The session of an fd goes.
    pub session_free: fn(i64),
}

/// A session's `onRequest` for a finished request: the stream's context and
/// the session's default one. A bailout in it returns as `Err`.
pub type Handler = Box<dyn Fn(Ctx, Ctx) -> Result<(), Bailout>>;

static SERVER: OnceLock<&'static Server> = OnceLock::new();

/// crates/phphttp2's MINIT: its entry points, kept for the process.
pub fn register(server: &'static Server) {
    SERVER.get_or_init(|| server);
}

/// The registered HTTP/2 server; None before its MINIT.
pub(crate) fn server() -> Option<&'static Server> {
    SERVER.get().copied()
}

/// `f` of the response's `Date` value, cached per second.
pub fn date<R>(f: impl FnOnce(&[u8]) -> R) -> R {
    header::date(f)
}

/// `SW_HTTP2_DATA_BUFFER_SIZE`.
const DATA_BUFFER_SIZE: usize = 8192;

/// Built with `SW_HAVE_COMPRESSION`.
fn compression() -> bool {
    CAPABILITIES.load(Relaxed) & 8 != 0
}

impl Ctx {
    /// `Context::set_header(key, value, false)` of a string value.
    pub fn set_header(self, key: &[u8], value: &[u8]) {
        crate::response::set_header_bytes(self, key, value);
    }

    /// `$request->server`, `header`, `get`, `post` or `cookie` (made on first
    /// use), or the response object.
    #[must_use]
    pub fn value(mut self, which: Value) -> *mut zval {
        let state = self.state();
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

    /// The response's `header`, `cookie` or `trailer` array, with a reference
    /// of its own: a value's `__toString` may replace or unset the property
    /// while the array is walked, and no borrow of the state outlives this.
    #[must_use]
    pub fn response_array(mut self, which: ResponseArray) -> Option<HeldArray> {
        header::property(&self.state().response.zobject, which as usize)
    }

    /// Whether the request body has bytes.
    #[must_use]
    pub fn has_body(mut self) -> bool {
        let data = self.state().request.h2_data.as_deref();
        data.is_some_and(|data| !data.is_empty())
    }

    /// `$request->post` from the whole body (`PARSE_STRING`).
    pub fn treat_post(self) {
        self.treat(Value::Post, None);
    }

    /// The `key` entry of `$request->server`, `header`, `get`, `post` or
    /// `cookie`, with a reference of its own: no borrow of the state outlives
    /// the lookup. None before the array is made.
    #[must_use]
    pub fn entry(mut self, which: Value, key: &[u8]) -> Option<Owned> {
        let request = &self.state().request;
        let slot = match which {
            Value::Server => &request.zserver,
            Value::Header => &request.zheader,
            Value::Get => &request.zget,
            Value::Post => &request.zpost,
            Value::Cookie => &request.zcookie,
            Value::Response => return None,
        };
        Some(slot.get()?.array()?.find(key)?.copy())
    }

    /// Start the request body (`SW_HTTP2_DATA_BUFFER_SIZE` reserved), unless
    /// it has started: the bytes it holds.
    #[must_use]
    pub fn data_start(mut self) -> usize {
        self.body().len()
    }

    /// Append a DATA frame's payload to the request body.
    pub fn data_append(mut self, payload: &[u8]) {
        self.body().extend_from_slice(payload);
    }

    /// The request body, started on first use.
    fn body(&mut self) -> &mut Vec<u8> {
        self.state()
            .request
            .h2_data
            .get_or_insert_with(|| Box::new(Vec::with_capacity(DATA_BUFFER_SIZE)))
    }

    /// `parse_multipart_data` over the whole body.
    #[must_use]
    pub fn multipart(mut self) -> bool {
        let state = self.state();
        let data = state.request.h2_data.take().unwrap_or_default();
        let parsed = parse::multipart_parse(state, &data);
        self.state().request.h2_data = Some(data);
        parsed
    }

    /// The context's `field` (0 for a compression field without compression).
    #[must_use]
    pub fn get(mut self, field: Field) -> u32 {
        let state = self.state();
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
    pub fn set(mut self, field: Field, value: u32) {
        let state = self.state();
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
        context::send(self, data)
    }

    /// `ctx->close`.
    pub fn close(self) {
        context::close(self);
    }

    /// `compression_types` lists the response's content type.
    #[must_use]
    pub fn compressible(mut self, kind: &[u8]) -> bool {
        compression() && context::compression_type(self.state(), Some(kind))
    }

    /// `Context::set_compression_method` from a request's `accept-encoding`.
    pub fn compression_method(mut self, accept_encoding: &[u8]) {
        parse::compression_method(self.state(), accept_encoding);
    }

    /// `Context::compress`: the encoding's name and the compressed body's
    /// length, or `None` to send `body` as it is.
    #[must_use]
    pub fn compress(mut self, body: &[u8]) -> Option<(&'static CStr, usize)> {
        header::compress_body(self.state(), body).map(|(encoding, body)| (encoding, body.len()))
    }

    /// The body the last [`Ctx::compress`] made.
    #[must_use]
    pub fn compressed(mut self) -> Vec<u8> {
        header::compressed(self.state()).to_vec()
    }

    /// The connection's session id.
    #[must_use]
    pub fn fd(mut self) -> i64 {
        self.state().fd
    }

    /// `~Stream`: the stream lets go of its context, which is complete. A
    /// request that never reached its handler (reset, or its connection
    /// closed, first) also releases the request and response objects, which
    /// only the context holds.
    /// # Safety
    /// A stream's context, released once; no copy of the handle is used after.
    /// # Errors
    /// A destructor bailed out.
    pub unsafe fn release_stream(mut self, dispatched: bool) -> Result<(), Bailout> {
        // Ended first, so the response's release answers nothing.
        self.state().end = 1;
        let released = if dispatched {
            Ok(())
        } else {
            context::release_objects(self)
        };
        self.state().stream_id = 0;
        // SAFETY: per the caller.
        unsafe { free(self) };
        released
    }

    /// The static handler answers the request: no server hook counts it.
    pub fn unhook(mut self) {
        let state = self.state();
        state.before_request = 0;
        state.after_response = 0;
    }

    /// `delete default_ctx`: a server connection's settings context, which no
    /// object or stream holds, goes with its session.
    /// # Safety
    /// Nothing else references the context, and no copy of the handle is used after.
    pub unsafe fn release_default(self) {
        // SAFETY: per the caller.
        unsafe { free(self) };
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
        let mut ctx = context::alloc();
        context::init(ctx.state(), server);
        let state = ctx.state();
        state.fd = fd;
        state.http2 = 1;
        state.keepalive = 1;
        state.before_request = 1;
        ctx
    }

    /// A stream's context: a copy of this settings context's, with its request
    /// and response objects and the request's `streamId`.
    #[must_use]
    pub fn stream_context(mut self, fd: i64, stream_id: u32) -> Self {
        let mut ctx = context::context_new(fd);
        context::copy_settings(ctx.state(), self.state());
        let state = ctx.state();
        state.http2 = 1;
        state.stream_id = stream_id;
        state.keepalive = 1;
        if let Some(request) = state.request.zobject.props() {
            request.long(c"streamId", i64::from(stream_id));
        }
        ctx
    }

    /// `ctx->request.path`: the `:path` up to its query.
    pub fn set_path(mut self, path: &[u8]) {
        let state = self.state();
        // The path's bytes, copied into request memory the context frees.
        state.request.path = parse::estrndup(path).cast();
        state.request.path_len = u32::try_from(path.len()).unwrap_or(u32::MAX);
    }

    /// A `multipart/form-data` content type's boundary starts the request's
    /// multipart parser; false when it has none.
    #[must_use]
    pub fn multipart_start(mut self, content_type: &[u8]) -> bool {
        parse::multipart_start(self.state(), content_type)
    }

    /// `sapi_module.treat_data(PARSE_STRING, ...)`: a query string's pairs into
    /// the request's `which` array.
    pub fn treat_query(self, which: Value, query: &[u8]) {
        self.treat(which, Some(query));
    }

    /// `PARSE_STRING` of `query`, or of the request body for `None`, into the
    /// request's `which` array.
    fn treat(mut self, which: Value, query: Option<&[u8]>) {
        let array = self.value(which);
        let body = self.state().request.h2_data.as_deref();
        // The bytes, copied into request memory the SAPI frees.
        let text = parse::estrndup(query.unwrap_or_else(|| body.map_or(&[], Vec::as_slice)));
        // SAFETY: a fresh copy and the request's live array. Neither the state
        // nor anything this frame owns is held while PHP parses it, which may
        // bail out.
        unsafe { swr_zend_treat_string(text, array) };
    }
}
