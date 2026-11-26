//! HTTP/2 server sessions, stream contexts, files and responses.
#![forbid(unsafe_code)]
mod files;
mod server;
mod session;

use crate::{context, header, parse};
pub(crate) use files::{send_file, static_file};
pub use server::{entries, server_on_close};
pub(crate) use server::{goaway, on_receive, ping, reset, respond, session_receive, write};
pub use session::IdMap;
pub(crate) use session::{session_free, session_new};
use std::{cell::Cell, ffi::CStr};
use swoole_rs_corebase::{CCast, buffer::BoundedBytes};
use swoole_rs_server::view;
use swoole_rs_zend::{
    ArrayMut, HeldArray, Owned, ZString,
    bailout::{self, Bailout},
};

/// The HTTP/2 server's contexts: a connection's settings context
/// ([`Ctx::server_context`]) or a stream's ([`Ctx::stream_context`]), and the
/// holds that keep them.
pub use crate::context::{Ctx, Hold};

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

/// A session's `onRequest` for a finished request: the stream's context and
/// the session's default one. A bailout in it returns as `Err`.
pub type Handler = Box<dyn Fn(Ctx, Ctx) -> Result<(), Bailout>>;

/// An HTTP/2 stream's hold on its context ([`Ctx::stream_context`]), let go
/// when this drops (`~Stream`): the context is complete by then, and goes once
/// its request and response objects have. A request that never reached its
/// handler (reset, or its connection closed, first) also releases those
/// objects, which only the context holds; a bailout in a destructor is deferred.
pub struct StreamCtx {
    hold: Hold,
    dispatched: Cell<bool>,
}

impl StreamCtx {
    #[must_use]
    pub fn ctx(&self) -> Ctx<'_> {
        self.hold.ctx()
    }

    /// The request goes to its handler, which releases its objects.
    pub fn dispatch(&self) {
        self.dispatched.set(true);
    }
}

impl Drop for StreamCtx {
    fn drop(&mut self) {
        let mut ctx = self.hold.ctx();
        // Ended first, so the response's release answers nothing.
        ctx.state().end = true;
        if !self.dispatched.get() {
            bailout::defer(context::release_objects(ctx));
        }
        ctx.state().stream_id = 0;
    }
}

/// `f` of the response's `Date` value, cached per second.
pub fn date<R>(f: impl FnOnce(&[u8]) -> R) -> R {
    header::date(f)
}

/// `SW_HTTP2_DATA_BUFFER_SIZE`.
const DATA_BUFFER_SIZE: usize = swoole_rs_corebase::constants::SW_HTTP2_DATA_BUFFER_SIZE as usize;

/// Built with `SW_HAVE_COMPRESSION`.
fn compression() -> bool {
    parse::enabled_codecs() != 0
}

impl Ctx<'_> {
    /// `Context::set_header(key, value, false)` of a string value.
    pub fn set_header(self, key: &[u8], value: &[u8]) {
        crate::response::set_header_bytes(self, key, value);
    }

    /// `$request->header` and `$request->server`, made unless they exist: a
    /// header block makes both, whatever it holds.
    pub fn header_arrays(mut self) {
        let request = &mut self.state().request;
        parse::init_property(request, parse::Property::Header);
        parse::init_property(request, parse::Property::Server);
    }

    /// `add_assoc_str_ex` on `$request->server` or `header` (made on first
    /// use): the array takes `value` under `key`. Nothing for another `which`.
    pub fn insert(mut self, which: Value, key: &[u8], value: ZString) {
        let state = self.state();
        let property = match which {
            Value::Server => parse::Property::Server,
            Value::Header => parse::Property::Header,
            _ => return,
        };
        parse::edit(&mut state.request, property, |array| {
            array.insert(key, Owned::string(value));
        });
    }

    /// `$request->server`, `header`, `get`, `post` or `cookie` (made on first
    /// use), or the response object.
    fn edit_value<R>(
        state: &mut crate::state::State,
        which: Value,
        edit: impl FnOnce(&mut ArrayMut) -> R,
    ) -> Option<R> {
        let property = match which {
            Value::Response => return None,
            Value::Server => parse::Property::Server,
            Value::Header => parse::Property::Header,
            Value::Get => parse::Property::Get,
            Value::Post => parse::Property::Post,
            Value::Cookie => parse::Property::Cookie,
        };
        parse::edit(&mut state.request, property, edit)
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
        data.is_some_and(|data| !data.as_ref().is_empty())
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
        let property = match which {
            Value::Server => parse::Property::Server,
            Value::Header => parse::Property::Header,
            Value::Get => parse::Property::Get,
            Value::Post => parse::Property::Post,
            Value::Cookie => parse::Property::Cookie,
            Value::Response => return None,
        };
        let value = parse::made(&self.state().request, property)?;
        let array = value.array()?;
        Some(array.find(key)?.copy())
    }

    /// Append one DATA payload within the session's request body limit.
    fn data_append(mut self, payload: &[u8], limit: u32) -> Result<(), usize> {
        self.state()
            .request
            .h2_data
            .get_or_insert_with(|| Box::new(BoundedBytes::new(limit as usize, DATA_BUFFER_SIZE)))
            .append(payload)
    }

    /// `parse_multipart_data` over the whole body.
    #[must_use]
    pub fn multipart(mut self) -> bool {
        let state = self.state();
        let data = state.request.h2_data.take();
        let parsed = parse::multipart_parse(state, data.as_deref().map_or(&[], AsRef::as_ref));
        self.state().request.h2_data = data;
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
            Field::Status => state.response.status.c_cast::<u32>(),
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
    pub fn set(mut self, field: Field, value: u32) {
        let state = self.state();
        match field {
            Field::End => state.end = value != 0,
            Field::SendHeader => state.send_header = value != 0,
            Field::SendChunked => state.send_chunked = value != 0,
            Field::Status => state.response.status = value.c_cast::<i32>(),
            Field::Form => state.request.post_form_urlencoded = value != 0,
            Field::AcceptCompression if compression() => state.accept_compression = value != 0,
            _ => {}
        }
    }

    /// `ctx->send`.
    #[must_use]
    pub fn send(self, data: &[u8]) -> bool {
        context::send(self, data)
    }

    /// How long a send may wait on the peer (upstream 6.3's
    /// `http2_server_send_timeout`): a server context's `send_timeout`, else upstream's
    /// default write timeout. A coroutine socket's context never waits on its window.
    #[must_use]
    pub fn send_deadline(mut self) -> swoole_rs_corebase::timer::Deadline {
        let state = self.state();
        let timeout = if context::co_socket(state) {
            0.0
        } else {
            context::server(state).config().send_timeout.get()
        };
        swoole_rs_corebase::timer::Deadline::of_write(timeout)
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

    /// The static handler answers the request: no server hook counts it.
    pub fn unhook(mut self) {
        let state = self.state();
        state.before_request = false;
        state.after_response = false;
    }

    /// `swoole_http_parse_cookie`: a `cookie` header's pairs into the request's
    /// `$cookie`.
    pub fn cookie(mut self, value: &[u8]) {
        Self::edit_value(self.state(), Value::Cookie, |array| {
            parse::parse_cookie(array.separate(), value);
        });
    }

    /// A server connection's settings context (`default_ctx`): the server's
    /// settings, for session `fd`, counted by the request hooks.
    #[must_use]
    pub fn server_context(server: view::Server<'_>, fd: i64) -> Hold {
        let hold = context::alloc();
        let mut ctx = hold.ctx();
        context::init(ctx.state(), server);
        let state = ctx.state();
        state.fd = fd;
        state.http2 = true;
        state.keepalive = true;
        state.before_request = true;
        hold
    }

    /// A stream's context: a copy of this settings context's, with its request
    /// and response objects and the request's `streamId`.
    #[must_use]
    pub fn stream_context(mut self, fd: i64, stream_id: u32) -> StreamCtx {
        let hold = context::context_new(fd);
        let mut ctx = hold.ctx();
        context::copy_settings(ctx.state(), self.state());
        let state = ctx.state();
        state.http2 = true;
        state.stream_id = stream_id;
        state.keepalive = true;
        if let Some(request) = state.request.zobject.props() {
            request.long(c"streamId", i64::from(stream_id));
        }
        StreamCtx {
            hold,
            dispatched: Cell::new(false),
        }
    }

    /// `ctx->request.path`: the `:path` up to its query.
    pub fn set_path(mut self, path: &[u8]) {
        let state = self.state();
        state.request.path = Some(path.into());
    }

    /// A `multipart/form-data` content type's boundary starts the request's
    /// multipart parser; false when it has none.
    #[must_use]
    pub fn multipart_start(mut self, content_type: &[u8], limit: u32) -> bool {
        parse::multipart_start(self.state(), content_type, limit as usize)
    }

    /// `sapi_module.treat_data(PARSE_STRING, ...)`: a query string's pairs into
    /// the request's `which` array.
    pub fn treat_query(self, which: Value, query: &[u8]) {
        self.treat(which, Some(query));
    }

    /// `PARSE_STRING` of `query`, or of the request body for `None`, into the
    /// request's `which` array.
    fn treat(mut self, which: Value, query: Option<&[u8]>) {
        let state = self.state();
        let body = state.request.h2_data.as_deref();
        // The bytes, copied into request memory the SAPI frees. Nothing this
        // frame owns is held while PHP parses them, which may bail out.
        let text = parse::Copied::of(query.unwrap_or_else(|| body.map_or(&[], AsRef::as_ref)));
        Self::edit_value(state, which, |array| {
            if let Some(array) = array.separate() {
                parse::treat_string(text, array);
            }
        });
    }
}
