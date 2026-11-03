//! `http_server::listen`'s request parsing (#931): the llhttp and multipart callbacks,
//! over the wrapper's [`Request`]. The parsers stay the third-party C ones; native code
//! (`src/wrapper/http.cc`'s `wrapper_native`) fills the `Context`'s strings and maps,
//! runs the parsers and does the upload file IO.
use crate::{
    Request, View, swr_v1_wrappers_body_append, swr_v1_wrappers_body_skip,
    swr_v1_wrappers_complete, swr_v1_wrappers_disposition, swr_v1_wrappers_header,
    swr_v1_wrappers_header_kind, swr_v1_wrappers_part_end, swr_v1_wrappers_request_view,
    swr_v1_wrappers_split,
};
use core::ffi::{c_char, c_int, c_void};
use core::ptr::{self, NonNull};

/// One native effect on the request's `Context` (`SWR_WRAPPER_HTTP_*`).
pub type Native =
    unsafe extern "C" fn(*mut c_void, u32, *mut c_void, *const u8, usize, *const u8, usize) -> u64;

// `ctx->request_path`, `ctx->query_string`, `ctx->headers[a] = b`, `post_form_urlencoded`.
const PATH: u32 = 0;
const QUERY: u32 = 1;
const HEADER: u32 = 2;
const FORM_URLENCODED: u32 = 3;
/// Version, server protocol and keep-alive from the llhttp parser in `object`.
const HEADERS_COMPLETE: u32 = 4;
/// `ctx->body.append(a)`.
const BODY: u32 = 5;
/// A parser over the boundary in the `Content-Type` value `a` (settings `object`): it, or null.
const MULTIPART_NEW: u32 = 6;
/// Closes the parser's upload file, then frees the parser.
const MULTIPART_FREE: u32 = 7;
/// `multipart_parser_execute(object, a)`.
const MULTIPART_EXECUTE: u32 = 8;
/// The `an/bn bytes processed` warning.
const LOG_PARSE: u32 = 9;
/// `ctx->files[a] = b`; whether `ctx->files` has `a`.
const FILE: u32 = 10;
const HAS_FILE: u32 = 11;
/// `fwrite(a)` to the upload file `object`; on a short write, upstream's cleanup for
/// upload `b` and its warning, and 1.
const WRITE: u32 = 12;
/// A temporary file for upload `a`, opened and recorded in `ctx->files`: its `FILE *`.
const OPEN: u32 = 13;
/// `ctx->form_data[a] = b`.
const FORM_DATA: u32 = 14;
const FCLOSE: u32 = 15;
/// Whether the `Content-Type` value `a` has a multipart boundary.
const BOUNDARY: u32 = 16;

/// llhttp's `HPE_PAUSED`.
const PAUSED: c_int = 21;

/// The head of `llhttp_t`; native code asserts the layout.
#[repr(C)]
pub struct Llhttp {
    _index: i32,
    _span_pos0: *mut c_void,
    _span_cb0: *mut c_void,
    _error: i32,
    _reason: *const c_char,
    _error_pos: *const c_char,
    data: *mut c_void,
    _current: *mut c_void,
    _content_length: u64,
    _kind: u8,
    method: u8,
}

/// The head of third-party `struct multipart_parser`; native code asserts the layout.
#[repr(C)]
pub struct Multipart {
    _settings: *const c_void,
    _index: usize,
    _error_i: usize,
    _boundary_length: u8,
    fp: *mut c_void,
    data: *mut c_void,
}

type Data<P> = Option<unsafe extern "C" fn(*mut P, *const c_char, usize) -> c_int>;
type Notify<P> = Option<unsafe extern "C" fn(*mut P) -> c_int>;

/// `llhttp_settings_t`: 25 callbacks.
#[repr(C)]
pub struct LlhttpSettings {
    on_message_begin: Notify<Llhttp>,
    on_protocol: Data<Llhttp>,
    on_url: Data<Llhttp>,
    on_status: Data<Llhttp>,
    on_method: Data<Llhttp>,
    on_version: Data<Llhttp>,
    on_header_field: Data<Llhttp>,
    on_header_value: Data<Llhttp>,
    on_chunk_extension_name: Data<Llhttp>,
    on_chunk_extension_value: Data<Llhttp>,
    on_headers_complete: Notify<Llhttp>,
    on_body: Data<Llhttp>,
    on_message_complete: Notify<Llhttp>,
    /// `on_protocol_complete` through `on_reset`.
    rest: [Notify<Llhttp>; 12],
}

/// `multipart_parser_settings`.
#[repr(C)]
pub struct MultipartSettings {
    on_header_field: Data<Multipart>,
    on_header_value: Data<Multipart>,
    on_part_data: Data<Multipart>,
    on_part_data_begin: Notify<Multipart>,
    on_headers_complete: Notify<Multipart>,
    on_part_data_end: Notify<Multipart>,
    on_body_end: Notify<Multipart>,
}

static HTTP: LlhttpSettings = LlhttpSettings {
    on_message_begin: None,
    on_protocol: None,
    on_url: Some(on_url),
    on_status: None,
    on_method: None,
    on_version: None,
    on_header_field: Some(on_header_field),
    on_header_value: Some(on_header_value),
    on_chunk_extension_name: None,
    on_chunk_extension_value: None,
    on_headers_complete: Some(on_headers_complete),
    on_body: Some(on_body),
    on_message_complete: Some(on_message_complete),
    rest: [None; 12],
};

static MULTIPART: MultipartSettings = MultipartSettings {
    on_header_field: Some(on_part_header_field),
    on_header_value: Some(on_part_header_value),
    on_part_data: Some(on_part_data),
    on_part_data_begin: None,
    on_headers_complete: Some(on_part_headers_complete),
    on_part_data_end: Some(on_part_data_end),
    on_body_end: None,
};

/// A request's `Context` and its open multipart parser.
#[derive(Default)]
pub(crate) struct Binding {
    context: Option<(NonNull<c_void>, Native)>,
    multipart: Option<NonNull<Multipart>>,
}

impl Request {
    fn op(&self, op: u32, object: *mut c_void, a: &[u8], b: &[u8]) -> u64 {
        // Callbacks run only from a parse over a bound request.
        let Some((context, native)) = self.binding.context else {
            std::process::abort();
        };
        // SAFETY: the native call runs one effect on the live `Context`.
        unsafe {
            native(
                context.as_ptr(),
                op,
                object,
                a.as_ptr(),
                a.len(),
                b.as_ptr(),
                b.len(),
            )
        }
    }

    fn view(&self) -> View {
        let mut view = View::default();
        // SAFETY: both are live; the view borrows this request's own buffers.
        unsafe { swr_v1_wrappers_request_view(self, &raw mut view) };
        view
    }

    /// Frees an open multipart parser (`ContextImpl::clear_multipart`).
    pub(crate) fn clear_multipart(&mut self) {
        if let Some(parser) = self.binding.multipart.take() {
            self.op(MULTIPART_FREE, parser.as_ptr().cast(), &[], &[]);
        }
    }
}

/// The request a parser callback runs for.
///
/// # Safety
/// `data` is the live request [`swr_v1_wrappers_request_bind`] bound and the parse
/// passed, with no other borrow live.
unsafe fn request<'a>(data: *mut c_void) -> &'a mut Request {
    // SAFETY: per the contract.
    unsafe { &mut *data.cast::<Request>() }
}

/// The request an llhttp callback runs for.
///
/// # Safety
/// `parser` is the live parser `ContextImpl::parse` made over a bound request, of
/// which no other borrow is live.
unsafe fn http<'a>(parser: *mut Llhttp) -> &'a mut Request {
    // SAFETY: per the contract.
    let data = unsafe { (*parser).data };
    // SAFETY: per the contract.
    unsafe { request(data) }
}

/// The parser and request a multipart callback runs for.
///
/// # Safety
/// `parser` is a live parser `on_header_value` bound to a request, of which no other
/// borrow is live.
unsafe fn part<'a>(parser: *mut Multipart) -> (&'a mut Multipart, &'a mut Request) {
    // SAFETY: per the contract.
    let parser = unsafe { &mut *parser };
    // SAFETY: per the contract.
    let request = unsafe { request(parser.data) };
    (parser, request)
}

/// # Safety
/// `at` lends `length` bytes for the call.
unsafe fn span<'a>(at: *const c_char, length: usize) -> &'a [u8] {
    if length == 0 {
        return &[];
    }
    // SAFETY: per the contract.
    unsafe { core::slice::from_raw_parts(at.cast(), length) }
}

/// The view's span as bytes.
fn text<'a>(span: crate::Span) -> &'a [u8] {
    // SAFETY: a view's spans borrow the request's live buffers.
    unsafe { span_of(span.data, span.len) }
}

/// # Safety
/// `data` lends `len` bytes while the slice lives.
unsafe fn span_of<'a>(data: *const u8, len: usize) -> &'a [u8] {
    // SAFETY: per the contract.
    unsafe { span(data.cast(), len) }
}

unsafe extern "C" fn on_url(parser: *mut Llhttp, at: *const c_char, length: usize) -> c_int {
    // SAFETY: llhttp calls back with the parser `parse` made over a bound request.
    let r = unsafe { http(parser) };
    // SAFETY: llhttp lends the URL bytes.
    let url = unsafe { span(at, length) };
    // SAFETY: as above.
    let path = unsafe { swr_v1_wrappers_split(url.as_ptr(), url.len(), b'?') };
    r.op(PATH, ptr::null_mut(), &url[..path], &[]);
    if path < length && length - path > 1 {
        r.op(QUERY, ptr::null_mut(), &url[path + 1..], &[]);
    }
    0
}

unsafe extern "C" fn on_header_field(
    parser: *mut Llhttp,
    at: *const c_char,
    length: usize,
) -> c_int {
    // SAFETY: llhttp calls back with the parser `parse` made over a bound request.
    let r = unsafe { http(parser) };
    // SAFETY: llhttp lends the field bytes.
    unsafe { swr_v1_wrappers_header(r, at.cast(), length) }
}

unsafe extern "C" fn on_header_value(
    parser: *mut Llhttp,
    at: *const c_char,
    length: usize,
) -> c_int {
    // SAFETY: llhttp calls back with its live parser.
    let method = unsafe { (*parser).method };
    // SAFETY: llhttp calls back with the parser `parse` made over a bound request.
    let r = unsafe { http(parser) };
    // SAFETY: llhttp lends the value bytes.
    let value = unsafe { span(at, length) };
    r.op(HEADER, ptr::null_mut(), text(r.view().header), value);
    // SAFETY: as above.
    let kind =
        unsafe { swr_v1_wrappers_header_kind(r, method.into(), value.as_ptr(), value.len(), 0) };
    if kind == 1 {
        r.op(FORM_URLENCODED, ptr::null_mut(), &[], &[]);
    } else if kind == 2 {
        if r.op(BOUNDARY, ptr::null_mut(), value, &[]) == 0 {
            return -1;
        }
        r.clear_multipart();
        let made = r.op(
            MULTIPART_NEW,
            (&raw const MULTIPART).cast_mut().cast(),
            value,
            &[],
        );
        let Some(multipart) = NonNull::new(made as *mut Multipart) else {
            return -1;
        };
        // SAFETY: a parser native code just made.
        unsafe { (*multipart.as_ptr()).data = (&raw mut *r).cast() };
        r.binding.multipart = Some(multipart);
    }
    if kind < 0 { -1 } else { 0 }
}

unsafe extern "C" fn on_headers_complete(parser: *mut Llhttp) -> c_int {
    // SAFETY: llhttp calls back with the parser `parse` made over a bound request.
    let r = unsafe { http(parser) };
    r.op(HEADERS_COMPLETE, parser.cast(), &[], &[]);
    0
}

unsafe extern "C" fn on_body(parser: *mut Llhttp, at: *const c_char, length: usize) -> c_int {
    if length == 0 {
        return 0;
    }
    // SAFETY: llhttp calls back with the parser `parse` made over a bound request.
    let r = unsafe { http(parser) };
    // SAFETY: llhttp lends the body bytes.
    let body = unsafe { span(at, length) };
    if let Some(multipart) = r.binding.multipart {
        // SAFETY: as above.
        let skip = unsafe { swr_v1_wrappers_body_skip(r, body.as_ptr(), body.len()) };
        let part = &body[skip..];
        // The parser calls back into this request, so no borrow of it is live then.
        let request = (&raw mut *r).cast::<c_void>();
        let n = raw_op(
            request,
            MULTIPART_EXECUTE,
            multipart.as_ptr().cast(),
            part.as_ptr(),
            part.len(),
            0,
        );
        if n != part.len() as u64 {
            // The two counts travel as lengths.
            let processed = usize::try_from(n).unwrap_or(usize::MAX);
            raw_op(
                request,
                LOG_PARSE,
                ptr::null_mut(),
                ptr::null(),
                processed,
                part.len(),
            );
        }
    } else {
        r.op(BODY, ptr::null_mut(), body, &[]);
    }
    // SAFETY: as above; the multipart parser's callbacks are done.
    let r = unsafe { http(parser) };
    if r.view().flags & 1 != 0 { PAUSED } else { 0 }
}

/// [`Request::op`] without a borrow of the request across the effect, which may call
/// back into it: `a` is `an` bytes (or a count, with `a` null) and `bn` a second count.
fn raw_op(
    request: *mut c_void,
    op: u32,
    object: *mut c_void,
    a: *const u8,
    an: usize,
    bn: usize,
) -> u64 {
    // SAFETY: callbacks pass their live request; the copy ends the borrow before the call.
    let binding = unsafe { &(*request.cast::<Request>()).binding };
    let Some((context, native)) = binding.context else {
        std::process::abort();
    };
    // SAFETY: the native call runs one effect on the live `Context`.
    unsafe { native(context.as_ptr(), op, object, a, an, ptr::null(), bn) }
}

unsafe extern "C" fn on_message_complete(parser: *mut Llhttp) -> c_int {
    // SAFETY: llhttp calls back with the parser `parse` made over a bound request.
    let r = unsafe { http(parser) };
    // SAFETY: a live request.
    unsafe { swr_v1_wrappers_complete(r) };
    PAUSED
}

unsafe extern "C" fn on_part_header_field(
    parser: *mut Multipart,
    at: *const c_char,
    length: usize,
) -> c_int {
    // SAFETY: the multipart parser calls back with the parser bound to a request.
    let (_, r) = unsafe { part(parser) };
    // SAFETY: the parser lends the field bytes.
    unsafe { swr_v1_wrappers_header(r, at.cast(), length) }
}

unsafe extern "C" fn on_part_header_value(
    parser: *mut Multipart,
    at: *const c_char,
    length: usize,
) -> c_int {
    // SAFETY: the multipart parser calls back with the parser bound to a request.
    let (_, r) = unsafe { part(parser) };
    // SAFETY: the parser lends the value bytes.
    let value = unsafe { span(at, length) };
    // SAFETY: as above.
    let kind = unsafe { swr_v1_wrappers_header_kind(r, 0, value.as_ptr(), value.len(), 1) };
    if kind == 3 {
        // SAFETY: as above.
        return unsafe { swr_v1_wrappers_disposition(r, value.as_ptr(), value.len()) };
    }
    if kind == 4 {
        // SAFETY: a live request.
        unsafe { swr_v1_wrappers_complete(r) };
        r.op(FILE, ptr::null_mut(), text(r.view().form), value);
    }
    if kind < 0 { -1 } else { 0 }
}

unsafe extern "C" fn on_part_data(
    parser: *mut Multipart,
    at: *const c_char,
    length: usize,
) -> c_int {
    // SAFETY: the multipart parser calls back with the parser bound to a request.
    let (p, r) = unsafe { part(parser) };
    // SAFETY: the parser lends the data bytes.
    let data = unsafe { span(at, length) };
    let form = r.view().form;
    if form.len != 0 {
        // SAFETY: as above.
        return unsafe { swr_v1_wrappers_body_append(r, data.as_ptr(), data.len()) };
    }
    if !p.fp.is_null() && r.op(WRITE, p.fp, data, text(form)) != 0 {
        p.fp = ptr::null_mut();
    }
    0
}

unsafe extern "C" fn on_part_headers_complete(parser: *mut Multipart) -> c_int {
    // SAFETY: the multipart parser calls back with the parser bound to a request.
    let (p, r) = unsafe { part(parser) };
    let view = r.view();
    if view.input.len == 0 || r.op(HAS_FILE, ptr::null_mut(), text(view.form), &[]) != 0 {
        return 0;
    }
    let fp = r.op(OPEN, ptr::null_mut(), text(view.form), &[]) as *mut c_void;
    if !fp.is_null() {
        p.fp = fp;
    }
    0
}

unsafe extern "C" fn on_part_data_end(parser: *mut Multipart) -> c_int {
    // SAFETY: the multipart parser calls back with the parser bound to a request.
    let (p, r) = unsafe { part(parser) };
    let view = r.view();
    if view.form.len != 0 {
        r.op(FORM_DATA, ptr::null_mut(), text(view.form), text(view.body));
    }
    if !p.fp.is_null() {
        r.op(FCLOSE, p.fp, &[], &[]);
        p.fp = ptr::null_mut();
    }
    // SAFETY: a live request.
    unsafe { swr_v1_wrappers_part_end(r) };
    0
}

/// Binds the request to its `Context` for a parse; effects go through `native`.
///
/// # Safety
/// `p` is a live request; `context` outlives the parse.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_wrappers_request_bind(
    p: *mut Request,
    context: *mut c_void,
    native: Native,
) {
    // SAFETY: per the contract.
    if let (Some(r), Some(context)) = (unsafe { p.as_mut() }, NonNull::new(context)) {
        r.binding.context = Some((context, native));
    }
}

/// The llhttp callbacks (`llhttp_settings_t`).
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_wrappers_http_settings() -> *const LlhttpSettings {
    &raw const HTTP
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{swr_v1_wrappers_request_free, swr_v1_wrappers_request_new};
    use std::cell::RefCell;

    /// The effects a parse asked for, with their byte arguments.
    #[derive(Default)]
    struct Context {
        effects: Vec<(u32, Vec<u8>, Vec<u8>)>,
        has_file: bool,
    }

    thread_local! {
        static PARSER: RefCell<Multipart> = const { RefCell::new(Multipart {
            _settings: ptr::null(),
            _index: 0,
            _error_i: 0,
            _boundary_length: 0,
            fp: ptr::null_mut(),
            data: ptr::null_mut(),
        }) };
    }

    unsafe extern "C" fn native(
        context: *mut c_void,
        op: u32,
        _: *mut c_void,
        a: *const u8,
        an: usize,
        b: *const u8,
        bn: usize,
    ) -> u64 {
        let context = unsafe { &mut *context.cast::<Context>() };
        let bytes = |p: *const u8, n: usize| {
            if p.is_null() {
                Vec::new()
            } else {
                unsafe { core::slice::from_raw_parts(p, n) }.to_vec()
            }
        };
        context.effects.push((op, bytes(a, an), bytes(b, bn)));
        match op {
            BOUNDARY => 1,
            MULTIPART_NEW => PARSER.with(|p| p.as_ptr() as u64),
            HAS_FILE => context.has_file.into(),
            OPEN => 0x1000,
            _ => 0,
        }
    }

    fn llhttp(request: *mut Request, method: u8) -> Llhttp {
        Llhttp {
            _index: 0,
            _span_pos0: ptr::null_mut(),
            _span_cb0: ptr::null_mut(),
            _error: 0,
            _reason: ptr::null(),
            _error_pos: ptr::null(),
            data: request.cast(),
            _current: ptr::null_mut(),
            _content_length: 0,
            _kind: 0,
            method,
        }
    }

    fn ops(context: &Context) -> Vec<u32> {
        context.effects.iter().map(|e| e.0).collect()
    }

    fn data(callback: DataCallback<Llhttp>, parser: &mut Llhttp, text: &[u8]) -> c_int {
        unsafe { callback(parser, text.as_ptr().cast(), text.len()) }
    }
    type DataCallback<P> = unsafe extern "C" fn(*mut P, *const c_char, usize) -> c_int;

    fn part(callback: DataCallback<Multipart>, text: &[u8]) -> c_int {
        PARSER.with(|p| unsafe { callback(p.as_ptr(), text.as_ptr().cast(), text.len()) })
    }

    fn notify(callback: unsafe extern "C" fn(*mut Multipart) -> c_int) -> c_int {
        PARSER.with(|p| unsafe { callback(p.as_ptr()) })
    }

    #[test]
    fn a_request_fills_the_context() {
        let mut context = Context::default();
        let request = swr_v1_wrappers_request_new();
        unsafe { swr_v1_wrappers_request_bind(request, (&raw mut context).cast(), native) };
        let mut parser = llhttp(request, 1);
        assert_eq!(data(on_url, &mut parser, b"/a/b?x=1"), 0, "url");
        assert_eq!(
            data(on_url, &mut parser, b"/only?"),
            0,
            "url without a query"
        );
        assert_eq!(data(on_header_field, &mut parser, b"Host"), 0, "field");
        assert_eq!(data(on_header_value, &mut parser, b"example"), 0, "value");
        assert_eq!(
            unsafe { on_headers_complete(&raw mut parser) },
            0,
            "headers complete"
        );
        assert_eq!(data(on_body, &mut parser, b"body"), 0, "body");
        assert_eq!(
            unsafe { on_message_complete(&raw mut parser) },
            PAUSED,
            "pauses at the end"
        );
        assert_eq!(
            data(on_body, &mut parser, b"more"),
            PAUSED,
            "a completed request pauses"
        );
        let expected: [(u32, &[u8], &[u8]); 7] = [
            (PATH, b"/a/b", b""),
            (QUERY, b"x=1", b""),
            (PATH, b"/only", b""),
            (HEADER, b"Host", b"example"),
            (HEADERS_COMPLETE, b"", b""),
            (BODY, b"body", b""),
            (BODY, b"more", b""),
        ];
        let got: Vec<(u32, &[u8], &[u8])> = context
            .effects
            .iter()
            .map(|(op, a, b)| (*op, a.as_slice(), b.as_slice()))
            .collect();
        assert_eq!(got, expected, "effects in order");
        unsafe { swr_v1_wrappers_request_free(request) };
    }

    #[test]
    fn forms_and_uploads_stream_through_the_multipart_parser() {
        let mut context = Context::default();
        let request = swr_v1_wrappers_request_new();
        unsafe { swr_v1_wrappers_request_bind(request, (&raw mut context).cast(), native) };
        let mut parser = llhttp(request, 3);
        data(on_header_field, &mut parser, b"Content-Type");
        assert_eq!(
            data(
                on_header_value,
                &mut parser,
                b"application/x-www-form-urlencoded"
            ),
            0,
            "form"
        );
        assert!(ops(&context).contains(&FORM_URLENCODED), "urlencoded");
        assert_eq!(
            data(
                on_header_value,
                &mut parser,
                b"multipart/form-data; boundary=x"
            ),
            0,
            "multipart"
        );
        assert!(
            ops(&context).ends_with(&[HEADER, BOUNDARY, MULTIPART_NEW]),
            "a parser over the boundary"
        );
        assert_eq!(
            PARSER.with(|p| p.borrow().data),
            request.cast(),
            "bound to the request"
        );
        context.effects.clear();
        // A field part.
        part(on_part_header_field, b"Content-Disposition");
        assert_eq!(
            part(on_part_header_value, b"form-data; name=\"a\""),
            0,
            "disposition"
        );
        part(on_part_data, b"1");
        part(on_part_data, b"2");
        notify(on_part_data_end);
        assert_eq!(
            context.effects,
            [(FORM_DATA, b"\"a\"".to_vec(), b"12".to_vec())],
            "the field"
        );
        context.effects.clear();
        // A file part.
        part(on_part_header_field, b"Content-Disposition");
        part(
            on_part_header_value,
            b"form-data; name=\"f\"; filename=\"x\"",
        );
        notify(on_part_headers_complete);
        assert_eq!(
            PARSER.with(|p| p.borrow().fp) as usize,
            0x1000,
            "its file is open"
        );
        part(on_part_data, b"file");
        notify(on_part_data_end);
        assert_eq!(
            ops(&context),
            [HAS_FILE, OPEN, WRITE, FCLOSE],
            "written to its file"
        );
        assert!(PARSER.with(|p| p.borrow().fp).is_null(), "and closed");
        context.effects.clear();
        // The body goes to the multipart parser.
        assert_eq!(data(on_body, &mut parser, b"\r\n--x"), 0, "body");
        assert_eq!(context.effects[0].0, MULTIPART_EXECUTE, "executed");
        assert_eq!(context.effects[0].1, b"--x", "leading line breaks skipped");
        assert_eq!(context.effects[1].0, LOG_PARSE, "a short parse is logged");
        context.effects.clear();
        unsafe { swr_v1_wrappers_request_free(request) };
        assert_eq!(
            ops(&context),
            [MULTIPART_FREE],
            "the parser goes with the request"
        );
    }
}
