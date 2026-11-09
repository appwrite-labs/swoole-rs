//! `http_server::Request`, the HTTP/1 request a `ListenPort` parses (#931): its layout
//! and every operation over it. Native code keeps the struct's fields, because
//! upstream's own `ListenPort` reads them whenever the ports module is off, but each
//! method is a call into this module, and its `FormData` is a Rust [`HttpForm`]. Its
//! effects go through the request's [`HttpNative`] call, `effects` outside tests:
//! `swoole::String` allocation, the third-party multipart parser, stdio,
//! `swoole_tmpfile` and logs. Trace logs are compiled out, as in upstream's builds
//! without `--enable-trace-log`.
use core::ffi::{c_char, c_int, c_void};
use core::ptr;
use swoole_rs_buffer::string::{CppString, delete_string, new_sized_string, new_string};
use swoole_rs_corebase::{log, set_errno};
use swoole_rs_http::{
    CHUNK_SIZE, HttpChunked, HttpChunks, HttpFrame, HttpHeaderInfo, HttpSpan, case_equal,
};
use swoole_rs_ossupport::swoole_tmpfile;

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

/// The head of third-party `struct multipart_parser` (checked against the real parser in
/// `parser_mirrors_the_bundled_multipart_parser`).
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
pub type HttpNative = unsafe extern "C" fn(u32, *mut c_void, *const u8, u64, u64) -> u64;

// HttpNative effects. Each reads only its arguments.
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
/// A client sent the reserved `Swoole-Upload-File` header.
const LOG_RESERVED: u32 = 18;

// The third-party multipart parser (thirdparty/multipart_parser.c), linked into the
// extension beside this crate.
unsafe extern "C" {
    fn multipart_parser_init(
        boundary: *const c_char,
        length: usize,
        settings: *const ParserSettings,
    ) -> *mut Parser;
    fn multipart_parser_execute(parser: *mut Parser, data: *const c_char, length: usize) -> isize;
    fn multipart_parser_free(parser: *mut Parser);
    fn multipart_parser_error_msg(parser: *mut Parser, buffer: *mut c_char, length: usize)
    -> c_int;
}

/// `SW_LOG_NOTICE`, `SW_LOG_WARNING` and `SW_ERROR_SERVER_INVALID_REQUEST`.
const NOTICE: c_int = 3;
const WARNING: c_int = 4;
const INVALID_REQUEST: c_int = 9010;

fn errno() -> c_int {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// The errno libc's `strtoull` leaves when no digits convert; libcs differ.
fn no_digits_errno() -> c_int {
    static VALUE: std::sync::OnceLock<c_int> = std::sync::OnceLock::new();
    *VALUE.get_or_init(|| {
        let saved = errno();
        set_errno(0);
        let mut end = ptr::null_mut();
        // SAFETY: a NUL-terminated literal and a writable end pointer.
        unsafe { libc::strtoull(c"x".as_ptr(), &raw mut end, 10) };
        let value = errno();
        set_errno(saved);
        value
    })
}

/// The request's effects outside tests: each `op` is a direct call. Effects with no
/// result answer 0.
///
/// # Safety
/// The arguments are what the `op`'s constant documents, each live for the call.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "lengths, descriptors and ssize_t results native code carried as u64"
)]
unsafe extern "C" fn effects(op: u32, a: *mut c_void, b: *const u8, n: u64, m: u64) -> u64 {
    let length = n as usize;
    let parser = a.cast::<Parser>();
    let file = a.cast::<libc::FILE>();
    match op {
        STRING_NEW if b.is_null() => {
            // SAFETY: a null allocator is the default one.
            return unsafe { new_sized_string(length, ptr::null()) as u64 };
        }
        // SAFETY: `b` holds `n` bytes.
        STRING_NEW => return unsafe { new_string(b, length) as u64 },
        // SAFETY: a string from `STRING_NEW`, not used again.
        STRING_FREE => unsafe { delete_string(a.cast()) },
        STRING_APPEND => {
            // SAFETY: a live `swoole::String`.
            let string = unsafe { &mut *a.cast::<CppString>() };
            // SAFETY: `n` readable bytes at `b`.
            unsafe { string.append(b, length) };
        }
        // SAFETY: the boundary's `n` bytes and the parser's settings.
        PARSER_NEW => return unsafe { multipart_parser_init(b.cast(), length, a.cast()) as u64 },
        PARSER_EXECUTE => {
            // SAFETY: a live parser and `n` bytes at `b`.
            return unsafe { multipart_parser_execute(parser, b.cast(), length) as u64 };
        }
        // SAFETY: a parser from `PARSER_NEW`, or null, not used again.
        PARSER_FREE => unsafe { multipart_parser_free(parser) },
        // SAFETY: a writable, NUL-terminated `XXXXXX` template.
        TMPFILE => return i64::from(unsafe { swoole_tmpfile(a.cast()) }).cast_unsigned(),
        // SAFETY: a descriptor and a NUL-terminated mode.
        FDOPEN => return unsafe { libc::fdopen(n as c_int, c"wb+".as_ptr()) as u64 },
        // SAFETY: a live stream and `n` bytes at `b`.
        FWRITE => return unsafe { libc::fwrite(b.cast(), 1, length, file) as u64 },
        FFLUSH => {
            // SAFETY: a live stream.
            unsafe { libc::fflush(file) };
        }
        FCLOSE => {
            // SAFETY: a live stream, not used again.
            unsafe { libc::fclose(file) };
        }
        UNLINK => {
            // SAFETY: a NUL-terminated path.
            unsafe { libc::unlink(a.cast()) };
        }
        NUMBER_ERRORS => {
            if n & 1 != 0 {
                set_errno(libc::ERANGE);
            }
            if n & 2 != 0 && no_digits_errno() != 0 {
                set_errno(no_digits_errno());
            }
        }
        LOG_PARSER_NEW => log::warning(
            c"Request::init_multipart_parser",
            format_args!("multipart_parser_init() failed"),
        ),
        LOG_FOPEN => {
            // SAFETY: the upload's NUL-terminated name.
            let name = unsafe { core::ffi::CStr::from_ptr(a.cast()) }.to_string_lossy();
            log::sys_warning(
                c"http_server::multipart_on_header_value",
                format_args!("fopen({name}) failed"),
            );
        }
        LOG_WRITE => log::sys_warning(
            c"http_server::multipart_on_data",
            format_args!("failed to write upload file"),
        ),
        LOG_PARSE_REASON => {
            let mut text = [0u8; 256];
            // SAFETY: a live parser and a writable buffer of that size.
            let written =
                unsafe { multipart_parser_error_msg(parser, text.as_mut_ptr().cast(), text.len()) };
            // `%.*s`: at most `written` bytes, up to the NUL.
            let text = text.split(|&byte| byte == 0).next().unwrap_or_default();
            let text = text
                .get(..usize::try_from(written).unwrap_or(0))
                .unwrap_or(text);
            log::error_log(
                NOTICE,
                INVALID_REQUEST,
                c"Request::parse_multipart_data",
                format_args!(
                    "parse multipart body failed, reason: {}",
                    String::from_utf8_lossy(text)
                ),
            );
        }
        LOG_RESERVED => log::error_log(
            WARNING,
            INVALID_REQUEST,
            c"http_server::multipart_on_header_value",
            format_args!(
                "Bad Request: the reserved header 'Swoole-Upload-File' may not be sent by clients"
            ),
        ),
        LOG_PARSE_PARTIAL => log::error_log(
            NOTICE,
            INVALID_REQUEST,
            c"Request::parse_multipart_data",
            format_args!("parse multipart body failed, {n}/{m} bytes processed"),
        ),
        _ => std::process::abort(),
    }
    0
}

/// The multipart parser's `MPPE_PAUSED` and `MPPE_ERROR` callback results.
const MPPE_PAUSED: c_int = 1;
const MPPE_ERROR: c_int = -1;
/// `SW_HTTP_UPLOAD_FILE`.
const UPLOAD_FILE: &[u8] = b"Swoole-Upload-File";
/// `SW_BUFFER_SIZE_BIG`.
const BUFFER_SIZE_BIG: u64 = 65536;

/// A multipart body being streamed: upstream's `FormData`.
pub struct HttpForm {
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
    /// The part header's name, copied: it may arrive split across reads.
    header_name: Vec<u8>,
    upload_size: usize,
    upload_max: usize,
}

impl HttpForm {
    /// The `Content-Type` value after `multipart/form-data`, in the header buffer.
    #[must_use]
    pub fn boundary(&self) -> (*const u8, u32) {
        (self.boundary, self.boundary_length)
    }
}

/// `http_server::Request`, field for field; native code asserts the layout.
#[repr(C)]
pub struct HttpRequest {
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
    /// `CHUNK_SIZE`, `CHUNK_DATA` or `CHUNK_TRAILER`: what the chunked body expects next.
    pub chunk_state: u8,
    pub url_offset: u32,
    pub url_length: u32,
    pub max_length: u32,
    /// Without `\r\n`.
    pub request_line_length: u32,
    /// Including the request line and `\r\n\r\n`.
    pub header_length: u32,
    pub content_length: u64,
    pub form: Option<Box<HttpForm>>,
    pub buffer: *mut HttpString,
    pub native: HttpNative,
    /// The current chunk's size, where its line or data starts (0 until the body
    /// is first scanned), and how far its size line has been scanned.
    pub chunk_length: usize,
    pub chunk_offset: usize,
    pub chunk_scan_offset: usize,
}

impl HttpRequest {
    /// `HttpRequest()`: what native code's constructor makes.
    #[must_use]
    pub fn new(native: HttpNative) -> Self {
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
            chunk_state: CHUNK_SIZE,
            url_offset: 0,
            url_length: 0,
            max_length: 0,
            request_line_length: 0,
            header_length: 0,
            content_length: 0,
            form: None,
            buffer: ptr::null_mut(),
            native,
            chunk_length: 0,
            chunk_offset: 0,
            chunk_scan_offset: 0,
        }
    }

    fn op(&self, op: u32, a: *mut c_void, b: *const u8, n: u64, m: u64) -> u64 {
        // SAFETY: the native call runs one effect over its arguments.
        unsafe { (self.native)(op, a, b, n, m) }
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

    /// `memset(&method, 0, offsetof(HttpRequest, form_data_))`: the form and buffer stay.
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
                | u32::from(self.header_parsed) << 4,
            number_errors: 0,
        };
        loop {
            let mut boundary = HttpSpan::default();
            let end = self.header_length.wrapping_sub(4) as usize;
            let status = swoole_rs_http::header_info(input, end, &mut state, &mut boundary);
            self.content_length = state.content_length;
            self.known_length = state.flags & 1 != 0;
            self.keep_alive = state.flags & 2 != 0;
            self.chunked = state.flags & 4 != 0;
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
            self.form = Some(Box::new(HttpForm {
                boundary: input.as_ptr().wrapping_add(boundary.offset),
                boundary_length: u32::try_from(boundary.length).unwrap_or(u32::MAX),
                parser: ptr::null_mut(),
                saved: ptr::null_mut(),
                upload: ptr::null_mut(),
                template: Vec::new(),
                header_name: Vec::new(),
                upload_size: 0,
                upload_max: 0,
            }));
        }
    }

    /// Scans the chunked body from where the last call stopped: `SW_OK` once its
    /// trailer section ended, with `content_length` up to there; `SW_ERR` to wait for
    /// more, or with `excepted` (bad framing) or `too_large` (past `max_length`) set.
    pub fn get_chunked_body_length(&mut self) -> i32 {
        let (_, input) = self.input();
        let mut chunks = HttpChunks {
            state: self.chunk_state,
            length: self.chunk_length,
            offset: self.chunk_offset,
            scan_offset: self.chunk_scan_offset,
        };
        let outcome = chunks.scan(input, self.header_length as usize, self.max_length as usize);
        self.chunk_state = chunks.state;
        self.chunk_length = chunks.length;
        self.chunk_offset = chunks.offset;
        self.chunk_scan_offset = chunks.scan_offset;
        match outcome {
            HttpChunked::Complete(length) => {
                self.known_length = true;
                self.content_length = length;
                return SW_OK;
            }
            HttpChunked::Partial => {}
            HttpChunked::Malformed => self.excepted = true,
            HttpChunked::TooLarge => self.too_large = true,
        }
        SW_ERR
    }

    /// An `Expect: 100-continue` header.
    #[must_use]
    pub fn has_expect_header(&self) -> bool {
        let (_, input) = self.input();
        swoole_rs_http::has_expect_header(input)
    }

    /// The value of header `name`, as a span of the buffer.
    #[must_use]
    pub fn get_header(&self, name: &[u8]) -> Option<HttpSpan> {
        let (_, input) = self.input();
        let mut value = HttpSpan::default();
        let start = self.url_offset as usize + self.url_length as usize + 10;
        let found = swoole_rs_http::header_value(
            input,
            start,
            self.header_length as usize,
            name,
            &mut value,
        );
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
    request: *mut HttpRequest,
    tmp_dir: &[u8],
    upload_max: usize,
) -> bool {
    // SAFETY: per the contract.
    let r = unsafe { &mut *request };
    let Some(form) = r.form.as_deref() else {
        return false;
    };
    // SAFETY: the boundary still points into the header buffer it was found in.
    let text = unsafe { crate::bytes(form.boundary, form.boundary_length as usize) };
    let Ok(Some(boundary)) = text.and_then(|text| swoole_rs_http::boundary(text, 0)) else {
        return false;
    };
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
pub unsafe fn destroy_multipart_parser(request: *mut HttpRequest) {
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
pub unsafe fn parse_multipart_data(request: *mut HttpRequest, buffer: *mut HttpString) -> bool {
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
    match swoole_rs_http::multipart_result(processed, length, r.excepted.into()) {
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
unsafe fn parts<'a>(parser: *mut Parser) -> (&'a mut Parser, &'a mut HttpRequest) {
    // SAFETY: per the contract.
    let parser = unsafe { &mut *parser };
    // SAFETY: per the contract.
    let request = unsafe { &mut *parser.data.cast::<HttpRequest>() };
    (parser, request)
}

fn form_of(request: &mut HttpRequest) -> &mut HttpForm {
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
    let max_length = request.max_length as usize;
    let form = form_of(request);
    if form.header_name.len().saturating_add(length) > max_length {
        (request.excepted, request.too_large) = (true, true);
        return MPPE_PAUSED;
    }
    // SAFETY: the parser lends `length` bytes at `at`.
    let fragment = unsafe { core::slice::from_raw_parts(at.cast::<u8>(), length) };
    form.header_name.extend_from_slice(fragment);
    0
}

unsafe extern "C" fn on_header_value(
    parser: *mut Parser,
    at: *const c_char,
    length: usize,
) -> c_int {
    // SAFETY: the parser calls back with itself, set up by `init_multipart_parser`.
    let (parser, request) = unsafe { parts(parser) };
    let form = form_of(request);
    // The value callback means the name is complete.
    let (saved, name) = (form.saved, core::mem::take(&mut form.header_name));
    // Only the reactor may write this header; a client never names a part header so.
    if case_equal(&name, UPLOAD_FILE) {
        request.op(LOG_RESERVED, ptr::null_mut(), ptr::null(), 0, 0);
        return MPPE_ERROR;
    }
    request.append(saved, name.as_ptr(), name.len());
    request.append(saved, b": ".as_ptr(), 2);
    request.append(saved, at.cast(), length);
    request.append(saved, b"\r\n".as_ptr(), 2);
    if !case_equal(&name, b"content-disposition") {
        return 0;
    }
    // `parse_cookie` over a NUL-terminated copy, stopping at the first `filename`.
    // SAFETY: the parser lends `length` bytes at `at`.
    let mut value = unsafe { core::slice::from_raw_parts(at.cast::<u8>(), length) }.to_vec();
    value.push(0);
    if swoole_rs_http::multipart_filename(&mut value) != 1 {
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
    let max_length = u64::from(request.max_length);
    let form = form_of(request);
    let saved = form.saved;
    if parser.fp.is_null() {
        // SAFETY: the saved buffer is live while the form is.
        if (unsafe { (*saved).length } as u64).wrapping_add(length as u64) > max_length {
            (request.excepted, request.unavailable) = (true, true);
            return MPPE_PAUSED;
        }
        request.append(saved, at.cast(), length);
        return 0;
    }
    form.upload_size = form.upload_size.wrapping_add(length);
    if form.upload_size > form.upload_max {
        (request.excepted, request.too_large) = (true, true);
        return MPPE_PAUSED;
    }
    let written = request.op(FWRITE, parser.fp, at.cast(), length as u64, 0);
    if written != length as u64 {
        request.op(FCLOSE, parser.fp, ptr::null(), 0, 0);
        parser.fp = ptr::null_mut();
        (request.excepted, request.unavailable) = (true, true);
        request.op(LOG_WRITE, ptr::null_mut(), ptr::null(), 0, 0);
        return MPPE_PAUSED;
    }
    0
}

unsafe extern "C" fn on_headers_complete(parser: *mut Parser) -> c_int {
    // SAFETY: the parser calls back with itself, set up by `init_multipart_parser`.
    let (parser, request) = unsafe { parts(parser) };
    let form = form_of(request);
    form.header_name.clear();
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
    let form = form_of(request);
    form.header_name.clear();
    let saved = form.saved;
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
    0
}

#[must_use]
pub fn request_new() -> *mut HttpRequest {
    Box::into_raw(Box::new(HttpRequest::new(effects)))
}

/// `delete request` of one [`request_new`] made: the destructor, then the memory.
///
/// # Safety
/// `request` is null or from [`request_new`], not used afterwards.
pub unsafe fn request_free(request: *mut HttpRequest) {
    if !request.is_null() {
        // SAFETY: per the contract.
        unsafe { destroy_multipart_parser(request) };
        // SAFETY: per the contract: the request is boxed and nothing uses it after.
        drop(unsafe { Box::from_raw(request) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use swoole_rs_http::CHUNK_DATA;

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
        if op == PARSER_EXECUTE {
            HOST.with_borrow_mut(|host| host.effects.push(op));
            // Outside the borrow: the parser calls back into the request's effects.
            return unsafe { multipart_parser_execute(a.cast(), b.cast(), size(n)) }.cast_unsigned()
                as u64;
        }
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
                // A test's stand-in parser, or else the bundled one.
                PARSER_NEW if PARSER.get().is_null() => unsafe {
                    multipart_parser_init(b.cast(), size(n), a.cast()) as u64
                },
                PARSER_NEW => PARSER.get() as u64,
                PARSER_FREE if a.cast() != PARSER.get() => {
                    unsafe { multipart_parser_free(a.cast()) };
                    0
                }
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

    fn request(input: &mut Input) -> HttpRequest {
        let mut request = HttpRequest::new(native);
        request.buffer = &raw mut input.buffer;
        request
    }

    fn url(request: &HttpRequest, input: &Input) -> Vec<u8> {
        let at = request.url_offset as usize;
        input.bytes[at..at + request.url_length as usize].to_vec()
    }

    fn parsed(text: &[u8]) -> (Box<Input>, HttpRequest) {
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

    /// The bundled parser (crates/oracle's `thirdparty/multipart_parser.c`) read through
    /// this crate's mirror: what `init` and a rejected start boundary write lands where
    /// `Parser` and `ParserSettings` say.
    #[test]
    fn parser_mirrors_the_bundled_multipart_parser() {
        use swoole_rs_oracle as _;
        let p = unsafe { multipart_parser_init(c"abc".as_ptr(), 3, &raw const SETTINGS) };
        assert!(!p.is_null());
        let parser = unsafe { &*p };
        assert_eq!(parser.settings, (&raw const SETTINGS).cast());
        assert_eq!(parser.boundary_length, 5, "\"--\" + boundary");
        assert_eq!(&parser.boundary[..6], b"--abc\0");
        assert!(parser.fp.is_null() && parser.data.is_null(), "calloc'd");
        assert_eq!(
            unsafe { multipart_parser_execute(p, c"x".as_ptr(), 1) },
            -1,
            "MPPE_ERROR"
        );
        assert_eq!(unsafe { &*p }.error_reason, 4, "MPPE_BAD_START_BOUNDARY");
        unsafe { multipart_parser_free(p) };
    }

    #[test]
    fn layouts_match_native_static_asserts() {
        use core::mem::{offset_of, size_of};
        let request = [
            offset_of!(HttpRequest, url_offset),
            offset_of!(HttpRequest, content_length),
            offset_of!(HttpRequest, form),
        ];
        assert_eq!(request, [12, 32, 40], "HttpRequest fields");
        let tail = [
            offset_of!(HttpRequest, buffer),
            offset_of!(HttpRequest, native),
            size_of::<HttpRequest>(),
        ];
        assert_eq!(tail, [48, 56, 88], "HttpRequest tail");
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
    fn get_protocol() {
        // `SW_HTTP_GET`, `_POST`, `_PUT`, `_PRI`; `SW_HTTP_VERSION_10`, `_11`.
        let (get, post, put, pri, v10, v11) = (2, 4, 5, 27, 1, 2);
        let protocol = |text: &[u8]| {
            let mut i = input(text);
            let mut r = request(&mut i);
            let status = r.get_protocol();
            let url = url(&r, &i);
            (status, r, url, i.buffer.offset)
        };
        let (status, r, url, _) =
            protocol(b"GET /index.html HTTP/1.1\r\nHost: example.com\r\n\r\n");
        assert_eq!((status, r.method, r.version), (SW_OK, get, v11), "GET");
        assert_eq!(url, b"/index.html", "its URL");

        // A long URL: `append_random_bytes(swoole_rand(1024, 2048), 1)` after `?a=`.
        let mut raw =
            vec![0u8; usize::try_from(swoole_rs_corebase::swoole_rand(1024, 2048)).unwrap()];
        assert_eq!(swoole_rs_corebase::random_bytes(&mut raw), raw.len());
        let mut encoded = vec![0u8; raw.len().div_ceil(3) * 4 + 1];
        let n = swoole_rs_base64::encode(&raw, &mut encoded);
        let text = [
            &b"GET /home/explore/?a="[..],
            &encoded[..n],
            b" HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: keep-alive\r\nCache-Control: max-age=0\r\n\r\n",
        ]
        .concat();
        let (status, r, url, _) = protocol(&text);
        assert_eq!((status, r.method, r.version), (SW_OK, get, v11), "long URL");
        assert_eq!(
            url.split(|b| *b == b'?').next().unwrap(),
            b"/home/explore/",
            "its path"
        );

        let (status, r, url, _) = protocol(
            b"POST /submit HTTP/1.0\r\nContent-Type: application/json\r\n\r\n{\"key\":\"value\"}",
        );
        assert_eq!((status, r.method, r.version), (SW_OK, post, v10), "POST");
        assert_eq!(url, b"/submit", "its URL");

        let (status, r, url, _) =
            protocol(b"PUT /resource HTTP/1.1\r\nContent-Type: text/plain\r\n\r\nNew content");
        assert_eq!((status, r.method, r.version), (SW_OK, put, v11), "PUT");
        assert_eq!(url, b"/resource", "its URL");

        let (status, r, _, offset) = protocol(b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n");
        assert_eq!((status, r.method), (SW_OK, pri), "HTTP/2 preface");
        assert_eq!(offset, 24, "the preface consumed");

        let (status, r, ..) = protocol(b"GET /");
        assert_eq!(
            (status, r.excepted),
            (SW_ERR, false),
            "too short: wait for more"
        );
        for text in [
            &b"UNKNOWN /path HTTP/1.1\r\n"[..],
            b"UPDATE  HTTP/1.1\r\n",
            b"GET /index.html HTTP/2.0\r\n",
            b"UNSUBSCRIBE /index.html\r\n",
            b"BREW / HTTP/1.1\r\n\r\n",
        ] {
            let (status, r, ..) = protocol(text);
            assert_eq!(
                (status, r.excepted),
                (SW_ERR, true),
                "{}",
                String::from_utf8_lossy(text)
            );
        }

        let (status, r, url, _) = protocol(b"GET /search?q=test&page=1#results HTTP/1.1\r\n");
        assert_eq!((status, r.method), (SW_OK, get), "complex URL");
        assert_eq!(url, b"/search?q=test&page=1#results", "its URL");

        let (status, r, url, _) = protocol(b"GET    /index.html    HTTP/1.1\r\n");
        assert_eq!(
            (status, r.method, r.version),
            (SW_OK, get, v11),
            "extra spaces"
        );
        assert_eq!(url, b"/index.html", "its URL");
    }

    // core-tests/src/server/http_server.cpp: all_method.
    #[test]
    fn all_method() {
        let pri = 27;
        for i in 0..pri + 4 {
            let Some(name) = swoole_rs_http::method_string(i) else {
                assert!(i == 0 || i > pri, "{i}");
                continue;
            };
            assert!(i != 0 && i <= pri, "{i}");
            let name = name.to_bytes();
            let mut i_ = input(&[name, b" / HTTP/1.1\r\nHost: example.com\r\n\r\n"].concat());
            let mut r = request(&mut i_);
            if i == pri {
                assert_eq!((r.get_protocol(), r.excepted), (SW_ERR, true), "PRI");
            } else {
                assert_eq!((r.get_protocol(), c_int::from(r.method)), (SW_OK, i), "{i}");
            }
        }
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
        assert!(r.chunked && !r.excepted, "chunked with Content-Length: 0");
    }

    #[test]
    fn chunked_must_be_the_final_transfer_coding() {
        for (codings, chunked, excepted) in [
            (&b"Transfer-Encoding: chunked"[..], true, false),
            (b"Transfer-Encoding: gzip, \tCHUNKED  ", true, false),
            (
                b"Transfer-Encoding: gzip\r\nTransfer-Encoding: chunked",
                true,
                false,
            ),
            (b"Transfer-Encoding: chunked, gzip", false, true),
            (
                b"Transfer-Encoding: chunked\r\nTransfer-Encoding: chunked",
                false,
                true,
            ),
            (b"Transfer-Encoding: chunkedchunked", false, true),
            (b"Transfer-Encoding: chunked\t", false, true),
            (b"Transfer-Encoding:", false, true),
            (b"Content-Length: 3", false, false),
        ] {
            let head = [&b"POST / HTTP/1.1\r\n"[..], codings, b"\r\n\r\n"].concat();
            let (_, r) = parsed(&head);
            assert_eq!(
                (r.chunked, r.excepted, r.header_parsed),
                (chunked, excepted, true),
                "{}",
                String::from_utf8_lossy(codings)
            );
        }
    }

    /// `get_chunked_body_length` over `body` after a chunked head, under `max`.
    fn chunked(body: &[u8], max: u32) -> (i32, HttpRequest, Box<Input>) {
        let head = b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n";
        let (i, mut r) = parsed(&[&head[..], body].concat());
        r.max_length = max;
        (r.get_chunked_body_length(), r, i)
    }

    #[test]
    fn chunked_bodies_end_after_the_trailer_section() {
        let head = 47u64;
        for (body, length) in [
            (&b"3\r\nabc\r\n0\r\n\r\n"[..], 13),
            (b"3;a=b;c;d=\"x\\\"y\"\r\nabc\r\n0\r\n\r\n", 28),
            (b"0\r\nX-Trailer: 1\r\n\r\n", 19),
            // A pipelined request behind the body stays out of it.
            (b"0\r\n\r\nGET / HTTP/1.1\r\n\r\n", 5),
        ] {
            let (status, r, _) = chunked(body, 1 << 20);
            assert_eq!(
                (status, r.known_length, r.content_length),
                (SW_OK, true, length),
                "{}",
                String::from_utf8_lossy(body)
            );
            assert_eq!(r.chunk_offset as u64, head + length, "the body's end");
        }
    }

    #[test]
    fn malformed_chunks_are_errors_and_partial_ones_wait() {
        for body in [
            &b"q\r\n0\r\n\r\n"[..],
            b"3 \r\nabc\r\n0\r\n\r\n",
            b"3\nabc\r\n0\r\n\r\n",
            b"3\r\nabcd\r\n0\r\n\r\n",
            b"3\r\nabc\rx",
            b"3;\r\nabc\r\n0\r\n\r\n",
            b"3;a=\r\nabc\r\n0\r\n\r\n",
            b"3;a=\"b\r\nabc\r\n0\r\n\r\n",
            b"3;a b\r\nabc\r\n0\r\n\r\n",
        ] {
            let (status, r, _) = chunked(body, 1 << 20);
            assert_eq!(
                (status, r.excepted, r.too_large),
                (SW_ERR, true, false),
                "{}",
                String::from_utf8_lossy(body)
            );
        }
        for body in [
            &b""[..],
            b"3",
            b"3\r",
            b"3\r\nab",
            b"3\r\nabc\r",
            b"0\r\n",
            b"0\r\nX: 1\r\n",
        ] {
            let (status, r, _) = chunked(body, 1 << 20);
            assert_eq!(
                (status, r.excepted, r.too_large),
                (SW_ERR, false, false),
                "waits: {}",
                String::from_utf8_lossy(body)
            );
        }
    }

    #[test]
    fn a_split_body_resumes_where_the_scan_stopped() {
        let (status, mut r, mut i) = chunked(b"3\r\nab", 1 << 20);
        assert_eq!(
            (status, r.chunk_state, r.chunk_length),
            (SW_ERR, CHUNK_DATA, 3)
        );
        let rest = b"c\r\n0\r\n\r\n";
        let length = i.buffer.length;
        i.bytes[length..length + rest.len()].copy_from_slice(rest);
        i.buffer.length += rest.len();
        assert_eq!(r.get_chunked_body_length(), SW_OK, "complete");
        assert_eq!(r.content_length, 13, "the whole body");
    }

    #[test]
    fn chunked_bodies_past_package_max_length_are_too_large() {
        // 47 header bytes: a 3-byte chunk needs 47 + 3 + 3 + 2 + 5 = 60.
        let (status, r, _) = chunked(b"3\r\nabc\r\n0\r\n\r\n", 60);
        assert_eq!(
            (status, r.content_length),
            (SW_OK, 13),
            "exactly at the limit"
        );
        let (status, r, _) = chunked(b"3\r\nabc\r\n0\r\n\r\n", 59);
        assert_eq!(
            (status, r.too_large, r.excepted),
            (SW_ERR, true, false),
            "one byte over"
        );
        let (status, r, _) = chunked(b"FFFFFFFFFFFFFFFFFFFF\r\n", u32::MAX);
        assert_eq!((status, r.too_large), (SW_ERR, true), "no overflow");
        let (status, r, _) = chunked(b"0\r\nX-Trailer: 1\r\n\r\n", 60);
        assert_eq!(
            (status, r.too_large),
            (SW_ERR, true),
            "a trailer past the limit"
        );
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

    /// `http_upload_provenance.cpp`'s `MultipartFixture`: a request whose form has
    /// boundary `b`, fed through the bundled parser as the reactor feeds it.
    struct Fixture {
        input: Box<Input>,
        request: Box<HttpRequest>,
    }

    impl Fixture {
        fn new() -> Self {
            use swoole_rs_oracle as _;
            let mut input = input(b"");
            let mut request = Box::new(request(&mut input));
            request.max_length = u32::try_from(BUFFER_SIZE_BIG).unwrap();
            let boundary = b"boundary=b";
            request.form = Some(Box::new(HttpForm {
                boundary: boundary.as_ptr(),
                boundary_length: u32::try_from(boundary.len()).unwrap(),
                parser: ptr::null_mut(),
                saved: ptr::null_mut(),
                upload: ptr::null_mut(),
                template: Vec::new(),
                header_name: Vec::new(),
                upload_size: 0,
                upload_max: 0,
            }));
            let r = &raw mut *request;
            assert!(unsafe { init_multipart_parser(r, b"/tmp", 1024 * 1024) });
            Self { input, request }
        }

        /// Like the reactor, the parse buffer is cleared after every read and refilled from its start.
        fn feed(&mut self, data: &[u8]) -> bool {
            self.request
                .append(self.request.buffer, data.as_ptr(), data.len());
            let r = &raw mut *self.request;
            unsafe { parse_multipart_data(r, (*r).buffer) }
        }

        fn forwarded(&self) -> String {
            String::from_utf8_lossy(&self.input.bytes[..self.input.buffer.length])
                .to_ascii_lowercase()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            unsafe { destroy_multipart_parser(&raw mut *self.request) };
        }
    }

    const PART_PREFIX: &[u8] = b"--b\r\nContent-Disposition: form-data; name=\"text\"\r\n";

    // core-tests/src/server/http_upload_provenance.cpp: reject_client_upload_marker.
    #[test]
    fn reject_client_upload_marker() {
        for name in [&b"Swoole-Upload-File"[..], b"sWoOlE-uPlOaD-fIlE"] {
            let body = [
                PART_PREFIX,
                name,
                b": /tmp/victim\r\n\r\nvalue\r\n--b--\r\n",
            ]
            .concat();
            for split in 1..body.len() {
                let trace = format!("{} split={split}", String::from_utf8_lossy(name));
                let mut f = Fixture::new();
                let mut ok = f.feed(&body[..split]);
                if ok {
                    ok = f.feed(&body[split..]);
                }
                assert!(!ok, "{trace}");
                assert!(
                    !f.forwarded().contains("\r\nswoole-upload-file:"),
                    "{trace}"
                );
                assert!(!f.request.tried_to_dispatch, "{trace}");
            }
        }
    }

    // core-tests/src/server/http_upload_provenance.cpp: header_name_split_across_reads_is_assembled.
    #[test]
    fn header_name_split_across_reads_is_assembled() {
        let body = [PART_PREFIX, b"X-Custom-Header: v\r\n\r\nvalue\r\n--b--\r\n"].concat();
        let start = body
            .windows(15)
            .position(|w| w == b"X-Custom-Header")
            .unwrap();
        for split in start..=start + b"X-Custom-Header: ".len() {
            let mut f = Fixture::new();
            assert!(f.feed(&body[..split]), "{split}");
            assert!(f.feed(&body[split..]), "{split}");
            assert!(f.request.tried_to_dispatch, "{split}");
            assert!(
                f.forwarded().contains("\r\nx-custom-header: v\r\n"),
                "{split}"
            );
        }
    }

    // core-tests/src/server/http_upload_provenance.cpp:
    // overwritten_read_buffer_cannot_forge_upload_marker. The header name must not be
    // a reference into the read buffer: a later read overwrites that memory.
    #[test]
    fn overwritten_read_buffer_cannot_forge_upload_marker() {
        let name = [b'A'; 24];
        let first = [PART_PREFIX, &name, b":"].concat();
        let name_offset = first.len() - name.len() - 1;

        // The forged header has the same length as the already accepted name and lands
        // exactly where it was stored.
        let forged = b"A: b\r\nSwoole-Upload-File";
        assert_eq!(forged.len(), name.len());
        let mut second = b" /tmp/victim\r\n\r\n".to_vec();
        assert!(second.len() <= name_offset);
        second.resize(name_offset, b'x');
        second.extend_from_slice(forged);
        second.extend_from_slice(b"\r\n--b--\r\n");

        let mut f = Fixture::new();
        assert!(f.feed(&first));
        assert!(f.feed(&second));
        assert!(!f.forwarded().contains("\r\nswoole-upload-file:"));
    }
}
