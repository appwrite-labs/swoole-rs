//! `http_server::Request`, the HTTP/1 request a `ListenPort` parses (#931), and every
//! operation over it. Its `FormData` is a Rust [`HttpForm`]. Its effects
//! (`swoole::String` allocation, the upload's file and `swoole_tmpfile`) are direct
//! calls, which tests run too. Its logs go through corebase's `log`. Trace logs are
//! compiled out, as in upstream's builds without `--enable-trace-log`.
use crate::multipart_parser::{Flow, MultipartParser, Parts};
use crate::{
    ChunkState, HttpChunked, HttpChunks, HttpFrame, HttpHeaderInfo, HttpSpan, MultipartError,
    case_equal,
};
use core::ffi::c_int;
use std::fs::File;
use std::io::{BufWriter, Write};
use swoole_rs_corebase::buffer::BoundedBytes;
use swoole_rs_corebase::buffer::string::{StringBuffer, boxed_sized_string};
use swoole_rs_corebase::log::LogLevel::{SW_LOG_NOTICE as NOTICE, SW_LOG_WARNING as WARNING};
use swoole_rs_corebase::ossupport::TempFile;
use swoole_rs_corebase::{log, set_errno};

/// `SW_LOG_NOTICE`, `SW_LOG_WARNING` and `SW_ERROR_SERVER_INVALID_REQUEST`.
const INVALID_REQUEST: c_int = swoole_rs_error::ErrorCode::SW_ERROR_SERVER_INVALID_REQUEST as c_int;

use swoole_rs_corebase::sys::{errno, strtoul};

/// The errno libc's `strtoull` leaves when no digits convert; libcs differ. Its
/// `strtoul` shares the conversion (both are 64-bit on every target).
fn no_digits_errno() -> c_int {
    static VALUE: std::sync::OnceLock<c_int> = std::sync::OnceLock::new();
    *VALUE.get_or_init(|| {
        let saved = errno();
        set_errno(0);
        _ = strtoul(c"x", 10);
        let value = errno();
        set_errno(saved);
        value
    })
}

/// `SW_HTTP_UPLOAD_FILE`.
const UPLOAD_FILE: &[u8] = b"Swoole-Upload-File";
/// `SW_BUFFER_SIZE_BIG`.
const BUFFER_SIZE_BIG: usize = swoole_rs_corebase::constants::SW_BUFFER_SIZE_BIG as usize;

/// Maximum number of temporary uploads held until a single request is dispatched.
const MAX_UPLOAD_FILES: usize = 256;

/// A multipart body being streamed: upstream's `FormData`.
pub struct HttpForm {
    /// The `Content-Type` value after `multipart/form-data`, in the header buffer.
    boundary: HttpSpan,
    parser: Option<MultipartParser>,
    /// The current upload's file, buffered as the `FILE` upstream wrote was; none
    /// between uploads.
    fp: Option<TempFile<BufWriter<File>>>,
    /// Completed parts stay owned until their rewritten request reaches a worker.
    uploads: Vec<TempFile<()>>,
    /// The body's own buffer, which receives go into and the parser reads from
    /// `init_multipart_parser` until `destroy_multipart_parser`, while the body is
    /// rewritten into the receive buffer.
    body: Option<Box<StringBuffer>>,
    /// `upload_tmp_dir + "/swoole.upfile.XXXXXX"`.
    template: Vec<u8>,
    /// The part header's name, copied: it may arrive split across reads.
    header_name: BoundedBytes,
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
            chunk_length: 0,
            chunk_offset: 0,
            chunk_scan_offset: 0,
        }
    }
}

/// The parse calls take the receive buffer the request is read into (upstream's
/// `buffer_`), which the connection owns; the readers take its bytes.
impl HttpRequest {
    /// `memset(&method, 0, offsetof(HttpRequest, form_data_))`: the form stays.
    pub fn clean(&mut self) {
        let form = self.form.take();
        *self = Self {
            form,
            ..Self::default()
        };
    }

    /// The multipart body's own buffer, from `init_multipart_parser` until
    /// `destroy_multipart_parser`: what is received goes into it, not the receive buffer.
    pub fn body(&mut self) -> Option<&mut StringBuffer> {
        self.form.as_deref_mut()?.body.as_deref_mut()
    }

    fn frame(
        &mut self,
        buffer: &mut StringBuffer,
        parse: fn(&mut HttpFrame, &[u8]) -> bool,
    ) -> bool {
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
        let found = parse(&mut frame, buffer.view());
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
    pub fn get_protocol(&mut self, buffer: &mut StringBuffer) -> bool {
        self.frame(buffer, HttpFrame::request_line)
    }

    /// Finds the `\r\n\r\n` ending the header block.
    pub fn get_header_length(&mut self, buffer: &mut StringBuffer) -> bool {
        self.frame(buffer, HttpFrame::header_end)
    }

    /// `Content-Length`, `Connection`, `Transfer-Encoding` and a multipart `Content-Type`.
    pub fn parse_header_info(&mut self, input: &[u8]) {
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
            let status = crate::header_info(input, end, &mut state);
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
                uploads: Vec::new(),
                body: None,
                template: Vec::new(),
                header_name: BoundedBytes::new(0, 0),
                upload_size: 0,
                upload_max: 0,
            }));
        }
    }

    /// Scans the chunked body from where the last call stopped: true once its
    /// trailer section ended, with `content_length` up to there; false to wait for
    /// more, or with `excepted` (bad framing) or `too_large` (past `max_length`) set.
    pub fn get_chunked_body_length(&mut self, input: &[u8]) -> bool {
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
    pub fn has_expect_header(&self, input: &[u8]) -> bool {
        crate::has_expect_header(input)
    }

    /// The value of header `name`, as a span of the buffer.
    #[must_use]
    pub fn get_header(&self, input: &[u8], name: &[u8]) -> Option<HttpSpan> {
        let start = self.url_offset as usize + self.url_length as usize + 10;
        crate::header_value(input, start, self.header_length as usize, name)
    }

    /// The request's URL, as the buffer's bytes.
    #[must_use]
    pub fn url_bytes<'a>(&self, input: &'a [u8]) -> &'a [u8] {
        let start = self.url_offset as usize;
        input
            .get(start..start + self.url_length as usize)
            .unwrap_or_default()
    }

    /// The value of header `name`, as the buffer's bytes.
    #[must_use]
    pub fn header_bytes<'a>(&self, input: &'a [u8], name: &[u8]) -> Option<&'a [u8]> {
        let span = self.get_header(input, name)?;
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

    /// `Request::init_multipart_parser`: a parser over the form's boundary in `buffer`,
    /// the receive buffer, and the body moved to the form's own buffer, `buffer` cut back
    /// to the header for the body to be rewritten into. False without a form.
    pub fn init_multipart_parser(
        &mut self,
        buffer: &mut StringBuffer,
        tmp_dir: &[u8],
        upload_max: usize,
    ) -> bool {
        let header = self.header_length as usize;
        let Some(form) = self.form.as_deref_mut() else {
            return false;
        };
        // The header buffer the boundary was found in.
        let input = buffer.view();
        let text = input
            .get(form.boundary.offset..)
            .and_then(|text| text.get(..form.boundary.length));
        let Some(text) = text else {
            return false;
        };
        let Ok(Some(boundary)) = crate::boundary(text, 0) else {
            return false;
        };
        let at = boundary.offset..boundary.offset + boundary.length;
        let Some(parser) = text.get(at).and_then(MultipartParser::new) else {
            return false;
        };
        form.header_name = BoundedBytes::new(self.max_length as usize, 0);
        form.parser = Some(parser);
        let mut body = boxed_sized_string(BUFFER_SIZE_BIG, None);
        body.append(input.get(header..).unwrap_or_default());
        buffer.truncate(header);
        form.body = Some(body);
        form.template = [tmp_dir, b"/swoole.upfile.XXXXXX\0"].concat();
        form.upload_max = upload_max;
        true
    }

    /// Free the parser and unfinished upload. The caller owns the completed files
    /// until the rewritten request has been dispatched.
    pub fn destroy_multipart_parser(&mut self) -> Vec<TempFile<()>> {
        let Some(form) = self.form.take() else {
            return Vec::new();
        };
        if let Some(mut body) = form.body {
            body.dispose();
        }
        form.uploads
    }

    /// `Request::parse_multipart_data(buffer_)`: feeds the body received since
    /// `init_multipart_parser` to the parser, whose callbacks rewrite it into `saved`,
    /// the receive buffer. False rejects the request.
    pub fn parse_multipart_data(&mut self, saved: &mut StringBuffer) -> bool {
        self.excepted = false;
        let Some(form) = self.form.as_deref_mut() else {
            return false;
        };
        // `init_multipart_parser` made both; only the form's drop takes them.
        let (Some(mut parser), Some(mut body)) = (form.parser.take(), form.body.take()) else {
            return false;
        };
        let length = body.len();
        let processed = parser.execute(
            &mut Rewrite {
                request: self,
                saved,
            },
            body.view(),
        );
        let reason = parser.error_message();
        let result = crate::multipart_result(processed, length, self.excepted);
        if result.is_ok() {
            body.clear();
        }
        let form = form_of(self);
        (form.parser, form.body) = (Some(parser), Some(body));
        match result {
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
            Ok(()) => true,
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

/// The multipart parser's callbacks over a request, which rewrite its body into the
/// receive buffer, `saved`.
struct Rewrite<'a> {
    request: &'a mut HttpRequest,
    saved: &'a mut StringBuffer,
}

impl Parts<'_> for Rewrite<'_> {
    fn on_header_field(&mut self, data: &[u8]) -> Flow {
        let r = &mut *self.request;
        if form_of(r).header_name.append(data).is_err() {
            (r.excepted, r.too_large) = (true, true);
            return Flow::Pause;
        }
        Flow::Go
    }

    fn on_header_value(&mut self, data: &[u8]) -> Flow {
        let form = form_of(self.request);
        // The value callback means the name is complete.
        let name = form.header_name.as_ref();
        // Only the reactor may write this header; a client never names a part header so.
        if case_equal(name, UPLOAD_FILE) {
            form.header_name.clear();
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
        for part in [name, b": ", data, b"\r\n"] {
            self.saved.append(part);
        }
        let disposition = case_equal(name, b"content-disposition");
        form.header_name.clear();
        if !disposition || !crate::multipart_has_filename(data) {
            return Flow::Go;
        }
        let form = form_of(self.request);
        if form.uploads.len() >= MAX_UPLOAD_FILES {
            (self.request.excepted, self.request.too_large) = (true, true);
            return Flow::Pause;
        }
        let mut template = form.template.clone();
        form.upload_size = 0;
        // `swoole_tmpfile` fills in the template's `XXXXXX`.
        let Some(fd) = TempFile::create(&mut template) else {
            self.request.excepted = true;
            return Flow::Go;
        };
        form.fp = Some(fd.map_file(BufWriter::new));
        Flow::Go
    }

    fn on_part_data(&mut self, data: &[u8]) -> Flow {
        let r = &mut *self.request;
        let max_length = u64::from(r.max_length);
        let length = data.len();
        let form = form_of(r);
        let Some(fp) = form.fp.as_mut() else {
            if (self.saved.len() as u64).wrapping_add(length as u64) > max_length {
                (r.excepted, r.unavailable) = (true, true);
                return Flow::Pause;
            }
            self.saved.append(data);
            return Flow::Go;
        };
        form.upload_size = form.upload_size.wrapping_add(length);
        if form.upload_size > form.upload_max {
            (r.excepted, r.too_large) = (true, true);
            return Flow::Pause;
        }
        if fp.file.write_all(data).is_err() {
            form.fp = None;
            (r.excepted, r.unavailable) = (true, true);
            log::sys_warning(
                c"http_server::multipart_on_data",
                format_args!("failed to write upload file"),
            );
            return Flow::Pause;
        }
        Flow::Go
    }

    fn on_headers_complete(&mut self) -> Flow {
        let form = form_of(self.request);
        form.header_name.clear();
        if let Some(upload) = &form.fp {
            self.saved.append(b"Swoole-Upload-File: ");
            // The name up to its NUL: `swoole_tmpfile` wrote no other.
            self.saved.append(upload.path().to_bytes());
        }
        self.request.multipart_header_parsed = true;
        self.saved.append(b"\r\n");
        Flow::Go
    }

    fn on_part_data_end(&mut self) -> Flow {
        self.request.multipart_header_parsed = false;
        // Dropping the file writes what is buffered and closes it.
        if let Some(mut upload) = form_of(self.request).fp.take() {
            if upload.file.flush().is_err() {
                (self.request.excepted, self.request.unavailable) = (true, true);
                return Flow::Pause;
            }
            #[cfg_attr(
                protocol_buffer_lint,
                expect(
                    clippy::disallowed_methods,
                    reason = "a file is opened only below MAX_UPLOAD_FILES and taken once at part end"
                )
            )]
            form_of(self.request).uploads.push(upload.map_file(drop));
            self.saved.append(b"\r\nSwoole-Upload-File");
        }
        self.saved.append(b"\r\n");
        Flow::Go
    }

    fn on_part_data_begin(&mut self, boundary: &[u8]) -> Flow {
        form_of(self.request).header_name.clear();
        self.saved.append(boundary);
        self.saved.append(b"\r\n");
        Flow::Go
    }

    fn on_body_end(&mut self, boundary: &[u8]) -> Flow {
        self.saved.append(boundary);
        self.saved.append(b"--");
        let r = &mut *self.request;
        r.content_length = (self.saved.len() as u64).wrapping_sub(u64::from(r.header_length));
        r.tried_to_dispatch = true;
        Flow::Go
    }
}

/// A request with no host, which is all zero bytes: written apart, its allocation stays
/// `malloc`, not `calloc` (corebase's `memory::apart`).
#[must_use]
pub fn request_new() -> Box<HttpRequest> {
    swoole_rs_corebase::memory::apart::boxed(HttpRequest::default)
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
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    const SW_OK: i32 = swoole_rs_corebase::constants::ResultCode::SW_OK as i32;
    const SW_ERR: i32 = swoole_rs_corebase::constants::ResultCode::SW_ERR as i32;

    /// The ported tests' `SW_OK` or `SW_ERR` for a parse step's result.
    fn code(done: bool) -> i32 {
        if done { SW_OK } else { SW_ERR }
    }
    /// A receive buffer holding `text`, with room to grow.
    struct Input(Box<StringBuffer>);

    impl Input {
        fn get(&self) -> &StringBuffer {
            &self.0
        }

        fn get_mut(&mut self) -> &mut StringBuffer {
            &mut self.0
        }

        fn bytes(&self) -> &[u8] {
            self.get().view()
        }
    }

    impl Drop for Input {
        fn drop(&mut self) {
            self.0.dispose();
        }
    }

    fn input(text: &[u8]) -> Input {
        let mut input = Input(boxed_sized_string(text.len() + 4096, None));
        input.get_mut().append(text);
        input
    }

    fn url(request: &HttpRequest, input: &Input) -> Vec<u8> {
        let at = request.url_offset as usize;
        input.bytes()[at..at + request.url_length as usize].to_vec()
    }

    fn parsed(text: &[u8]) -> (Input, HttpRequest) {
        let mut i = input(text);
        let mut r = HttpRequest::default();
        assert_eq!(code(r.get_protocol(i.get_mut())), SW_OK, "request line");
        assert_eq!(
            code(r.get_header_length(i.get_mut())),
            SW_OK,
            "header block"
        );
        r.parse_header_info(i.bytes());
        (i, r)
    }

    // core-tests/src/server/http_server.cpp: get_protocol.
    #[test]
    fn get_protocol() {
        // `SW_HTTP_GET`, `_POST`, `_PUT`, `_PRI`; `SW_HTTP_VERSION_10`, `_11`.
        let (get, post, put, pri, v10, v11) = (2, 4, 5, 27, 1, 2);
        let protocol = |text: &[u8]| {
            let mut i = input(text);
            let mut r = HttpRequest::default();
            let status = code(r.get_protocol(i.get_mut()));
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
            let Some(name) = crate::method_string(i) else {
                assert!(i == 0 || i > pri, "{i}");
                continue;
            };
            assert!(i != 0 && i <= pri, "{i}");
            let name = name.to_bytes();
            let mut i_ = input(&[name, b" / HTTP/1.1\r\nHost: example.com\r\n\r\n"].concat());
            let mut r = HttpRequest::default();
            let status = code(r.get_protocol(i_.get_mut()));
            if i == pri {
                assert_eq!((status, r.excepted), (SW_ERR, true), "PRI");
            } else {
                assert_eq!((status, c_int::from(r.method)), (SW_OK, i), "{i}");
            }
        }
    }

    // core-tests/src/server/http_server.cpp: has_expect_header.
    #[test]
    fn has_expect_header() {
        let r = HttpRequest::default();
        let i = input(b"HTTP/1.1 200 OK\r\nCache-Control: no-cache\r\nConnection: close\r\nContent-Length: 0\r\nContent-Type: text/html\r\nPragma: no-cache\r\n\r\n");
        assert!(!r.has_expect_header(i.bytes()), "no Expect");
        let i = input(b"POST /submit HTTP/1.1\r\nHost: www.example.com\r\nContent-Type: application/json\r\nContent-Length: 1000000\r\nExpect: 100-continue\r\n\r\n");
        assert!(r.has_expect_header(i.bytes()), "Expect: 100-continue");
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
        let host = r.get_header(i.bytes(), b"Host").unwrap();
        assert_eq!(
            &i.bytes()[host.offset..host.offset + host.length],
            b"a",
            "Host"
        );
        assert!(
            r.get_header(i.bytes(), b"Missing").is_none(),
            "absent header"
        );

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
        (code(r.get_chunked_body_length(i.bytes())), r, i)
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
        assert_eq!(
            code(r.get_chunked_body_length(i.bytes())),
            SW_OK,
            "complete"
        );
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
    fn clean_keeps_the_form() {
        let (_, mut r) = parsed(b"POST / HTTP/1.1\r\nContent-Type: multipart/form-data; boundary=x\r\nContent-Length: 9\r\n\r\n");
        assert!(r.form.is_some() && r.known_length, "a form");
        r.clean();
        assert!(r.form.is_some(), "form kept");
        assert!(
            !r.known_length && r.content_length == 0 && r.method == 0,
            "facts reset"
        );
        r.form = None;
    }

    /// One part header through the callbacks.
    fn field(parts: &mut Rewrite<'_>, name: &[u8], value: &[u8]) {
        parts.on_header_field(name);
        parts.on_header_value(value);
    }

    /// Streams a two-part body, one part a file, through the callbacks.
    #[test]
    fn multipart_bodies_are_rewritten_with_uploads_on_disk() {
        let head = b"POST / HTTP/1.1\r\nContent-Type: multipart/form-data; boundary=AB\r\nContent-Length: 999\r\n\r\n";
        let (mut i, mut r) = parsed(head);
        r.max_length = 1 << 20;
        let dir = std::env::temp_dir();
        assert!(
            r.init_multipart_parser(i.get_mut(), dir.as_os_str().as_bytes(), 100),
            "initialized"
        );
        assert_eq!(
            i.get().len(),
            head.len(),
            "the receive buffer keeps the header"
        );
        let mut parts = Rewrite {
            request: &mut r,
            saved: i.get_mut(),
        };
        parts.on_part_data_begin(b"--AB");
        field(&mut parts, b"Content-Disposition", b"form-data; name=\"a\"");
        parts.on_headers_complete();
        parts.on_part_data(b"1");
        parts.on_part_data_end();
        parts.on_part_data_begin(b"--AB");
        field(
            &mut parts,
            b"Content-Disposition",
            b"form-data; name=\"f\"; filename=\"x.txt\"",
        );
        parts.on_headers_complete();
        parts.on_part_data(b"file");
        parts.on_part_data_end();
        parts.on_body_end(b"--AB");
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
        assert!(r.form.is_none(), "the form is gone");
    }

    /// Through `parse_multipart_data`, a body arriving after the header is parsed
    /// from the form's own buffer, and an upload left open is removed.
    #[test]
    fn parsed_bodies_go_through_the_parser_and_open_uploads_are_removed() {
        let head = b"POST / HTTP/1.1\r\nContent-Type: multipart/form-data; boundary=b\r\n\r\n";
        let (mut i, mut r) = parsed(head);
        r.max_length = 1 << 20;
        let dir = std::env::temp_dir();
        assert!(r.init_multipart_parser(i.get_mut(), dir.as_os_str().as_bytes(), 100));
        let body = r.body().expect("the body has its own buffer");
        body.append(b"--b\r\nContent-Disposition: form-data; name=\"done\"; filename=\"first\"\r\n\r\ndone\r\n--b\r\nContent-Disposition: form-data; name=\"f\"; filename=\"x\"\r\n\r\nfi");
        assert!(r.parse_multipart_data(i.get_mut()), "parsed");
        assert!(
            r.body().is_some_and(|body| body.is_empty()),
            "the body buffer is cleared"
        );
        let form = r.form.as_deref().unwrap();
        let names = [form.uploads[0].path(), form.fp.as_ref().unwrap().path()]
            .map(|name| OsStr::from_bytes(name.to_bytes()).to_owned());
        assert!(names.iter().all(|name| std::fs::exists(name).unwrap()));
        r.destroy_multipart_parser();
        assert!(r.body().is_none(), "the body buffer is gone");
        assert!(
            names.iter().all(|name| !std::fs::exists(name).unwrap()),
            "both completed and unfinished uploads are gone"
        );
    }

    #[test]
    fn file_count_limit_applies_before_opening_another_upload() {
        let (mut saved, mut request) =
            parsed(b"POST / HTTP/1.1\r\nContent-Type: multipart/form-data; boundary=b\r\n\r\n");
        request.max_length = 1 << 20;
        let dir = std::env::temp_dir();
        assert!(request.init_multipart_parser(saved.get_mut(), dir.as_os_str().as_bytes(), 100));
        let mut parts = Rewrite {
            request: &mut request,
            saved: saved.get_mut(),
        };
        for _ in 0..MAX_UPLOAD_FILES {
            parts.on_header_field(b"Content-Disposition");
            assert_eq!(
                parts.on_header_value(b"form-data; filename=\"f\""),
                Flow::Go
            );
            assert_eq!(parts.on_part_data_end(), Flow::Go);
        }
        parts.on_header_field(b"Content-Disposition");
        assert_eq!(parts.on_header_value(b"form-data; name=\"text\""), Flow::Go);
        parts.on_header_field(b"Content-Disposition");
        assert_eq!(
            parts.on_header_value(b"form-data; filename=\"f\""),
            Flow::Pause
        );
        assert!(request.excepted && request.too_large);
        assert!(form_of(&mut request).fp.is_none());
        assert_eq!(request.destroy_multipart_parser().len(), MAX_UPLOAD_FILES);
    }

    #[test]
    fn oversize_parts_and_uploads_are_rejected() {
        let (_, mut r) =
            parsed(b"POST / HTTP/1.1\r\nContent-Type: multipart/form-data; boundary=AB\r\n\r\n");
        let mut saved = input(b"0123456789");
        let mut on_part_data = |request: &mut HttpRequest, data: &[u8]| {
            let saved = saved.get_mut();
            Rewrite { request, saved }.on_part_data(data)
        };
        if let Some(form) = r.form.as_deref_mut() {
            form.upload_max = 3;
        }
        r.max_length = 12;
        assert_eq!(on_part_data(&mut r, b"abc"), Flow::Pause, "rejected");
        assert!(
            r.excepted && r.unavailable && !r.too_large,
            "a field past max_length"
        );
        (r.excepted, r.unavailable) = (false, false);
        if let Some(form) = r.form.as_deref_mut() {
            let file = TempFile::create(&mut b"/tmp/swr-http-XXXXXX\0".to_owned()).unwrap();
            form.fp = Some(file.map_file(BufWriter::new));
        }
        assert_eq!(on_part_data(&mut r, b"abcd"), Flow::Pause, "rejected");
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
        request: HttpRequest,
    }

    impl Fixture {
        fn new() -> Self {
            let (mut input, mut request) =
                parsed(b"POST / HTTP/1.1\r\nContent-Type: multipart/form-data; boundary=b\r\n\r\n");
            request.max_length = u32::try_from(BUFFER_SIZE_BIG).unwrap();
            assert!(request.init_multipart_parser(input.get_mut(), b"/tmp", 1024 * 1024));
            Self { input, request }
        }

        /// Like the reactor, the parse buffer is cleared after every read and refilled from its start.
        fn feed(&mut self, data: &[u8]) -> bool {
            self.request.body().expect("a body buffer").append(data);
            self.request.parse_multipart_data(self.input.get_mut())
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

    #[test]
    fn multipart_header_name_limit_spans_reads() {
        let mut f = Fixture::new();
        assert!(f.feed(b"--b\r\n"));
        let name = vec![b'X'; BUFFER_SIZE_BIG - 1];
        assert!(f.feed(&name));
        assert!(f.feed(b"Y"));
        assert!(!f.request.too_large && !f.request.excepted);
        assert!(f.feed(b"Z"));
        assert!(f.request.too_large && f.request.excepted);
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
