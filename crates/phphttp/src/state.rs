//! `swoole::http::Context`: a request's and response's state, which
//! `context::alloc` allocates and Request and Response point at.
use std::{
    collections::HashSet,
    ffi::{CString, c_int, c_void},
    rc::Rc,
};
use swoole_rs_zend::{Declared, Kept, Owned, Props, UNDEF, Value, sys::zval};

/// A context's request or response object: empty until the context sets it.
#[repr(transparent)]
pub(crate) struct Slot(Kept);

impl std::ops::Deref for Slot {
    type Target = Kept;

    fn deref(&self) -> &Kept {
        &self.0
    }
}

impl std::ops::DerefMut for Slot {
    fn deref_mut(&mut self) -> &mut Kept {
        &mut self.0
    }
}

impl Slot {
    /// `swoole_http_init_and_read_property`: the slot of the object's declared
    /// `property`, which holds an array made on first use, bit `which` of
    /// `made`. Read at each use, the slot holds whatever the property now does.
    #[inline]
    pub(crate) fn array(
        &self,
        made: &mut u8,
        which: usize,
        property: &Declared,
        size: u32,
    ) -> Option<&Value> {
        let object = self.get()?;
        let bit = 1 << which;
        if *made & bit != 0 {
            return property.get(object);
        }
        let array = property.init_array(object, size)?;
        *made |= bit;
        Some(array)
    }

    /// `ZVAL_COPY_VALUE`: the zval's bits without a reference of their own
    /// (`IS_UNDEF` while empty), for Zend to borrow.
    pub(crate) fn copy_value(&self) -> zval {
        self.get().map_or(UNDEF, Value::copy_value)
    }

    /// Property writes on the slot's object, under its own class; None
    /// before the context sets the slot.
    pub(crate) fn props(&self) -> Option<Props<'_>> {
        self.get()?.props(None).map(Props::own)
    }
}

/// `swoole::http::Request`.
pub(crate) struct Request {
    pub(crate) version: c_int,
    /// The path up to its query: `on_url`'s, or the HTTP/2 `:path`'s.
    pub(crate) path: Option<Box<[u8]>>,
    pub(crate) post_form_urlencoded: bool,
    /// The request's bytes, a string once any arrive.
    pub(crate) zdata: Owned,
    /// Where the body (unless chunked) starts in the input that holds it.
    pub(crate) body_at: usize,
    pub(crate) body_length: usize,
    /// An HTTP/2 request's body, as its DATA frames arrive.
    #[expect(
        clippy::box_collection,
        reason = "all zeroes must be valid: a null Box is None"
    )]
    pub(crate) h2_data: Option<Box<Vec<u8>>>,
    pub(crate) zobject: Slot,
    /// The array properties made so far, a bit per `parse::Property`.
    pub(crate) arrays: u8,
}

/// The parser's own bookkeeping (`ContextState::parse_`). All zeroes is the
/// initial state, which `context::alloc` writes.
pub(crate) struct Parse {
    pub(crate) header_value_len: usize,
    /// `Request::parse()`: spans llhttp flushed at the end of an input buffer.
    pub(crate) url_length: usize,
    pub(crate) header_name_offset: usize,
    pub(crate) header_name_length: usize,
    pub(crate) header_value_length: usize,
    pub(crate) form_data_name: Option<Box<[u8]>>,
    pub(crate) input_name: Option<Box<[u8]>>,
    /// The current upload's `$_FILES` entry (`IS_UNDEF` when there is none).
    pub(crate) multipart_header: Owned,
    #[expect(
        clippy::box_collection,
        reason = "all zeroes must be valid: a null Box is None"
    )]
    pub(crate) form_data: Option<Box<Vec<u8>>>,
    #[expect(
        clippy::box_collection,
        reason = "all zeroes must be valid: a null Box is None"
    )]
    pub(crate) chunked_body: Option<Box<Vec<u8>>>,
    pub(crate) input_var_num: u16,
}

/// `swoole::http::ContextState`.
pub(crate) struct State {
    pub(crate) fd: i64,
    pub(crate) request: Request,
    pub(crate) parser: swoole_rs_http::llhttp::Parser,
    pub(crate) mt_parser: Option<Box<crate::parse::Multipart>>,
    pub(crate) completed: bool,
    pub(crate) keepalive: bool,
    pub(crate) recv_chunked: bool,
    pub(crate) websocket: bool,
    pub(crate) parse_cookie: bool,
    pub(crate) parse_body: bool,
    pub(crate) parse_files: bool,
    pub(crate) enable_compression: bool,
    pub(crate) accept_compression: bool,
    pub(crate) compression_method: swoole_rs_httpbindings::Compression,
    pub(crate) parse: Parse,
    pub(crate) response: Response,
    pub(crate) compression_min_length: u32,
    pub(crate) compression_level: i8,
    pub(crate) send_chunked: bool,
    pub(crate) upgrade: bool,
    pub(crate) send_header: bool,
    pub(crate) content_compressed: bool,
    /// `end_`: the response is complete.
    pub(crate) end: bool,
    /// `send_trailer_`: `trailer()` set one.
    pub(crate) send_trailer: bool,
    /// Upstream's `websocket_compression`, `detached` and `http2` bits.
    pub(crate) websocket_compression: bool,
    pub(crate) detached: bool,
    pub(crate) http2: bool,
    /// The coroutine socket; `IS_NULL` once bound to a server.
    pub(crate) zsocket: Owned,
    /// The HTTP/2 stream, 0 once it is released.
    pub(crate) stream_id: u32,
    /// The `Server` a server's context answers through (`private_data`).
    pub(crate) private_data: *mut c_void,
    /// The coroutine socket of `zsocket`'s object, while that is bound; the
    /// all-zeroes handle until then, which nothing reads.
    pub(crate) socket: swoole_rs_phpruntime::co::Co,
    /// The response bytes being sent (upstream's `write_buffer`).
    #[expect(
        clippy::box_collection,
        reason = "all zeroes must be valid: a null Box is None"
    )]
    pub(crate) write_buffer: Option<Box<Vec<u8>>>,
    /// The compressed body (upstream's `zlib_buffer`).
    #[expect(
        clippy::box_collection,
        reason = "all zeroes must be valid: a null Box is None"
    )]
    pub(crate) compressed: Option<Box<Vec<u8>>>,
    /// `private_data_2`: the handler the server picked for the request; none
    /// (all zeroes) until it picks one.
    pub(crate) callback: Option<std::rc::Rc<crate::server::Callable>>,
    /// `onBeforeRequest` is the server's: count or queue the request first.
    pub(crate) before_request: bool,
    /// `onAfterResponse` is the server's: uncount the request.
    pub(crate) after_response: bool,
    /// A coroutine server's `WebSocketSettings` (`websocket::SETTING_*` bits).
    pub(crate) websocket_settings: u8,
    /// Only the reactor's dispatch sets this: it rewrote the body, so its
    /// `Swoole-Upload-File` headers name files it saved. Other input is a client's.
    pub(crate) upload_preprocessed: bool,
    /// A coroutine server's or `Request::create`'s `upload_tmp_dir`; a
    /// server's context reads its server's.
    pub(crate) upload_tmp_dir: Option<Rc<CString>>,
    /// A coroutine server's `http_compression_types`; a server's context
    /// reads its server's.
    pub(crate) compression_types: Option<Rc<HashSet<Box<[u8]>>>>,
    /// A coroutine server's WebSocket message being assembled from fragments.
    pub(crate) ws_buffer: Option<Box<crate::websocket::Message>>,
}

/// `swoole::http::Response`.
pub(crate) struct Response {
    pub(crate) status: c_int,
    /// `status()`'s reason, as `%s` prints it: up to its first NUL.
    pub(crate) reason: Option<Box<CString>>,
    pub(crate) zobject: Slot,
    /// The array properties made so far, a bit per `host::Property`.
    pub(crate) arrays: u8,
}
