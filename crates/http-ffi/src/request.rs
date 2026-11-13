//! `http_server::Request`, the HTTP/1 request a `ListenPort` parses (#931): its layout
//! and every operation over it. Native code keeps the struct's fields, because
//! upstream's own `ListenPort` reads them whenever the ports module is off, but each
//! method is a call into this module, and its `FormData` is a Rust [`HttpForm`]. Its
//! effects go through the request's [`HttpNative`] call, `effects` outside tests:
//! `swoole::String` allocation, stdio and `swoole_tmpfile`. Its logs go to corebase's
//! log host. Trace logs are compiled out, as in upstream's builds without
//! `--enable-trace-log`.
use core::ffi::{CStr, c_int, c_void};
use core::ptr;
use swoole_rs_buffer::string::{CppString, delete_string, new_sized_string, swr_string_append};
use swoole_rs_corebase::{log, set_errno};
use swoole_rs_http::multipart_parser::{MPPE_PAUSED, MultipartParser, Parts};
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

/// `op(op, a, b, n, m)`: one native effect (`SWR_HTTP_NATIVE_*`).
pub type HttpNative = unsafe extern "C" fn(u32, *mut c_void, *const u8, u64, u64) -> u64;

// HttpNative effects. Each reads only its arguments.
/// `new String(n)`.
const STRING_NEW: u32 = 0;
const STRING_FREE: u32 = 1;
/// `a->append(b, n)`.
const STRING_APPEND: u32 = 2;
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
    reason = "lengths and descriptors native code carried as u64"
)]
unsafe extern "C" fn effects(op: u32, a: *mut c_void, b: *const u8, n: u64, _: u64) -> u64 {
    let length = n as usize;
    let file = a.cast::<libc::FILE>();
    match op {
        STRING_NEW => return new_sized_string(length, None) as u64,
        // SAFETY: a string from `STRING_NEW`, not used again.
        STRING_FREE => unsafe { delete_string(a.cast()) },
        STRING_APPEND => {
            // SAFETY: a live `swoole::String`.
            let string = unsafe { &mut *a.cast::<CppString>() };
            // SAFETY: `n` readable bytes at `b`.
            unsafe { swr_string_append(string, b, length) };
        }
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
        _ => std::process::abort(),
    }
    0
}

/// The multipart parser's `MPPE_ERROR` callback result.
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
    parser: Option<MultipartParser>,
    /// The current upload's stream, null between uploads.
    fp: *mut c_void,
    /// The receive buffer, cut back to the header, that the body is rewritten into.
    saved: *mut HttpString,
    /// The current upload's file name, NUL-terminated.
    upload: Vec<u8>,
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

    /// The saved buffer's length, 0 before `init_multipart_parser` saves one.
    fn saved_length(&self) -> usize {
        // SAFETY: null, or the saved buffer, which is live while the form is.
        unsafe { self.saved.as_ref() }.map_or(0, |saved| saved.length)
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
                parser: None,
                fp: ptr::null_mut(),
                saved: ptr::null_mut(),
                upload: Vec::new(),
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

    /// `Request::init_multipart_parser`: a parser over the form's boundary, the body moved
    /// to its own buffer and the receive buffer kept, cut back to the header, to rewrite
    /// the body into. False without a form.
    pub fn init_multipart_parser(&mut self, tmp_dir: &[u8], upload_max: usize) -> bool {
        let r = self;
        let Some(form) = r.form.as_deref() else {
            return false;
        };
        // SAFETY: the boundary still points into the header buffer it was found in.
        let text = unsafe { crate::bytes(form.boundary, form.boundary_length as usize) };
        let Ok(Some(boundary)) = text.and_then(|text| swoole_rs_http::boundary(text, 0)) else {
            return false;
        };
        let at = boundary.offset..boundary.offset + boundary.length;
        let parser = MultipartParser::new(text.unwrap_or_default().get(at).unwrap_or_default());
        let Some(form) = r.form.as_deref_mut() else {
            return false;
        };
        form.parser = Some(parser);
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
        let Some(form) = r.form.as_deref_mut() else {
            return false;
        };
        form.saved = saved;
        form.template = template;
        form.upload_max = upload_max;
        true
    }

    /// `Request::destroy_multipart_parser`: the body buffer freed, the receive buffer back,
    /// an unfinished upload closed and unlinked, the parser and form gone.
    pub fn destroy_multipart_parser(&mut self) {
        let r = self;
        let Some(form) = r.form.take() else {
            return;
        };
        r.op(STRING_FREE, r.buffer.cast(), ptr::null(), 0, 0);
        r.buffer = form.saved;
        if !form.fp.is_null() {
            r.op(FCLOSE, form.fp, ptr::null(), 0, 0);
            r.op(
                UNLINK,
                form.upload.as_ptr().cast_mut().cast(),
                ptr::null(),
                0,
                0,
            );
        }
    }

    /// `Request::parse_multipart_data`: feeds `buffer` to the parser, whose callbacks
    /// rewrite the body into the saved buffer. False rejects the request.
    pub fn parse_multipart_data(&mut self, buffer: &mut HttpString) -> bool {
        let r = self;
        r.excepted = false;
        let Some(mut parser) = r.form.as_deref_mut().and_then(|form| form.parser.take()) else {
            return false;
        };
        let length = buffer.length;
        // SAFETY: the buffer holds `length` bytes, which the callbacks only append past.
        let data = unsafe { buffer.bytes() };
        let processed = parser.execute(r, data) as i64;
        let reason = parser.error_message();
        if let Some(form) = r.form.as_deref_mut() {
            form.parser = Some(parser);
        }
        match swoole_rs_http::multipart_result(processed, length, r.excepted.into()) {
            outcome if outcome < 0 => {
                // Upstream formats it into 256 bytes, its NUL included.
                let reason = reason.get(..255).unwrap_or(&reason);
                log::error_log(
                    NOTICE,
                    INVALID_REQUEST,
                    c"Request::parse_multipart_data",
                    format_args!(
                        "parse multipart body failed, reason: {}",
                        String::from_utf8_lossy(reason)
                    ),
                );
                false
            }
            2 => {
                // `buffer->clear()`.
                (buffer.length, buffer.offset) = (0, 0);
                true
            }
            outcome => {
                log::error_log(
                    NOTICE,
                    INVALID_REQUEST,
                    c"Request::parse_multipart_data",
                    format_args!(
                        "parse multipart body failed, {}/{length} bytes processed",
                        processed.cast_unsigned()
                    ),
                );
                outcome != 0
            }
        }
    }
}

fn form_of(request: &mut HttpRequest) -> &mut HttpForm {
    // Callbacks run only from `parse_multipart_data`, on a request with a form.
    request
        .form
        .as_deref_mut()
        .unwrap_or_else(|| std::process::abort())
}

/// The multipart parser's callbacks, which rewrite the body into the saved buffer.
impl Parts for HttpRequest {
    fn on_header_field(&mut self, data: &[u8]) -> c_int {
        let max_length = self.max_length as usize;
        let form = form_of(self);
        if form.header_name.len().saturating_add(data.len()) > max_length {
            (self.excepted, self.too_large) = (true, true);
            return MPPE_PAUSED;
        }
        form.header_name.extend_from_slice(data);
        0
    }

    fn on_header_value(&mut self, data: &[u8]) -> c_int {
        let form = form_of(self);
        // The value callback means the name is complete.
        let (saved, name) = (form.saved, core::mem::take(&mut form.header_name));
        // Only the reactor may write this header; a client never names a part header so.
        if case_equal(&name, UPLOAD_FILE) {
            log::error_log(
                WARNING,
                INVALID_REQUEST,
                c"http_server::multipart_on_header_value",
                format_args!(
                    "Bad Request: the reserved header 'Swoole-Upload-File' may not be sent by clients"
                ),
            );
            return MPPE_ERROR;
        }
        self.append(saved, name.as_ptr(), name.len());
        self.append(saved, b": ".as_ptr(), 2);
        self.append(saved, data.as_ptr(), data.len());
        self.append(saved, b"\r\n".as_ptr(), 2);
        if !case_equal(&name, b"content-disposition") {
            return 0;
        }
        // `parse_cookie` over a NUL-terminated copy, stopping at the first `filename`.
        let mut value = data.to_vec();
        value.push(0);
        if swoole_rs_http::multipart_filename(&mut value) != 1 {
            return 0;
        }
        let form = form_of(self);
        form.upload.clone_from(&form.template);
        form.upload.push(0);
        // `swoole_tmpfile` fills in the template's `XXXXXX`.
        let name = form.upload.as_mut_ptr();
        form.upload_size = 0;
        let fd = self.op(TMPFILE, name.cast(), ptr::null(), 0, 0);
        // `swoole_tmpfile`'s `int`, sign-extended.
        if fd.cast_signed() < 0 {
            self.excepted = true;
            return 0;
        }
        let fp = self.op(FDOPEN, ptr::null_mut(), ptr::null(), fd, 0) as *mut c_void;
        if fp.is_null() {
            let name = CStr::from_bytes_until_nul(&form_of(self).upload).unwrap_or_default();
            log::sys_warning(
                c"http_server::multipart_on_header_value",
                format_args!("fopen({}) failed", name.to_string_lossy()),
            );
            return 0;
        }
        form_of(self).fp = fp;
        0
    }

    fn on_part_data(&mut self, data: &[u8]) -> c_int {
        let max_length = u64::from(self.max_length);
        let length = data.len();
        let form = form_of(self);
        let (saved, fp) = (form.saved, form.fp);
        if fp.is_null() {
            if (form.saved_length() as u64).wrapping_add(length as u64) > max_length {
                (self.excepted, self.unavailable) = (true, true);
                return MPPE_PAUSED;
            }
            self.append(saved, data.as_ptr(), length);
            return 0;
        }
        form.upload_size = form.upload_size.wrapping_add(length);
        if form.upload_size > form.upload_max {
            (self.excepted, self.too_large) = (true, true);
            return MPPE_PAUSED;
        }
        let written = self.op(FWRITE, fp, data.as_ptr(), length as u64, 0);
        if written != length as u64 {
            self.op(FCLOSE, fp, ptr::null(), 0, 0);
            form_of(self).fp = ptr::null_mut();
            (self.excepted, self.unavailable) = (true, true);
            log::sys_warning(
                c"http_server::multipart_on_data",
                format_args!("failed to write upload file"),
            );
            return MPPE_PAUSED;
        }
        0
    }

    fn on_headers_complete(&mut self) -> c_int {
        let form = form_of(self);
        form.header_name.clear();
        let (saved, fp) = (form.saved, form.fp);
        if !fp.is_null() {
            self.append(
                saved,
                b"Swoole-Upload-File: ".as_ptr(),
                UPLOAD_FILE.len() + 2,
            );
            let form = form_of(self);
            // The name up to its NUL: `swoole_tmpfile` wrote no other.
            let name = form
                .upload
                .split(|&byte| byte == 0)
                .next()
                .unwrap_or_default();
            let (name, length) = (name.as_ptr(), name.len());
            self.append(saved, name, length);
        }
        self.multipart_header_parsed = true;
        self.append(saved, b"\r\n".as_ptr(), 2);
        0
    }

    fn on_part_data_end(&mut self) -> c_int {
        self.multipart_header_parsed = false;
        let form = form_of(self);
        let (saved, fp) = (form.saved, form.fp);
        if !fp.is_null() {
            self.append(
                saved,
                b"\r\nSwoole-Upload-File".as_ptr(),
                UPLOAD_FILE.len() + 2,
            );
            self.op(FFLUSH, fp, ptr::null(), 0, 0);
            self.op(FCLOSE, fp, ptr::null(), 0, 0);
            form_of(self).fp = ptr::null_mut();
        }
        self.append(saved, b"\r\n".as_ptr(), 2);
        0
    }

    fn on_part_data_begin(&mut self, boundary: &[u8]) -> c_int {
        let form = form_of(self);
        form.header_name.clear();
        let saved = form.saved;
        self.append(saved, boundary.as_ptr(), boundary.len());
        self.append(saved, b"\r\n".as_ptr(), 2);
        0
    }

    fn on_body_end(&mut self, boundary: &[u8]) -> c_int {
        let saved = form_of(self).saved;
        self.append(saved, boundary.as_ptr(), boundary.len());
        self.append(saved, b"--".as_ptr(), 2);
        let length = form_of(self).saved_length();
        self.content_length = (length as u64).wrapping_sub(u64::from(self.header_length));
        self.tried_to_dispatch = true;
        0
    }
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
        // SAFETY: per the contract: the request is boxed and nothing uses it after.
        let mut request = unsafe { Box::from_raw(request) };
        request.destroy_multipart_parser();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
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
        effects();
        assert!(r.init_multipart_parser(b"/tmp", 100), "initialized");
        assert_eq!(
            i.buffer.length,
            head.len(),
            "the receive buffer keeps the header"
        );
        let field = |r: &mut HttpRequest, name: &[u8], value: &[u8]| {
            r.on_header_field(name);
            r.on_header_value(value);
        };
        r.on_part_data_begin(b"--AB");
        field(&mut r, b"Content-Disposition", b"form-data; name=\"a\"");
        r.on_headers_complete();
        r.on_part_data(b"1");
        r.on_part_data_end();
        r.on_part_data_begin(b"--AB");
        field(
            &mut r,
            b"Content-Disposition",
            b"form-data; name=\"f\"; filename=\"x.txt\"",
        );
        r.on_headers_complete();
        r.on_part_data(b"file");
        r.on_part_data_end();
        r.on_body_end(b"--AB");
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
        r.destroy_multipart_parser();
        assert!(
            r.form.is_none() && r.buffer == (&raw const i.buffer).cast_mut(),
            "the receive buffer is back"
        );
        assert_eq!(effects(), [STRING_FREE], "body freed");
    }

    #[test]
    fn oversize_parts_and_uploads_are_rejected() {
        let (_, mut r) =
            parsed(b"POST / HTTP/1.1\r\nContent-Type: multipart/form-data; boundary=AB\r\n\r\n");
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
        assert_eq!(r.on_part_data(b"abc"), 1, "rejected");
        assert!(
            r.excepted && r.unavailable && !r.too_large,
            "a field past max_length"
        );
        (r.excepted, r.unavailable) = (false, false);
        if let Some(form) = r.form.as_deref_mut() {
            form.fp = ptr::without_provenance_mut(0x1000);
        }
        assert_eq!(r.on_part_data(b"abcd"), 1, "rejected");
        assert!(
            r.excepted && r.too_large,
            "an upload past upload_max_filesize"
        );
        r.form = None;
    }

    /// `http_upload_provenance.cpp`'s `MultipartFixture`: a request whose form has
    /// boundary `b`, fed through the multipart parser as the reactor feeds it.
    struct Fixture {
        input: Box<Input>,
        request: Box<HttpRequest>,
    }

    impl Fixture {
        fn new() -> Self {
            let mut input = input(b"");
            let mut request = Box::new(request(&mut input));
            request.max_length = u32::try_from(BUFFER_SIZE_BIG).unwrap();
            let boundary = b"boundary=b";
            request.form = Some(Box::new(HttpForm {
                boundary: boundary.as_ptr(),
                boundary_length: u32::try_from(boundary.len()).unwrap(),
                parser: None,
                fp: ptr::null_mut(),
                saved: ptr::null_mut(),
                upload: Vec::new(),
                template: Vec::new(),
                header_name: Vec::new(),
                upload_size: 0,
                upload_max: 0,
            }));
            assert!(request.init_multipart_parser(b"/tmp", 1024 * 1024));
            Self { input, request }
        }

        /// Like the reactor, the parse buffer is cleared after every read and refilled from its start.
        fn feed(&mut self, data: &[u8]) -> bool {
            self.request
                .append(self.request.buffer, data.as_ptr(), data.len());
            let buffer = self.request.buffer;
            // SAFETY: the fixture's live input buffer, which only the request reaches.
            self.request.parse_multipart_data(unsafe { &mut *buffer })
        }

        fn forwarded(&self) -> String {
            String::from_utf8_lossy(&self.input.bytes[..self.input.buffer.length])
                .to_ascii_lowercase()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            self.request.destroy_multipart_parser();
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
