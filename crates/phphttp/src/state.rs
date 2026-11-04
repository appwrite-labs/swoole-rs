//! `swoole::http::ContextState`: the part of `swoole::http::Context` the
//! request parser reads and writes, laid out as `ext-src/php_swoole_http.h`
//! declares it (C++ asserts the size against `SWR_PHPHTTP_STATE_SIZE`).
use std::{
    collections::HashSet,
    ffi::{CString, c_char, c_int, c_void},
    rc::Rc,
};
use swoole_rs_zend::sys::zval;

/// `llhttp_t` (`thirdparty/llhttp/llhttp.h`, `llhttp__internal_s`).
#[repr(C)]
pub(crate) struct Llhttp {
    index: i32,
    span_pos0: *mut c_void,
    span_cb0: *mut c_void,
    pub(crate) error: i32,
    pub(crate) reason: *const c_char,
    error_pos: *const c_char,
    pub(crate) data: *mut c_void,
    current: *mut c_void,
    pub(crate) content_length: u64,
    kind: u8,
    pub(crate) method: u8,
    pub(crate) http_major: u8,
    pub(crate) http_minor: u8,
    header_state: u8,
    lenient_flags: u16,
    upgrade: u8,
    finish: u8,
    pub(crate) flags: u16,
    status_code: u16,
    initial_message_completed: u8,
    pub(crate) settings: *const c_void,
}

/// A `zval *` and the `zval` it points at once set (`Request`'s pairs).
#[repr(C)]
pub(crate) struct Slot {
    pub(crate) ptr: *mut zval,
    pub(crate) value: zval,
}

/// `swoole::http::Request`.
#[repr(C)]
pub(crate) struct Request {
    pub(crate) version: c_int,
    pub(crate) path: *mut c_char,
    pub(crate) path_len: u32,
    pub(crate) ext: *const c_char,
    pub(crate) ext_len: u32,
    pub(crate) post_form_urlencoded: u8,
    pub(crate) zdata: zval,
    pub(crate) body_at: *const c_char,
    pub(crate) body_length: usize,
    h2_data_buffer: *mut c_void,
    pub(crate) zobject: Slot,
    pub(crate) zserver: Slot,
    pub(crate) zheader: Slot,
    pub(crate) zget: Slot,
    pub(crate) zpost: Slot,
    pub(crate) zcookie: Slot,
    pub(crate) zfiles: Slot,
    pub(crate) ztmpfiles: Slot,
}

/// The parser's own bookkeeping (`ContextState::parse_`), Rust's alone. All
/// zeroes is the initial state: C++ value-initialises the context.
#[repr(C)]
pub(crate) struct Parse {
    pub(crate) header_name: *const u8,
    pub(crate) header_name_len: usize,
    pub(crate) header_value_len: usize,
    /// `Request::parse()`: spans llhttp flushed at the end of an input buffer.
    pub(crate) url_length: usize,
    pub(crate) header_name_offset: usize,
    pub(crate) header_name_length: usize,
    pub(crate) header_value_length: usize,
    pub(crate) tmp_content_type: *const u8,
    pub(crate) tmp_content_type_len: usize,
    pub(crate) form_data_name: Option<Box<[u8]>>,
    pub(crate) input_name: Option<Box<[u8]>>,
    /// The current upload's `$_FILES` entry (`IS_UNDEF` when there is none).
    pub(crate) multipart_header: zval,
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
#[repr(C)]
pub(crate) struct State {
    pub(crate) fd: i64,
    pub(crate) request: Request,
    pub(crate) parser: Llhttp,
    pub(crate) mt_parser: *mut crate::parse::Multipart,
    pub(crate) completed: u8,
    pub(crate) keepalive: u8,
    pub(crate) recv_chunked: u8,
    pub(crate) websocket: u8,
    pub(crate) parse_cookie: u8,
    pub(crate) parse_body: u8,
    pub(crate) parse_files: u8,
    pub(crate) enable_compression: u8,
    pub(crate) accept_compression: u8,
    pub(crate) compression_method: i8,
    pub(crate) parse: Parse,
    pub(crate) response: Response,
    pub(crate) compression_min_length: u32,
    pub(crate) compression_level: i8,
    pub(crate) send_chunked: u8,
    pub(crate) upgrade: u8,
    pub(crate) send_header: u8,
    pub(crate) content_compressed: u8,
    /// `end_`: the response is complete.
    pub(crate) end: u8,
    /// `send_trailer_`: `trailer()` set one.
    pub(crate) send_trailer: u8,
    /// Upstream's `websocket_compression`, `detached` and `http2` bits.
    pub(crate) websocket_compression: u8,
    pub(crate) detached: u8,
    pub(crate) http2: u8,
    /// The coroutine socket; `IS_NULL` once bound to a server.
    pub(crate) zsocket: zval,
    /// The HTTP/2 stream, 0 once it is released.
    pub(crate) stream_id: u32,
    /// The `Server` or the coroutine `Socket`.
    pub(crate) private_data: *mut c_void,
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
    /// `private_data_2`: the handler the server picked for the request.
    pub(crate) callback: *mut c_void,
    /// `onBeforeRequest` is the server's: count or queue the request first.
    pub(crate) before_request: u8,
    /// `onAfterResponse` is the server's: uncount the request.
    pub(crate) after_response: u8,
    /// A coroutine server's `WebSocketSettings` (`websocket::SETTING_*` bits).
    pub(crate) websocket_settings: u8,
    /// A coroutine server's or `Request::create`'s `upload_tmp_dir`; a
    /// server's context reads its server's.
    pub(crate) upload_tmp_dir: Option<Rc<CString>>,
    /// A coroutine server's `http_compression_types`; a server's context
    /// reads its server's.
    pub(crate) compression_types: Option<Rc<HashSet<Box<[u8]>>>>,
}

/// `swoole::http::Response`.
#[repr(C)]
pub(crate) struct Response {
    method: c_int,
    version: c_int,
    pub(crate) status: c_int,
    pub(crate) reason: *mut c_char,
    pub(crate) zobject: Slot,
    zheader: Slot,
    zcookie: Slot,
    ztrailer: Slot,
}

/// `SWR_PHPHTTP_PARSE_SIZE` and `SWR_PHPHTTP_STATE_SIZE`.
const _: () = assert!(size_of::<Parse>() == 144, "ext-src/php_swoole_http.h");
const _: () = assert!(size_of::<State>() == 760, "ext-src/php_swoole_http.h");
const _: () = assert!(size_of::<Response>() == 120, "ext-src/php_swoole_http.h");
const _: () = assert!(size_of::<Llhttp>() == 96, "ext-src/php_swoole_http.h");
const _: () = assert!(size_of::<Request>() == 272, "ext-src/php_swoole_http.h");
const _: () = assert!(
    std::mem::offset_of!(State, parse) == 400,
    "ext-src/php_swoole_http.h"
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_fields_follow_the_response_state() {
        assert_eq!(std::mem::offset_of!(State, websocket_compression), 675);
        assert_eq!(std::mem::offset_of!(State, zsocket), 680);
        assert_eq!(std::mem::offset_of!(State, stream_id), 696);
        assert_eq!(std::mem::offset_of!(State, write_buffer), 712);
        assert_eq!(std::mem::offset_of!(State, compressed), 720);
        assert_eq!(std::mem::offset_of!(State, callback), 728);
        assert_eq!(std::mem::offset_of!(State, before_request), 736);
        assert_eq!(std::mem::offset_of!(State, websocket_settings), 738);
        assert_eq!(std::mem::offset_of!(State, upload_tmp_dir), 744);
        assert_eq!(std::mem::offset_of!(State, compression_types), 752);
    }
}
