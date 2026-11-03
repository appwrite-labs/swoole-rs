//! `http_server::Request`, the HTTP/1 request a `ListenPort` parses (#931): its layout
//! and every operation over it. Native code keeps the struct's fields, because
//! upstream's own `ListenPort` reads them whenever the ports module is off, but each
//! method is a call into this module, and its `FormData` is a Rust [`Form`]. The
//! effects Rust can't run go through the request's [`Native`] call
//! (`http_server::request_native`): `swoole::String` allocation, the third-party
//! multipart parser, stdio, `swoole_tmpfile` and logs.
use crate::{HttpFrame, HttpHeaderInfo, HttpSpan, case_equal, framing, headers};
use core::ffi::{c_char, c_int, c_void};
use core::ptr;

const SW_OK: i32 = 0;
const SW_ERR: i32 = -1;

/// `swoole::String`'s head.
#[repr(C)]
pub struct HttpString {
    pub length: usize,
    pub size: usize,
    pub offset: i64,
    pub str: *mut u8,
}

impl HttpString {
    /// The bytes before `length`.
    ///
    /// # Safety
    /// `str` holds `length` initialized bytes while the slice lives.
    unsafe fn bytes<'a>(&self) -> &'a [u8] {
        if self.length == 0 || self.str.is_null() {
            return &[];
        }
        // SAFETY: per the contract.
        unsafe { core::slice::from_raw_parts(self.str, self.length) }
    }
}

/// The head of third-party `struct multipart_parser`; native code asserts the layout.
#[repr(C)]
struct Parser {
    settings: *const c_void,
    index: usize,
    error_i: usize,
    boundary_length: u8,
    fp: *mut c_void,
    data: *mut c_void,
    state: u8,
    error_reason: u8,
    boundary: [u8; (2 + 70) * 2 + 9],
}

/// `struct multipart_parser_settings`.
#[repr(C)]
struct ParserSettings {
    on_header_field: DataCallback,
    on_header_value: DataCallback,
    on_part_data: DataCallback,
    on_part_data_begin: NotifyCallback,
    on_headers_complete: NotifyCallback,
    on_part_data_end: NotifyCallback,
    on_body_end: NotifyCallback,
}
type DataCallback = unsafe extern "C" fn(*mut Parser, *const c_char, usize) -> c_int;
type NotifyCallback = unsafe extern "C" fn(*mut Parser) -> c_int;

static SETTINGS: ParserSettings = ParserSettings {
    on_header_field,
    on_header_value,
    on_part_data,
    on_part_data_begin,
    on_headers_complete,
    on_part_data_end,
    on_body_end,
};

/// `op(op, a, b, n, m)`: one native effect (`SWR_HTTP_NATIVE_*`).
pub type Native = unsafe extern "C" fn(u32, *mut c_void, *const u8, u64, u64) -> u64;

// Native effects. Each reads only its arguments.
/// `new String(n)` when `b` is null, else `new String(b, n)`.
const STRING_NEW: u32 = 0;
const STRING_FREE: u32 = 1;
/// `a->append(b, n)`.
const STRING_APPEND: u32 = 2;
/// `multipart_parser_init(b, n, a)`.
const PARSER_NEW: u32 = 3;
/// `multipart_parser_execute(a, b, n)`.
const PARSER_EXECUTE: u32 = 4;
const PARSER_FREE: u32 = 5;
/// `swoole_tmpfile(a)`.
const TMPFILE: u32 = 6;
/// `fdopen(n, "wb+")`.
const FDOPEN: u32 = 7;
/// `fwrite(b, 1, n, a)`, `fflush(a)`, `fclose(a)`, `unlink(a)`.
const FWRITE: u32 = 8;
const FFLUSH: u32 = 9;
const FCLOSE: u32 = 10;
const UNLINK: u32 = 11;
/// The errno `strtoull` leaves: bit 1 `ERANGE`, bit 2 its no-digits value.
const NUMBER_ERRORS: u32 = 12;
/// Upstream's logs: `multipart_parser_init() failed`, `fopen(a) failed`, `failed to
/// write upload file`, the parser's error reason (a) and `n/m bytes processed`.
const LOG_PARSER_NEW: u32 = 13;
const LOG_FOPEN: u32 = 14;
const LOG_WRITE: u32 = 15;
const LOG_PARSE_REASON: u32 = 16;
const LOG_PARSE_PARTIAL: u32 = 17;
/// Trace logs, sent only when the request was made with `trace`.
const TRACE_CHUNK: u32 = 18;
const TRACE_HEADER_FIELD: u32 = 19;
const TRACE_HEADER_VALUE: u32 = 20;
const TRACE_DATA: u32 = 21;
const TRACE_HEADERS_COMPLETE: u32 = 22;
const TRACE_DATA_END: u32 = 23;
const TRACE_PART_BEGIN: u32 = 24;
const TRACE_BODY_END: u32 = 25;
const TRACE_EXECUTE: u32 = 26;

/// `SW_HTTP_UPLOAD_FILE`.
const UPLOAD_FILE: &[u8] = b"Swoole-Upload-File";
/// `SW_BUFFER_SIZE_BIG`.
const BUFFER_SIZE_BIG: u64 = 65536;

/// A multipart body being streamed: upstream's `FormData`.
pub struct Form {
    /// The `Content-Type` value after `multipart/form-data`, in the header buffer.
    boundary: *const u8,
    boundary_length: u32,
    parser: *mut Parser,
    /// The receive buffer, cut back to the header, that the body is rewritten into.
    saved: *mut HttpString,
    /// The current upload's file name (a `swoole::String`).
    upload: *mut HttpString,
    /// `upload_tmp_dir + "/swoole.upfile.XXXXXX"`.
    template: Vec<u8>,
    header_name: *const u8,
    header_name_length: usize,
    upload_size: usize,
    upload_max: usize,
}

/// `http_server::Request`, field for field; native code asserts the layout.
#[repr(C)]
pub struct Request {
    pub method: u8,
    pub version: u8,
    pub excepted: bool,
    pub too_large: bool,
    pub unavailable: bool,
    pub header_parsed: bool,
    pub tried_to_dispatch: bool,
    pub multipart_header_parsed: bool,
    pub known_length: bool,
    pub keep_alive: bool,
    pub chunked: bool,
    pub nobody_chunked: bool,
    pub url_offset: u32,
    pub url_length: u32,
    pub max_length: u32,
    /// Without `\r\n`.
    pub request_line_length: u32,
    /// Including the request line and `\r\n\r\n`.
    pub header_length: u32,
    pub content_length: u64,
    pub form: Option<Box<Form>>,
    pub buffer: *mut HttpString,
    pub native: Native,
}

impl Request {
    /// `Request()`: what native code's constructor makes.
    #[must_use]
    pub fn new(native: Native) -> Self {
        Self {
            method: 0,
            version: 0,
            excepted: false,
            too_large: false,
            unavailable: false,
            header_parsed: false,
            tried_to_dispatch: false,
            multipart_header_parsed: false,
            known_length: false,
            keep_alive: false,
            chunked: false,
            nobody_chunked: false,
            url_offset: 0,
            url_length: 0,
            max_length: 0,
            request_line_length: 0,
            header_length: 0,
            content_length: 0,
            form: None,
            buffer: ptr::null_mut(),
            native,
        }
    }

    fn op(&self, op: u32, a: *mut c_void, b: *const u8, n: u64, m: u64) -> u64 {
        // SAFETY: the native call runs one effect over its arguments.
        unsafe { (self.native)(op, a, b, n, m) }
    }

    /// Native code drops traces unless they are compiled in.
    fn trace(&self, op: u32, b: *const u8, n: u64, m: u64) {
        self.op(op, ptr::null_mut(), b, n, m);
    }

    /// The buffer being parsed.
    fn input<'a>(&self) -> (&'a mut HttpString, &'a [u8]) {
        // SAFETY: a request is parsed only once native code gave it a live buffer,
        // which nothing else touches during the call.
        let buffer = unsafe { &mut *self.buffer };
        // SAFETY: the buffer holds `length` initialized bytes.
        let bytes = unsafe { buffer.bytes() };
        (buffer, bytes)
    }

    /// `memset(&method, 0, offsetof(Request, form_data_))`: the form and buffer stay.
    pub fn clean(&mut self) {
        let (form, buffer) = (self.form.take(), self.buffer);
        *self = Self {
            form,
            buffer,
            ..Self::new(self.native)
        };
    }

    fn frame(&mut self, operation: u32) -> i32 {
        let (buffer, input) = self.input();
        let mut frame = HttpFrame {
            offset: usize::try_from(buffer.offset).unwrap_or(usize::MAX),
            content_length: self.content_length,
            method: self.method.into(),
            version: self.version.into(),
            excepted: self.excepted.into(),
            url_offset: self.url_offset,
            url_length: self.url_length,
            line_length: self.request_line_length,
            header_length: self.header_length,
            known_length: self.known_length.into(),
        };
        let status = frame.parse(input, operation);
        buffer.offset = i64::try_from(frame.offset).unwrap_or(i64::MAX);
        self.content_length = frame.content_length;
        // The native fields are bytes and bits: keep their truncation.
        self.method = frame.method.to_le_bytes()[0];
        self.version = frame.version.to_le_bytes()[0];
        self.excepted = frame.excepted & 1 != 0;
        self.url_offset = frame.url_offset;
        self.url_length = frame.url_length;
        self.request_line_length = frame.line_length;
        self.header_length = frame.header_length;
        self.known_length = frame.known_length & 1 != 0;
        if status == 0 { SW_OK } else { SW_ERR }
    }

    /// The method, URL and version from the request line, or an HTTP/2 preface.
    pub fn get_protocol(&mut self) -> i32 {
        self.frame(0)
    }

    /// Finds the `\r\n\r\n` ending the header block.
    pub fn get_header_length(&mut self) -> i32 {
        self.frame(1)
    }

    /// `Content-Length`, `Connection`, `Transfer-Encoding` and a multipart `Content-Type`.
    pub fn parse_header_info(&mut self) {
        let (_, input) = self.input();
        let mut state = HttpHeaderInfo {
            cursor: self.request_line_length as usize + 2,
            content_length: self.content_length,
            flags: u32::from(self.known_length)
                | u32::from(self.keep_alive) << 1
                | u32::from(self.chunked) << 2
                | u32::from(self.nobody_chunked) << 3
                | u32::from(self.header_parsed) << 4,
            number_errors: 0,
        };
        loop {
            let mut boundary = HttpSpan::default();
            let end = self.header_length.wrapping_sub(4) as usize;
            let status = headers::header_info(input, end, &mut state, &mut boundary);
            self.content_length = state.content_length;
            self.known_length = state.flags & 1 != 0;
            self.keep_alive = state.flags & 2 != 0;
            self.chunked = state.flags & 4 != 0;
            self.nobody_chunked = state.flags & 8 != 0;
            self.header_parsed = state.flags & 16 != 0;
            if state.number_errors != 0 {
                self.op(
                    NUMBER_ERRORS,
                    ptr::null_mut(),
                    ptr::null(),
                    state.number_errors.into(),
                    0,
                );
            }
            if status < 0 {
                self.excepted = true;
                return;
            }
            if status == 0 {
                return;
            }
            self.form = Some(Box::new(Form {
                boundary: input[boundary.offset..].as_ptr(),
                boundary_length: u32::try_from(boundary.length).unwrap_or(u32::MAX),
                parser: ptr::null_mut(),
                saved: ptr::null_mut(),
                upload: ptr::null_mut(),
                template: Vec::new(),
                header_name: ptr::null(),
                header_name_length: 0,
                upload_size: 0,
                upload_max: 0,
            }));
        }
    }

    /// Finds the chunked body's end: `SW_OK` once its zero-length chunk arrived.
    pub fn get_chunked_body_length(&mut self) -> i32 {
        let (buffer, input) = self.input();
        let mut cursor = usize::try_from(buffer.offset).unwrap_or(usize::MAX);
        let mut initial = true;
        loop {
            let (status, step) =
                framing::chunk_step(input, cursor, self.header_length as usize, initial);
            if status < 0 {
                if status == -2 {
                    self.excepted = true;
                }
                return SW_ERR;
            }
            self.trace(
                TRACE_CHUNK,
                input[step.line_offset..].as_ptr(),
                step.line_length as u64,
                step.chunk_length as u64,
            );
            if status == 2 {
                self.known_length = true;
                self.content_length = step.body_length;
                return SW_OK;
            }
            if status == 3 {
                return SW_ERR;
            }
            cursor = step.next;
            initial = false;
        }
    }

    /// An `Expect: 100-continue` header.
    #[must_use]
    pub fn has_expect_header(&self) -> bool {
        let (_, input) = self.input();
        // SAFETY: `input` is readable for its length.
        unsafe { crate::swr_v1_http_expect(input.as_ptr(), input.len()) > 0 }
    }

    /// The value of header `name`, as a span of the buffer.
    #[must_use]
    pub fn get_header(&self, name: &[u8]) -> Option<HttpSpan> {
        let (_, input) = self.input();
        let mut value = HttpSpan::default();
        let start = self.url_offset as usize + self.url_length as usize + 10;
        let found =
            framing::header_value(input, start, self.header_length as usize, name, &mut value);
        (found == 1).then_some(value)
    }

    fn append(&self, string: *mut HttpString, data: *const u8, length: usize) {
        self.op(STRING_APPEND, string.cast(), data, length as u64, 0);
    }
}

/// `Request::init_multipart_parser`: a parser over the form's boundary, the body moved
/// to its own buffer and the receive buffer kept, cut back to the header, to rewrite
/// the body into.
///
/// # Safety
/// `request` is live and has a form; nothing else uses it during the call.
pub unsafe fn init_multipart_parser(
    request: *mut Request,
    tmp_dir: &[u8],
    upload_max: usize,
) -> bool {
    // SAFETY: per the contract.
    let r = unsafe { &mut *request };
    let Some(form) = r.form.as_deref() else {
        return false;
    };
    let mut boundary = HttpSpan::default();
    // SAFETY: the boundary still points into the header buffer it was found in.
    if unsafe {
        crate::swr_v1_http_boundary(
            form.boundary,
            form.boundary_length as usize,
            0,
            &raw mut boundary,
        )
    } != 1
    {
        return false;
    }
    let at = form.boundary.wrapping_add(boundary.offset);
    let parser = r.op(
        PARSER_NEW,
        (&raw const SETTINGS).cast_mut().cast(),
        at,
        boundary.length as u64,
        0,
    ) as *mut Parser;
    let Some(form) = r.form.as_deref_mut() else {
        return false;
    };
    form.parser = parser;
    if parser.is_null() {
        r.op(LOG_PARSER_NEW, ptr::null_mut(), ptr::null(), 0, 0);
        return false;
    }
    // SAFETY: a parser native code just made.
    unsafe { (*parser).data = request.cast() };
    let body =
        r.op(STRING_NEW, ptr::null_mut(), ptr::null(), BUFFER_SIZE_BIG, 0) as *mut HttpString;
    let header = r.header_length as usize;
    let (buffer, input) = r.input();
    let rest = input.get(r.header_length as usize..).unwrap_or_default();
    r.append(body, rest.as_ptr(), rest.len());
    buffer.length = header;
    let saved = r.buffer;
    r.buffer = body;
    let mut template = tmp_dir.to_vec();
    template.extend_from_slice(b"/swoole.upfile.XXXXXX");
    let upload = r.op(
        STRING_NEW,
        ptr::null_mut(),
        template.as_ptr(),
        template.len() as u64,
        0,
    ) as *mut HttpString;
    let Some(form) = r.form.as_deref_mut() else {
        return false;
    };
    form.saved = saved;
    form.template = template;
    form.upload = upload;
    form.upload_max = upload_max;
    true
}

/// `Request::destroy_multipart_parser`: the body buffer freed, the receive buffer back,
/// an unfinished upload closed and unlinked, the parser and form gone.
///
/// # Safety
/// `request` is live; nothing else uses it during the call.
pub unsafe fn destroy_multipart_parser(request: *mut Request) {
    // SAFETY: per the contract.
    let r = unsafe { &mut *request };
    let Some(form) = r.form.take() else {
        return;
    };
    r.op(STRING_FREE, r.buffer.cast(), ptr::null(), 0, 0);
    r.buffer = form.saved;
    // SAFETY: the parser, when there is one, is live until freed below.
    if let Some(parser) = unsafe { form.parser.as_ref() }
        && !parser.fp.is_null()
    {
        r.op(FCLOSE, parser.fp, ptr::null(), 0, 0);
        // SAFETY: the upload's NUL-terminated name.
        r.op(
            UNLINK,
            unsafe { (*form.upload).str }.cast(),
            ptr::null(),
            0,
            0,
        );
    }
    r.op(PARSER_FREE, form.parser.cast(), ptr::null(), 0, 0);
    r.op(STRING_FREE, form.upload.cast(), ptr::null(), 0, 0);
}

/// `Request::parse_multipart_data`: feeds `buffer` to the parser, whose callbacks
/// rewrite the body into the saved buffer. False rejects the request.
///
/// # Safety
/// `request` is live with an initialized form and `buffer` is live; the parser's
/// callbacks are the only other users of either during the call.
pub unsafe fn parse_multipart_data(request: *mut Request, buffer: *mut HttpString) -> bool {
    // SAFETY: per the contract; the borrow ends before the parser runs.
    let r = unsafe { &mut *request };
    r.excepted = false;
    let parser = r
        .form
        .as_deref()
        .map_or(ptr::null_mut(), |form| form.parser);
    let native = r.native;
    // SAFETY: per the contract.
    let input = unsafe { &*buffer };
    let (data, length) = (input.str, input.length);
    // SAFETY: the parser calls back into this request through `data`; no Rust borrow
    // of it is live.
    let processed =
        unsafe { native(PARSER_EXECUTE, parser.cast(), data, length as u64, 0) }.cast_signed();
    // SAFETY: per the contract.
    let r = unsafe { &*request };
    r.trace(
        TRACE_EXECUTE,
        ptr::null(),
        length as u64,
        processed.cast_unsigned(),
    );
    match crate::swr_v1_http_multipart_result(processed, length, r.excepted.into()) {
        outcome if outcome < 0 => {
            r.op(LOG_PARSE_REASON, parser.cast(), ptr::null(), 0, 0);
            false
        }
        2 => {
            // SAFETY: per the contract.
            let input = unsafe { &mut *buffer };
            // `buffer->clear()`.
            (input.length, input.offset) = (0, 0);
            true
        }
        outcome => {
            r.op(
                LOG_PARSE_PARTIAL,
                ptr::null_mut(),
                ptr::null(),
                processed.cast_unsigned(),
                length as u64,
            );
            outcome != 0
        }
    }
}

/// The request and form a parser callback belongs to.
///
/// # Safety
/// `parser` is live with `data` set by [`init_multipart_parser`] to a live request
/// that has a form, and no other borrow of that request is live.
unsafe fn parts<'a>(parser: *mut Parser) -> (&'a mut Parser, &'a mut Request) {
    // SAFETY: per the contract.
    let parser = unsafe { &mut *parser };
    // SAFETY: per the contract.
    let request = unsafe { &mut *parser.data.cast::<Request>() };
    (parser, request)
}

fn form_of(request: &mut Request) -> &mut Form {
    // Callbacks run only from `parse_multipart_data`, on a request with a form.
    request
        .form
        .as_deref_mut()
        .unwrap_or_else(|| std::process::abort())
}

unsafe extern "C" fn on_header_field(
    parser: *mut Parser,
    at: *const c_char,
    length: usize,
) -> c_int {
    // SAFETY: the parser calls back with itself, set up by `init_multipart_parser`.
    let (_, request) = unsafe { parts(parser) };
    let form = form_of(request);
    (form.header_name, form.header_name_length) = (at.cast(), length);
    request.trace(TRACE_HEADER_FIELD, at.cast(), length as u64, 0);
    0
}

unsafe extern "C" fn on_header_value(
    parser: *mut Parser,
    at: *const c_char,
    length: usize,
) -> c_int {
    // SAFETY: the parser calls back with itself, set up by `init_multipart_parser`.
    let (parser, request) = unsafe { parts(parser) };
    request.trace(TRACE_HEADER_VALUE, at.cast(), length as u64, 0);
    let form = form_of(request);
    let (saved, name, name_length) = (form.saved, form.header_name, form.header_name_length);
    request.append(saved, name, name_length);
    request.append(saved, b": ".as_ptr(), 2);
    request.append(saved, at.cast(), length);
    request.append(saved, b"\r\n".as_ptr(), 2);
    // SAFETY: the parser lent the header name it last reported, still in its input.
    let name = unsafe { core::slice::from_raw_parts(name, name_length) };
    if !case_equal(name, b"content-disposition") {
        return 0;
    }
    // `parse_cookie` over a NUL-terminated copy, stopping at the first `filename`.
    // SAFETY: the parser lends `length` bytes at `at`.
    let mut value = unsafe { core::slice::from_raw_parts(at.cast::<u8>(), length) }.to_vec();
    value.push(0);
    // SAFETY: `value` is writable and NUL-terminated.
    if unsafe { crate::swr_v1_http_multipart_filename(value.as_mut_ptr(), value.len()) } != 1 {
        return 0;
    }
    let form = form_of(request);
    // SAFETY: the upload name is a live `swoole::String` while the form is.
    let name = unsafe { (*form.upload).str };
    // The upload name was made from the template, so it has room for it and a NUL.
    let room = form.template.len();
    // SAFETY: as above.
    unsafe { ptr::copy_nonoverlapping(form.template.as_ptr(), name, room) };
    // SAFETY: as above.
    unsafe { name.wrapping_add(room).write(0) };
    form.upload_size = 0;
    let fd = request.op(TMPFILE, name.cast(), ptr::null(), 0, 0);
    // `swoole_tmpfile`'s `int`, sign-extended.
    if fd.cast_signed() < 0 {
        request.excepted = true;
        return 0;
    }
    let fp = request.op(FDOPEN, ptr::null_mut(), ptr::null(), fd, 0) as *mut c_void;
    if fp.is_null() {
        request.op(LOG_FOPEN, name.cast(), ptr::null(), 0, 0);
        return 0;
    }
    parser.fp = fp;
    0
}

unsafe extern "C" fn on_part_data(parser: *mut Parser, at: *const c_char, length: usize) -> c_int {
    // SAFETY: the parser calls back with itself, set up by `init_multipart_parser`.
    let (parser, request) = unsafe { parts(parser) };
    request.trace(TRACE_DATA, ptr::null(), length as u64, 0);
    let max_length = u64::from(request.max_length);
    let form = form_of(request);
    let saved = form.saved;
    if parser.fp.is_null() {
        // SAFETY: the saved buffer is live while the form is.
        if (unsafe { (*saved).length } as u64).wrapping_add(length as u64) > max_length {
            (request.excepted, request.unavailable) = (true, true);
            return 1;
        }
        request.append(saved, at.cast(), length);
        return 0;
    }
    form.upload_size = form.upload_size.wrapping_add(length);
    if form.upload_size > form.upload_max {
        (request.excepted, request.too_large) = (true, true);
        return 1;
    }
    let written = request.op(FWRITE, parser.fp, at.cast(), length as u64, 0);
    if written != length as u64 {
        request.op(FCLOSE, parser.fp, ptr::null(), 0, 0);
        parser.fp = ptr::null_mut();
        (request.excepted, request.unavailable) = (true, true);
        request.op(LOG_WRITE, ptr::null_mut(), ptr::null(), 0, 0);
        return 1;
    }
    0
}

unsafe extern "C" fn on_headers_complete(parser: *mut Parser) -> c_int {
    // SAFETY: the parser calls back with itself, set up by `init_multipart_parser`.
    let (parser, request) = unsafe { parts(parser) };
    request.trace(TRACE_HEADERS_COMPLETE, ptr::null(), 0, 0);
    let form = form_of(request);
    let (saved, upload) = (form.saved, form.upload);
    if !parser.fp.is_null() {
        request.append(
            saved,
            b"Swoole-Upload-File: ".as_ptr(),
            UPLOAD_FILE.len() + 2,
        );
        // SAFETY: the upload name is a live `swoole::String` while the form is.
        let name = unsafe { (*upload).str };
        // SAFETY: its name is NUL-terminated.
        let name = unsafe { core::ffi::CStr::from_ptr(name.cast()) };
        request.append(saved, name.as_ptr().cast(), name.count_bytes());
    }
    request.multipart_header_parsed = true;
    request.append(saved, b"\r\n".as_ptr(), 2);
    0
}

unsafe extern "C" fn on_part_data_end(parser: *mut Parser) -> c_int {
    // SAFETY: the parser calls back with itself, set up by `init_multipart_parser`.
    let (parser, request) = unsafe { parts(parser) };
    request.trace(TRACE_DATA_END, ptr::null(), 0, 0);
    request.multipart_header_parsed = false;
    let saved = form_of(request).saved;
    if !parser.fp.is_null() {
        request.append(
            saved,
            b"\r\nSwoole-Upload-File".as_ptr(),
            UPLOAD_FILE.len() + 2,
        );
        request.op(FFLUSH, parser.fp, ptr::null(), 0, 0);
        request.op(FCLOSE, parser.fp, ptr::null(), 0, 0);
        parser.fp = ptr::null_mut();
    }
    request.append(saved, b"\r\n".as_ptr(), 2);
    0
}

unsafe extern "C" fn on_part_data_begin(parser: *mut Parser) -> c_int {
    // SAFETY: the parser calls back with itself, set up by `init_multipart_parser`.
    let (parser, request) = unsafe { parts(parser) };
    request.trace(TRACE_PART_BEGIN, ptr::null(), 0, 0);
    let saved = form_of(request).saved;
    request.append(
        saved,
        parser.boundary.as_ptr(),
        parser.boundary_length.into(),
    );
    request.append(saved, b"\r\n".as_ptr(), 2);
    0
}

unsafe extern "C" fn on_body_end(parser: *mut Parser) -> c_int {
    // SAFETY: the parser calls back with itself, set up by `init_multipart_parser`.
    let (parser, request) = unsafe { parts(parser) };
    let saved = form_of(request).saved;
    request.append(
        saved,
        parser.boundary.as_ptr(),
        parser.boundary_length.into(),
    );
    request.append(saved, b"--".as_ptr(), 2);
    // SAFETY: the saved buffer is live while the form is.
    let saved = unsafe { &*saved };
    request.content_length = (saved.length as u64).wrapping_sub(u64::from(request.header_length));
    request.tried_to_dispatch = true;
    request.trace(TRACE_BODY_END, saved.str, saved.length as u64, 0);
    0
}

/// `Request::get_protocol`.
///
/// # Safety
/// `request` and its buffer are live, and nothing else uses them during the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_http_request_protocol(request: *mut Request) -> i32 {
    // SAFETY: per the contract.
    unsafe { &mut *request }.get_protocol()
}

/// `Request::get_header_length`.
///
/// # Safety
/// As [`swr_v1_http_request_protocol`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_http_request_header_length(request: *mut Request) -> i32 {
    // SAFETY: per the contract.
    unsafe { &mut *request }.get_header_length()
}

/// `Request::get_chunked_body_length`.
///
/// # Safety
/// As [`swr_v1_http_request_protocol`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_http_request_chunked_length(request: *mut Request) -> i32 {
    // SAFETY: per the contract.
    unsafe { &mut *request }.get_chunked_body_length()
}

/// `Request::parse_header_info`.
///
/// # Safety
/// As [`swr_v1_http_request_protocol`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_http_request_parse_header(request: *mut Request) {
    // SAFETY: per the contract.
    unsafe { &mut *request }.parse_header_info();
}

/// `Request::has_expect_header`.
///
/// # Safety
/// As [`swr_v1_http_request_protocol`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_http_request_expect(request: *const Request) -> u32 {
    // SAFETY: per the contract.
    unsafe { &*request }.has_expect_header().into()
}

/// `Request::get_header`: 1 with the value's span in the buffer, else 0.
///
/// # Safety
/// As [`swr_v1_http_request_protocol`]; `name` holds `length` bytes; `value` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_http_request_header(
    request: *const Request,
    name: *const u8,
    length: usize,
    value: *mut HttpSpan,
) -> i32 {
    // SAFETY: per the contract.
    let r = unsafe { &*request };
    // SAFETY: per the contract.
    let name = unsafe { core::slice::from_raw_parts(name, length) };
    let Some(span) = r.get_header(name) else {
        return 0;
    };
    // SAFETY: per the contract.
    unsafe { value.write(span) };
    1
}

/// `Request::init_multipart_parser(server)`, given its upload directory and limit.
///
/// # Safety
/// As [`init_multipart_parser`]; `dir` holds `length` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_http_request_multipart_init(
    request: *mut Request,
    dir: *const u8,
    length: usize,
    upload_max: usize,
) -> u32 {
    // SAFETY: per the contract.
    let dir = unsafe { core::slice::from_raw_parts(dir, length) };
    // SAFETY: per the contract.
    unsafe { init_multipart_parser(request, dir, upload_max) }.into()
}

/// `Request::parse_multipart_data(buffer)`.
///
/// # Safety
/// As [`parse_multipart_data`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_http_request_multipart_parse(
    request: *mut Request,
    buffer: *mut HttpString,
) -> u32 {
    // SAFETY: per the contract.
    unsafe { parse_multipart_data(request, buffer) }.into()
}

/// `Request::destroy_multipart_parser`, and `~Request` (which runs it for a form).
///
/// # Safety
/// As [`destroy_multipart_parser`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_http_request_multipart_destroy(request: *mut Request) {
    // SAFETY: per the contract.
    unsafe { destroy_multipart_parser(request) };
}

/// `delete form_data_` of a form no parser was made for: `FormData::operator delete`.
///
/// # Safety
/// `form` is null or a request's form, not used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_http_form_free(form: *mut Form) {
    if !form.is_null() {
        // SAFETY: per the contract: forms are boxed.
        drop(unsafe { Box::from_raw(form) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    /// A `swoole::String` stand-in: the head and the bytes it points at.
    #[repr(C)]
    struct Text {
        head: HttpString,
        bytes: Vec<u8>,
    }

    impl Text {
        fn sync(&mut self) {
            self.head.str = self.bytes.as_mut_ptr();
            self.head.size = self.bytes.capacity();
        }
    }

    /// The native effects a test saw, the strings it made and what it wrote to files.
    #[derive(Default)]
    struct Host {
        effects: Vec<u32>,
        strings: Vec<*mut Text>,
        written: Vec<u8>,
    }

    thread_local! {
        static HOST: RefCell<Host> = RefCell::new(Host::default());
        static PARSER: Cell<*mut Parser> = const { Cell::new(ptr::null_mut()) };
    }

    fn size(n: u64) -> usize {
        usize::try_from(n).unwrap()
    }

    unsafe extern "C" fn native(op: u32, a: *mut c_void, b: *const u8, n: u64, _: u64) -> u64 {
        HOST.with_borrow_mut(|host| {
            host.effects.push(op);
            match op {
                STRING_NEW => {
                    let (length, mut bytes) = if b.is_null() {
                        (0, Vec::with_capacity(size(n)))
                    } else {
                        (
                            size(n),
                            unsafe { core::slice::from_raw_parts(b, size(n)) }.to_vec(),
                        )
                    };
                    bytes.push(0);
                    bytes.truncate(length);
                    let text = Box::into_raw(Box::new(Text {
                        head: HttpString {
                            length,
                            size: 0,
                            offset: 0,
                            str: ptr::null_mut(),
                        },
                        bytes,
                    }));
                    unsafe { (*text).sync() };
                    host.strings.push(text);
                    text as u64
                }
                STRING_APPEND => {
                    let data = unsafe { core::slice::from_raw_parts(b, size(n)) };
                    if host.strings.contains(&a.cast()) {
                        let text = unsafe { &mut *a.cast::<Text>() };
                        text.bytes.truncate(text.head.length);
                        text.bytes.extend_from_slice(data);
                        text.head.length = text.bytes.len();
                        text.sync();
                    } else {
                        // A test's receive buffer, which has room.
                        let buffer = unsafe { &mut *a.cast::<HttpString>() };
                        unsafe {
                            ptr::copy_nonoverlapping(
                                b,
                                buffer.str.wrapping_add(buffer.length),
                                data.len(),
                            );
                        }
                        buffer.length += data.len();
                    }
                    0
                }
                PARSER_NEW => PARSER.get() as u64,
                FWRITE => {
                    host.written
                        .extend_from_slice(unsafe { core::slice::from_raw_parts(b, size(n)) });
                    n
                }
                TMPFILE => 7,
                FDOPEN => 0x1000,
                _ => 0,
            }
        })
    }

    fn effects() -> Vec<u32> {
        HOST.with_borrow_mut(|host| core::mem::take(&mut host.effects))
    }

    /// A receive buffer holding `text`, with room to grow.
    struct Input {
        bytes: Vec<u8>,
        buffer: HttpString,
    }

    #[expect(clippy::unnecessary_box_returns, reason = "requests point into it")]
    fn input(text: &[u8]) -> Box<Input> {
        let mut bytes = text.to_vec();
        bytes.resize(text.len() + 4096, 0);
        let size = bytes.len();
        let mut input = Box::new(Input {
            buffer: HttpString {
                length: text.len(),
                size,
                offset: 0,
                str: ptr::null_mut(),
            },
            bytes,
        });
        input.buffer.str = input.bytes.as_mut_ptr();
        input
    }

    fn request(input: &mut Input) -> Request {
        let mut request = Request::new(native);
        request.buffer = &raw mut input.buffer;
        request
    }

    fn url(request: &Request, input: &Input) -> Vec<u8> {
        let at = request.url_offset as usize;
        input.bytes[at..at + request.url_length as usize].to_vec()
    }

    fn parsed(text: &[u8]) -> (Box<Input>, Request) {
        let mut i = input(text);
        let mut r = request(&mut i);
        assert_eq!(r.get_protocol(), SW_OK, "request line");
        assert_eq!(r.get_header_length(), SW_OK, "header block");
        r.parse_header_info();
        (i, r)
    }

    fn parser(boundary: &[u8], data: *mut c_void) -> Parser {
        let mut parser = Parser {
            settings: ptr::null(),
            index: 0,
            error_i: 0,
            boundary_length: u8::try_from(boundary.len()).unwrap(),
            fp: ptr::null_mut(),
            data,
            state: 0,
            error_reason: 0,
            boundary: [0; 153],
        };
        parser.boundary[..boundary.len()].copy_from_slice(boundary);
        parser
    }

    #[test]
    fn layouts_match_native_static_asserts() {
        use core::mem::{offset_of, size_of};
        let request = [
            offset_of!(Request, url_offset),
            offset_of!(Request, content_length),
            offset_of!(Request, form),
        ];
        assert_eq!(request, [12, 32, 40], "Request fields");
        let tail = [
            offset_of!(Request, buffer),
            offset_of!(Request, native),
            size_of::<Request>(),
        ];
        assert_eq!(tail, [48, 56, 64], "Request tail");
        let parser = [
            offset_of!(Parser, boundary_length),
            offset_of!(Parser, fp),
            offset_of!(Parser, data),
            offset_of!(Parser, boundary),
        ];
        assert_eq!(parser, [24, 32, 40, 50], "multipart_parser head");
        assert_eq!(
            size_of::<ParserSettings>(),
            7 * size_of::<usize>(),
            "seven callbacks"
        );
    }

    // core-tests/src/server/http_server.cpp: get_protocol.
    #[test]
    fn get_protocol_reads_the_request_line() {
        let mut i = input(b"GET /index.html HTTP/1.1\r\nHost: example.com\r\n\r\n");
        let mut r = request(&mut i);
        assert_eq!(r.get_protocol(), SW_OK, "GET");
        assert_eq!((r.method, r.version), (2, 2), "GET, HTTP/1.1");
        assert_eq!(url(&r, &i), b"/index.html", "its URL");

        let mut i = input(
            b"POST /submit HTTP/1.0\r\nContent-Type: application/json\r\n\r\n{\"key\":\"value\"}",
        );
        let mut r = request(&mut i);
        assert_eq!(r.get_protocol(), SW_OK, "POST");
        assert_eq!((r.method, r.version), (4, 1), "POST, HTTP/1.0");
        assert_eq!(url(&r, &i), b"/submit", "its URL");

        let mut i = input(b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n");
        let mut r = request(&mut i);
        assert_eq!(r.get_protocol(), SW_OK, "preface");
        assert_eq!(
            (r.method, i.buffer.offset),
            (27, 24),
            "HTTP/2 preface consumed"
        );

        let mut i = input(b"BREW / HTTP/1.1\r\n\r\n");
        let mut r = request(&mut i);
        assert_eq!(r.get_protocol(), SW_ERR, "unknown method");
        assert!(r.excepted, "unknown method is an error");

        let mut i = input(b"GET /");
        let mut r = request(&mut i);
        assert_eq!(r.get_protocol(), SW_ERR, "incomplete");
        assert!(!r.excepted, "incomplete: wait for more");
    }

    // core-tests/src/server/http_server.cpp: has_expect_header.
    #[test]
    fn has_expect_header() {
        let mut i = input(b"HTTP/1.1 200 OK\r\nCache-Control: no-cache\r\nConnection: close\r\nContent-Length: 0\r\nContent-Type: text/html\r\nPragma: no-cache\r\n\r\n");
        assert!(!request(&mut i).has_expect_header(), "no Expect");
        let mut i = input(b"POST /submit HTTP/1.1\r\nHost: www.example.com\r\nContent-Type: application/json\r\nContent-Length: 1000000\r\nExpect: 100-continue\r\n\r\n");
        assert!(request(&mut i).has_expect_header(), "Expect: 100-continue");
    }

    #[test]
    fn header_info_and_values() {
        let text = b"POST /x HTTP/1.1\r\nHost: a\r\nConnection: keep-alive\r\nContent-Length: 12\r\n\r\nbody";
        let (i, r) = parsed(text);
        assert_eq!(r.header_length as usize, text.len() - 4, "header length");
        assert!(
            r.header_parsed && r.keep_alive && r.known_length && !r.chunked,
            "flags"
        );
        assert_eq!(r.content_length, 12, "Content-Length");
        let host = r.get_header(b"Host").unwrap();
        assert_eq!(
            &i.bytes[host.offset..host.offset + host.length],
            b"a",
            "Host"
        );
        assert!(r.get_header(b"Missing").is_none(), "absent header");

        let (_, r) =
            parsed(b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\nContent-Length: 0\r\n\r\n");
        assert!(r.chunked && r.nobody_chunked, "chunked with no body");
    }

    #[test]
    fn chunked_bodies_end_at_the_zero_chunk() {
        let head = b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n";
        let offset = i64::try_from(head.len()).unwrap();
        let (mut i, mut r) = parsed(&[&head[..], b"3\r\nabc\r\n0\r\n\r\n"].concat());
        i.buffer.offset = offset;
        assert_eq!(r.get_chunked_body_length(), SW_OK, "complete");
        assert!(r.known_length, "length known");
        assert_eq!(
            r.content_length, 13,
            "upstream: up to the zero chunk's line, plus 4"
        );

        let (mut i, mut r) = parsed(&[&head[..], b"q\r\n0\r\n\r\n"].concat());
        i.buffer.offset = offset;
        assert_eq!(r.get_chunked_body_length(), SW_ERR, "bad chunk");
        assert!(r.excepted, "a bad chunk length is an error");
    }

    #[test]
    fn clean_keeps_form_and_buffer() {
        let (_, mut r) = parsed(b"POST / HTTP/1.1\r\nContent-Type: multipart/form-data; boundary=x\r\nContent-Length: 9\r\n\r\n");
        assert!(r.form.is_some() && r.known_length, "a form");
        let buffer = r.buffer;
        r.clean();
        assert!(
            r.form.is_some() && r.buffer == buffer,
            "form and buffer kept"
        );
        assert!(
            !r.known_length && r.content_length == 0 && r.method == 0,
            "facts reset"
        );
        r.form = None;
    }

    /// Streams a two-part body, one part a file, through the callbacks.
    #[test]
    fn multipart_bodies_are_rewritten_with_uploads_on_disk() {
        let head = b"POST / HTTP/1.1\r\nContent-Type: multipart/form-data; boundary=AB\r\nContent-Length: 999\r\n\r\n";
        let (i, mut r) = parsed(head);
        r.max_length = 1 << 20;
        let mut stand_in = parser(b"--AB", ptr::null_mut());
        let at = &raw mut stand_in;
        PARSER.set(at);
        effects();
        let request = &raw mut r;
        assert!(
            unsafe { init_multipart_parser(request, b"/tmp", 100) },
            "initialized"
        );
        assert_eq!(stand_in.data, request.cast(), "callbacks find the request");
        assert_eq!(
            i.buffer.length,
            head.len(),
            "the receive buffer keeps the header"
        );
        let field = |name: &[u8], value: &[u8]| unsafe {
            on_header_field(at, name.as_ptr().cast(), name.len());
            on_header_value(at, value.as_ptr().cast(), value.len());
        };
        unsafe {
            on_part_data_begin(at);
            field(b"Content-Disposition", b"form-data; name=\"a\"");
            on_headers_complete(at);
            on_part_data(at, b"1".as_ptr().cast(), 1);
            on_part_data_end(at);
            on_part_data_begin(at);
            field(
                b"Content-Disposition",
                b"form-data; name=\"f\"; filename=\"x.txt\"",
            );
            on_headers_complete(at);
            on_part_data(at, b"file".as_ptr().cast(), 4);
            on_part_data_end(at);
            on_body_end(at);
        }
        let body = &i.bytes[head.len()..i.buffer.length];
        let expected = [
            &b"--AB\r\nContent-Disposition: form-data; name=\"a\"\r\n\r\n1\r\n"[..],
            b"--AB\r\nContent-Disposition: form-data; name=\"f\"; filename=\"x.txt\"\r\n",
            b"Swoole-Upload-File: /tmp/swoole.upfile.XXXXXX\r\n",
            b"\r\nSwoole-Upload-File\r\n--AB--",
        ]
        .concat();
        assert_eq!(
            String::from_utf8_lossy(body),
            String::from_utf8_lossy(&expected),
            "rewritten body"
        );
        assert_eq!(
            HOST.with_borrow(|h| h.written.clone()),
            b"file",
            "the upload went to its file"
        );
        assert_eq!(r.content_length, expected.len() as u64, "body length");
        assert!(
            r.tried_to_dispatch && !r.multipart_header_parsed,
            "ready to dispatch"
        );
        let seen = effects();
        assert!(
            seen.contains(&TMPFILE) && seen.contains(&FFLUSH) && seen.contains(&FCLOSE),
            "file effects"
        );
        unsafe { destroy_multipart_parser(request) };
        assert!(
            r.form.is_none() && r.buffer == (&raw const i.buffer).cast_mut(),
            "the receive buffer is back"
        );
        assert_eq!(
            effects(),
            [STRING_FREE, PARSER_FREE, STRING_FREE],
            "body, parser and upload name freed"
        );
    }

    #[test]
    fn oversize_parts_and_uploads_are_rejected() {
        let (_, mut r) =
            parsed(b"POST / HTTP/1.1\r\nContent-Type: multipart/form-data; boundary=AB\r\n\r\n");
        let mut stand_in = parser(b"", (&raw mut r).cast());
        let mut saved = HttpString {
            length: 10,
            size: 0,
            offset: 0,
            str: ptr::null_mut(),
        };
        if let Some(form) = r.form.as_deref_mut() {
            (form.saved, form.upload_max) = (&raw mut saved, 3);
        }
        r.max_length = 12;
        assert_eq!(
            unsafe { on_part_data(&raw mut stand_in, b"abc".as_ptr().cast(), 3) },
            1,
            "rejected"
        );
        assert!(
            r.excepted && r.unavailable && !r.too_large,
            "a field past max_length"
        );
        (r.excepted, r.unavailable) = (false, false);
        stand_in.fp = ptr::without_provenance_mut(0x1000);
        assert_eq!(
            unsafe { on_part_data(&raw mut stand_in, b"abcd".as_ptr().cast(), 4) },
            1,
            "rejected"
        );
        assert!(
            r.excepted && r.too_large,
            "an upload past upload_max_filesize"
        );
        r.form = None;
    }
}
