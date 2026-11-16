//! `http_server::Request`, the HTTP/1 request a `ListenPort` parses (#931), and every
//! operation over it. Its `FormData` is a Rust [`HttpForm`]. Its effects
//! (`swoole::String` allocation, the upload's file and `swoole_tmpfile`) are direct
//! calls, which tests run too. Its logs go to corebase's log host. Trace logs are
//! compiled out, as in upstream's builds without `--enable-trace-log`.
use core::ffi::c_int;
use core::ptr;
use std::ffi::OsStr;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::os::unix::ffi::OsStrExt;
use swoole_rs_buffer::string::{CppString, boxed_sized_string};
use swoole_rs_corebase::log::LogLevel::{SW_LOG_NOTICE as NOTICE, SW_LOG_WARNING as WARNING};
use swoole_rs_corebase::{log, set_errno};
use swoole_rs_http::multipart_parser::{Flow, MultipartParser, Parts};
use swoole_rs_http::{
    ChunkState, HttpChunked, HttpChunks, HttpFrame, HttpHeaderInfo, HttpSpan, MultipartError,
    case_equal,
};
use swoole_rs_ossupport::create_tmpfile;

/// `SW_LOG_NOTICE`, `SW_LOG_WARNING` and `SW_ERROR_SERVER_INVALID_REQUEST`.
const INVALID_REQUEST: c_int = swoole_rs_error::ErrorCode::SW_ERROR_SERVER_INVALID_REQUEST as c_int;

use swoole_rs_corebase::sys::errno;

unsafe extern "C" {
    /// Reads `text` up to its NUL and points `end` into it; any base is defined (an
    /// unsupported one sets `EINVAL`).
    safe fn strtoull(
        text: swoole_rs_corebase::sys::Text<'_>,
        end: &mut *mut core::ffi::c_char,
        base: c_int,
    ) -> core::ffi::c_ulonglong;
}

/// The errno libc's `strtoull` leaves when no digits convert; libcs differ.
fn no_digits_errno() -> c_int {
    static VALUE: std::sync::OnceLock<c_int> = std::sync::OnceLock::new();
    *VALUE.get_or_init(|| {
        let saved = errno();
        set_errno(0);
        let mut end = ptr::null_mut();
        strtoull(c"x".into(), &mut end, 10);
        let value = errno();
        set_errno(saved);
        value
    })
}

/// `SW_HTTP_UPLOAD_FILE`.
const UPLOAD_FILE: &[u8] = b"Swoole-Upload-File";
/// `SW_BUFFER_SIZE_BIG`.
const BUFFER_SIZE_BIG: usize = 65536;

/// A multipart body being streamed: upstream's `FormData`.
pub struct HttpForm {
    /// The `Content-Type` value after `multipart/form-data`, in the header buffer.
    boundary: HttpSpan,
    parser: Option<MultipartParser>,
    /// The current upload's file, buffered as the `FILE` upstream wrote was; none
    /// between uploads.
    fp: Option<BufWriter<File>>,
    /// The body's own buffer, which the request parses from `init_multipart_parser`
    /// until `destroy_multipart_parser` gives it back the receive buffer.
    body: Option<Box<CppString>>,
    /// The receive buffer, cut back to the header, that the body is rewritten into.
    saved: *mut CppString,
    /// The saved buffer's length: only the form's own appends change it.
    saved_length: usize,
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
    /// The `Content-Type` value after `multipart/form-data`: its span of the header buffer.
    #[must_use]
    pub fn boundary(&self) -> &HttpSpan {
        &self.boundary
    }
}

/// `http_server::Request`'s fields.
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
    pub chunk_state: ChunkState,
    pub url_offset: u32,
    pub url_length: u32,
    pub max_length: u32,
    /// Without `\r\n`.
    pub request_line_length: u32,
    /// Including the request line and `\r\n\r\n`.
    pub header_length: u32,
    pub content_length: u64,
    pub form: Option<Box<HttpForm>>,
    pub buffer: *mut CppString,
    /// The current chunk's size, where its line or data starts (0 until the body
    /// is first scanned), and how far its size line has been scanned.
    pub chunk_length: usize,
    pub chunk_offset: usize,
    pub chunk_scan_offset: usize,
}

impl Default for HttpRequest {
    /// `HttpRequest()`.
    fn default() -> Self {
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
            chunk_state: ChunkState::Size,
            url_offset: 0,
            url_length: 0,
            max_length: 0,
            request_line_length: 0,
            header_length: 0,
            content_length: 0,
            form: None,
            buffer: ptr::null_mut(),
            chunk_length: 0,
            chunk_offset: 0,
            chunk_scan_offset: 0,
        }
    }
}

impl HttpRequest {
    /// The buffer being parsed.
    fn input<'a>(&self) -> (&'a mut CppString, &'a [u8]) {
        // SAFETY: a request is parsed only once native code gave it a live buffer,
        // which nothing else touches during the call.
        let buffer = unsafe { &mut *self.buffer };
        if buffer.is_empty() || buffer.as_ptr().is_null() {
            return (buffer, &[]);
        }
        // SAFETY: the buffer holds `len()` initialized bytes at `as_ptr()`.
        let bytes = unsafe { core::slice::from_raw_parts(buffer.as_ptr(), buffer.len()) };
        (buffer, bytes)
    }

    /// `memset(&method, 0, offsetof(HttpRequest, form_data_))`: the form and buffer stay.
    pub fn clean(&mut self) {
        let (form, buffer) = (self.form.take(), self.buffer);
        *self = Self {
            form,
            buffer,
            ..Self::default()
        };
    }

    fn frame(&mut self, parse: fn(&mut HttpFrame, &[u8]) -> bool) -> bool {
        let (buffer, input) = self.input();
        let mut frame = HttpFrame {
            offset: usize::try_from(buffer.offset()).unwrap_or(usize::MAX),
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
        let found = parse(&mut frame, input);
        buffer.set_offset(i64::try_from(frame.offset).unwrap_or(i64::MAX));
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
        found
    }

    /// The method, URL and version from the request line, or an HTTP/2 preface.
    pub fn get_protocol(&mut self) -> bool {
        self.frame(HttpFrame::request_line)
    }

    /// Finds the `\r\n\r\n` ending the header block.
    pub fn get_header_length(&mut self) -> bool {
        self.frame(HttpFrame::header_end)
    }

    /// `Content-Length`, `Connection`, `Transfer-Encoding` and a multipart `Content-Type`.
    pub fn parse_header_info(&mut self) {
        let (_, input) = self.input();
        let mut state = HttpHeaderInfo {
            cursor: self.request_line_length as usize + 2,
            content_length: self.content_length,
            flags: u32::from(self.known_length)
                | (u32::from(self.keep_alive) << 1)
                | (u32::from(self.chunked) << 2)
                | (u32::from(self.header_parsed) << 4),
            number_errors: 0,
        };
        loop {
            let end = self.header_length.wrapping_sub(4) as usize;
            let status = swoole_rs_http::header_info(input, end, &mut state);
            self.content_length = state.content_length;
            self.known_length = state.flags & 1 != 0;
            self.keep_alive = state.flags & 2 != 0;
            self.chunked = state.flags & 4 != 0;
            self.header_parsed = state.flags & 16 != 0;
            if state.number_errors != 0 {
                Self::number_errors(state.number_errors.into());
            }
            let Ok(boundary) = status else {
                self.excepted = true;
                return;
            };
            let Some(boundary) = boundary else {
                return;
            };
            self.form = Some(Box::new(HttpForm {
                boundary: HttpSpan {
                    offset: boundary.offset,
                    // Upstream keeps the length in a `uint32_t`.
                    length: boundary.length.min(u32::MAX as usize),
                },
                parser: None,
                fp: None,
                body: None,
                saved: ptr::null_mut(),
                saved_length: 0,
                upload: Vec::new(),
                template: Vec::new(),
                header_name: Vec::new(),
                upload_size: 0,
                upload_max: 0,
            }));
        }
    }

    /// Scans the chunked body from where the last call stopped: true once its
    /// trailer section ended, with `content_length` up to there; false to wait for
    /// more, or with `excepted` (bad framing) or `too_large` (past `max_length`) set.
    pub fn get_chunked_body_length(&mut self) -> bool {
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
                return true;
            }
            HttpChunked::Partial => {}
            HttpChunked::Malformed => self.excepted = true,
            HttpChunked::TooLarge => self.too_large = true,
        }
        false
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
        let start = self.url_offset as usize + self.url_length as usize + 10;
        swoole_rs_http::header_value(input, start, self.header_length as usize, name)
    }

    /// The request's URL, as the buffer's bytes.
    #[must_use]
    pub fn url_bytes(&self) -> &[u8] {
        let (_, input) = self.input();
        let start = self.url_offset as usize;
        input
            .get(start..start + self.url_length as usize)
            .unwrap_or_default()
    }

    /// The value of header `name`, as the buffer's bytes.
    #[must_use]
    pub fn header_bytes(&self, name: &[u8]) -> Option<&[u8]> {
        let span = self.get_header(name)?;
        let (_, input) = self.input();
        input.get(span.offset..span.offset.checked_add(span.length)?)
    }

    /// Leaves the errno `strtoull` would: bit 1 of `errors` `ERANGE`, bit 2 its
    /// no-digits value.
    fn number_errors(errors: u64) {
        if errors & 1 != 0 {
            set_errno(libc::ERANGE);
        }
        if errors & 2 != 0 && no_digits_errno() != 0 {
            set_errno(no_digits_errno());
        }
    }

    /// `append(data)` on the form's saved buffer.
    fn save(&mut self, data: &[u8]) {
        let form = form_of(self);
        // SAFETY: null, or the receive buffer `init_multipart_parser` saved, which
        // native code keeps live while the form is and nothing else borrows during a
        // callback. `data` is never its contents.
        let Some(saved) = (unsafe { form.saved.as_mut() }) else {
            return;
        };
        form.saved_length = form.saved_length.wrapping_add(data.len());
        saved.append(data);
    }

    /// `Request::init_multipart_parser`: a parser over the form's boundary, the body moved
    /// to its own buffer and the receive buffer kept, cut back to the header, to rewrite
    /// the body into. False without a form.
    pub fn init_multipart_parser(&mut self, tmp_dir: &[u8], upload_max: usize) -> bool {
        let r = self;
        let Some(form) = r.form.as_deref() else {
            return false;
        };
        // The header buffer the boundary was found in is still the request's.
        let (buffer, input) = r.input();
        let text = input
            .get(form.boundary.offset..)
            .and_then(|text| text.get(..form.boundary.length));
        let Some(text) = text else {
            return false;
        };
        let Ok(Some(boundary)) = swoole_rs_http::boundary(text, 0) else {
            return false;
        };
        let at = boundary.offset..boundary.offset + boundary.length;
        let parser = MultipartParser::new(text.get(at).unwrap_or_default());
        let Some(form) = r.form.as_deref_mut() else {
            return false;
        };
        form.parser = Some(parser);
        let header = r.header_length as usize;
        let mut body = boxed_sized_string(BUFFER_SIZE_BIG, None);
        body.append(input.get(header..).unwrap_or_default());
        buffer.truncate(header);
        let mut template = tmp_dir.to_vec();
        template.extend_from_slice(b"/swoole.upfile.XXXXXX");
        (form.saved, form.saved_length) = (r.buffer, header);
        r.buffer = &raw mut **form.body.insert(body);
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
        // A form `init_multipart_parser` never ran on has no body buffer.
        let Some(mut body) = form.body else {
            return;
        };
        body.dispose();
        r.buffer = form.saved;
        if let Some(fp) = form.fp {
            drop(fp);
            let name = form.upload.split(|&byte| byte == 0).next();
            _ = std::fs::remove_file(OsStr::from_bytes(name.unwrap_or_default()));
        }
    }

    /// `Request::parse_multipart_data(buffer_)`: feeds the request's buffer, the body
    /// since `init_multipart_parser`, to the parser, whose callbacks rewrite it into the
    /// saved buffer. False rejects the request.
    pub fn parse_multipart_data(&mut self) -> bool {
        let r = self;
        r.excepted = false;
        let Some(mut parser) = r.form.as_deref_mut().and_then(|form| form.parser.take()) else {
            return false;
        };
        // The callbacks append to the saved buffer, never to this one.
        let (buffer, data) = r.input();
        let length = buffer.len();
        let processed = parser.execute(r, data);
        let reason = parser.error_message();
        if let Some(form) = r.form.as_deref_mut() {
            form.parser = Some(parser);
        }
        match swoole_rs_http::multipart_result(processed, length, r.excepted) {
            Err(MultipartError::Parser) => {
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
            Ok(()) => {
                buffer.clear();
                true
            }
            Err(MultipartError::Short { excepted }) => {
                // The policy reports Short only for a successful byte count.
                log::error_log(
                    NOTICE,
                    INVALID_REQUEST,
                    c"Request::parse_multipart_data",
                    format_args!(
                        "parse multipart body failed, {}/{length} bytes processed",
                        processed.unwrap_or_default()
                    ),
                );
                excepted
            }
        }
    }
}

fn form_of(request: &mut HttpRequest) -> &mut HttpForm {
    // Callbacks run only from `parse_multipart_data`, on a request with a form.
    request.form.as_deref_mut().unwrap_or_else(|| {
        swoole_rs_corebase::fatal("a multipart callback on a request without a form")
    })
}

/// The multipart parser's callbacks, which rewrite the body into the saved buffer.
impl Parts<'_> for HttpRequest {
    fn on_header_field(&mut self, data: &[u8]) -> Flow {
        let max_length = self.max_length as usize;
        let form = form_of(self);
        if form.header_name.len().saturating_add(data.len()) > max_length {
            (self.excepted, self.too_large) = (true, true);
            return Flow::Pause;
        }
        form.header_name.extend_from_slice(data);
        Flow::Go
    }

    fn on_header_value(&mut self, data: &[u8]) -> Flow {
        let form = form_of(self);
        // The value callback means the name is complete.
        let name = core::mem::take(&mut form.header_name);
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
            return Flow::Fail;
        }
        self.save(&name);
        self.save(b": ");
        self.save(data);
        self.save(b"\r\n");
        if !case_equal(&name, b"content-disposition") {
            return Flow::Go;
        }
        // `parse_cookie` over a NUL-terminated copy, stopping at the first `filename`.
        let mut value = data.to_vec();
        value.push(0);
        if swoole_rs_http::multipart_filename(&mut value) != Ok(true) {
            return Flow::Go;
        }
        let form = form_of(self);
        form.upload.clone_from(&form.template);
        form.upload.push(0);
        form.upload_size = 0;
        // `swoole_tmpfile` fills in the template's `XXXXXX`.
        let Some(fd) = create_tmpfile(&mut form.upload) else {
            self.excepted = true;
            return Flow::Go;
        };
        form.fp = Some(BufWriter::new(File::from(fd)));
        Flow::Go
    }

    fn on_part_data(&mut self, data: &[u8]) -> Flow {
        let max_length = u64::from(self.max_length);
        let length = data.len();
        let form = form_of(self);
        let Some(fp) = form.fp.as_mut() else {
            if (form.saved_length as u64).wrapping_add(length as u64) > max_length {
                (self.excepted, self.unavailable) = (true, true);
                return Flow::Pause;
            }
            self.save(data);
            return Flow::Go;
        };
        form.upload_size = form.upload_size.wrapping_add(length);
        if form.upload_size > form.upload_max {
            (self.excepted, self.too_large) = (true, true);
            return Flow::Pause;
        }
        if fp.write_all(data).is_err() {
            form.fp = None;
            (self.excepted, self.unavailable) = (true, true);
            log::sys_warning(
                c"http_server::multipart_on_data",
                format_args!("failed to write upload file"),
            );
            return Flow::Pause;
        }
        Flow::Go
    }

    fn on_headers_complete(&mut self) -> Flow {
        let form = form_of(self);
        form.header_name.clear();
        if form.fp.is_some() {
            let upload = core::mem::take(&mut form.upload);
            self.save(b"Swoole-Upload-File: ");
            // The name up to its NUL: `swoole_tmpfile` wrote no other.
            self.save(upload.split(|&byte| byte == 0).next().unwrap_or_default());
            form_of(self).upload = upload;
        }
        self.multipart_header_parsed = true;
        self.save(b"\r\n");
        Flow::Go
    }

    fn on_part_data_end(&mut self) -> Flow {
        self.multipart_header_parsed = false;
        let form = form_of(self);
        // Dropping the file writes what is buffered and closes it.
        if form.fp.take().is_some() {
            self.save(b"\r\nSwoole-Upload-File");
        }
        self.save(b"\r\n");
        Flow::Go
    }

    fn on_part_data_begin(&mut self, boundary: &[u8]) -> Flow {
        form_of(self).header_name.clear();
        self.save(boundary);
        self.save(b"\r\n");
        Flow::Go
    }

    fn on_body_end(&mut self, boundary: &[u8]) -> Flow {
        self.save(boundary);
        self.save(b"--");
        let length = form_of(self).saved_length;
        self.content_length = (length as u64).wrapping_sub(u64::from(self.header_length));
        self.tried_to_dispatch = true;
        Flow::Go
    }
}

/// A request with no host, which is all zero bytes. Not inlined: built in place it
/// lets the compiler ask the allocator for zeroed memory, and glibc's `calloc`
/// skips its per-thread cache, at several hundred instructions a request.
#[inline(never)]
fn blank() -> HttpRequest {
    HttpRequest::default()
}

#[must_use]
pub fn request_new() -> Box<HttpRequest> {
    Box::new(blank())
}

/// `delete request` of one [`request_new`] made, if any: the destructor, then the memory.
pub fn request_free(request: Option<Box<HttpRequest>>) {
    if let Some(mut request) = request {
        request.destroy_multipart_parser();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SW_OK: i32 = swoole_rs_corebase::constants::ResultCode::SW_OK as i32;
    const SW_ERR: i32 = swoole_rs_corebase::constants::ResultCode::SW_ERR as i32;

    /// The ported tests' `SW_OK` or `SW_ERR` for a parse step's result.
    fn code(done: bool) -> i32 {
        if done { SW_OK } else { SW_ERR }
    }
    use swoole_rs_buffer::string::{delete_string, new_sized_string};

    /// A receive buffer holding `text`, with room to grow: a `swoole::String` the
    /// request points at, read back through the same pointer.
    struct Input(*mut CppString);

    impl Input {
        fn get(&self) -> &CppString {
            unsafe { &*self.0 }
        }

        fn get_mut(&mut self) -> &mut CppString {
            unsafe { &mut *self.0 }
        }

        fn bytes(&self) -> &[u8] {
            self.get().view()
        }
    }

    impl Drop for Input {
        fn drop(&mut self) {
            unsafe { delete_string(self.0) };
        }
    }

    fn input(text: &[u8]) -> Input {
        let mut input = Input(new_sized_string(text.len() + 4096, None));
        input.get_mut().append(text);
        input
    }

    fn request(input: &mut Input) -> HttpRequest {
        HttpRequest {
            buffer: input.0,
            ..HttpRequest::default()
        }
    }

    fn url(request: &HttpRequest, input: &Input) -> Vec<u8> {
        let at = request.url_offset as usize;
        input.bytes()[at..at + request.url_length as usize].to_vec()
    }

    fn parsed(text: &[u8]) -> (Input, HttpRequest) {
        let mut i = input(text);
        let mut r = request(&mut i);
        assert_eq!(code(r.get_protocol()), SW_OK, "request line");
        assert_eq!(code(r.get_header_length()), SW_OK, "header block");
        r.parse_header_info();
        (i, r)
    }

    // core-tests/src/server/http_server.cpp: get_protocol.
    #[test]
    fn get_protocol() {
        // `SW_HTTP_GET`, `_POST`, `_PUT`, `_PRI`; `SW_HTTP_VERSION_10`, `_11`.
        let (get, post, put, pri, v10, v11) = (2, 4, 5, 27, 1, 2);
        let protocol = |text: &[u8]| {
            let mut i = input(text);
            let mut r = request(&mut i);
            let status = code(r.get_protocol());
            let url = url(&r, &i);
            (status, r, url, i.get().offset())
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
                assert_eq!((code(r.get_protocol()), r.excepted), (SW_ERR, true), "PRI");
            } else {
                assert_eq!(
                    (code(r.get_protocol()), c_int::from(r.method)),
                    (SW_OK, i),
                    "{i}"
                );
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
            &i.bytes()[host.offset..host.offset + host.length],
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
    fn chunked(body: &[u8], max: u32) -> (i32, HttpRequest, Input) {
        let head = b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n";
        let (i, mut r) = parsed(&[&head[..], body].concat());
        r.max_length = max;
        (code(r.get_chunked_body_length()), r, i)
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
            (SW_ERR, ChunkState::Data, 3)
        );
        i.get_mut().append(b"c\r\n0\r\n\r\n");
        assert_eq!(code(r.get_chunked_body_length()), SW_OK, "complete");
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
        let dir = std::env::temp_dir();
        assert!(
            r.init_multipart_parser(dir.as_os_str().as_bytes(), 100),
            "initialized"
        );
        assert_eq!(
            i.get().len(),
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
        let body = &i.bytes()[head.len()..];
        let marker = b"Swoole-Upload-File: ";
        let at = body
            .windows(marker.len())
            .position(|w| w == marker)
            .unwrap();
        let name = body[at + marker.len()..]
            .split(|&b| b == b'\r')
            .next()
            .unwrap();
        let template = [dir.as_os_str().as_bytes(), b"/swoole.upfile."].concat();
        assert!(
            name.starts_with(&template) && name.len() == template.len() + 6,
            "{}",
            String::from_utf8_lossy(name)
        );
        let expected = [
            &b"--AB\r\nContent-Disposition: form-data; name=\"a\"\r\n\r\n1\r\n"[..],
            b"--AB\r\nContent-Disposition: form-data; name=\"f\"; filename=\"x.txt\"\r\n",
            marker,
            name,
            b"\r\n\r\nSwoole-Upload-File\r\n--AB--",
        ]
        .concat();
        assert_eq!(
            String::from_utf8_lossy(body),
            String::from_utf8_lossy(&expected),
            "rewritten body"
        );
        let name = OsStr::from_bytes(name);
        assert_eq!(
            std::fs::read(name).unwrap(),
            b"file",
            "the upload went to its file"
        );
        std::fs::remove_file(name).unwrap();
        assert_eq!(r.content_length, expected.len() as u64, "body length");
        assert!(
            r.tried_to_dispatch && !r.multipart_header_parsed,
            "ready to dispatch"
        );
        r.destroy_multipart_parser();
        assert!(
            r.form.is_none() && r.buffer == i.0,
            "the receive buffer is back"
        );
    }

    /// Through `parse_multipart_data`, a body arriving after the header is parsed
    /// from the form's own buffer, and an upload left open is removed.
    #[test]
    fn parsed_bodies_go_through_the_parser_and_open_uploads_are_removed() {
        let head = b"POST / HTTP/1.1\r\nContent-Type: multipart/form-data; boundary=b\r\n\r\n";
        let (i, mut r) = parsed(head);
        r.max_length = 1 << 20;
        let dir = std::env::temp_dir();
        assert!(r.init_multipart_parser(dir.as_os_str().as_bytes(), 100));
        assert_ne!(r.buffer, i.0, "the body has its own buffer");
        unsafe { &mut *r.buffer }
            .append(b"--b\r\nContent-Disposition: form-data; name=\"f\"; filename=\"x\"\r\n\r\nfi");
        assert!(r.parse_multipart_data(), "parsed");
        assert!(
            unsafe { &*r.buffer }.is_empty(),
            "the body buffer is cleared"
        );
        let upload = r.form.as_deref().unwrap().upload.clone();
        let name = OsStr::from_bytes(upload.split(|&b| b == 0).next().unwrap());
        assert!(std::fs::exists(name).unwrap(), "the upload is open");
        r.destroy_multipart_parser();
        assert_eq!(r.buffer, i.0, "the receive buffer is back");
        assert!(
            !std::fs::exists(name).unwrap(),
            "the unfinished upload is gone"
        );
    }

    #[test]
    fn oversize_parts_and_uploads_are_rejected() {
        let (_, mut r) =
            parsed(b"POST / HTTP/1.1\r\nContent-Type: multipart/form-data; boundary=AB\r\n\r\n");
        let saved = input(b"0123456789");
        if let Some(form) = r.form.as_deref_mut() {
            (form.saved, form.saved_length, form.upload_max) = (saved.0, 10, 3);
        }
        r.max_length = 12;
        assert_eq!(r.on_part_data(b"abc"), Flow::Pause, "rejected");
        assert!(
            r.excepted && r.unavailable && !r.too_large,
            "a field past max_length"
        );
        (r.excepted, r.unavailable) = (false, false);
        if let Some(form) = r.form.as_deref_mut() {
            let null = std::fs::OpenOptions::new().write(true).open("/dev/null");
            form.fp = Some(BufWriter::new(null.unwrap()));
        }
        assert_eq!(r.on_part_data(b"abcd"), Flow::Pause, "rejected");
        assert!(
            r.excepted && r.too_large,
            "an upload past upload_max_filesize"
        );
        r.form = None;
    }

    /// `http_upload_provenance.cpp`'s `MultipartFixture`: a request whose form has
    /// boundary `b`, fed through the multipart parser as the reactor feeds it.
    struct Fixture {
        input: Input,
        request: Box<HttpRequest>,
    }

    impl Fixture {
        fn new() -> Self {
            // The header buffer holds the boundary parameter, as the reactor's does.
            let boundary = b"boundary=b";
            let mut input = input(boundary);
            let mut request = Box::new(request(&mut input));
            request.max_length = u32::try_from(BUFFER_SIZE_BIG).unwrap();
            request.header_length = u32::try_from(boundary.len()).unwrap();
            request.form = Some(Box::new(HttpForm {
                boundary: HttpSpan {
                    offset: 0,
                    length: boundary.len(),
                },
                parser: None,
                fp: None,
                body: None,
                saved: ptr::null_mut(),
                saved_length: 0,
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
            unsafe { &mut *self.request.buffer }.append(data);
            self.request.parse_multipart_data()
        }

        fn forwarded(&self) -> String {
            String::from_utf8_lossy(self.input.bytes()).to_ascii_lowercase()
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
