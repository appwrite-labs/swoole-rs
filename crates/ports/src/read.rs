//! `ListenPort`'s readable callbacks (raw, length, EOF, Redis and HTTP) as whole
//! operations. Native code keeps the socket, connection, traces and dispatch, and
//! performs each effect through one `call(context, op, arg)`. The receive buffer and
//! the HTTP request are read and written in place through [`Shared`], the head of the
//! context (layouts pinned by native `static_assert`s), as upstream's reader did; the
//! request's own operations are `swoole_rs_http`'s.
use core::ffi::{CStr, c_char, c_int, c_void};
use swoole_rs_corebase::log;
use swoole_rs_error::ErrorCode;
use swoole_rs_http::{HttpRequest as Request, HttpString as Buffer};

/// One native fact or effect on the connection in `context`.
pub type PortsCall = unsafe extern "C" fn(context: *mut c_void, op: u32, arg: u64) -> u64;

/// Which readable callback runs.
pub const ON_READ_RAW: u32 = 0;
pub const ON_READ_LENGTH: u32 = 1;
pub const ON_READ_EOF: u32 = 2;
pub const ON_READ_REDIS: u32 = 3;
pub const ON_READ_HTTP: u32 = 4;

// Socket and connection.
/// `recv` into the buffer: arg 0 its free tail, 1 the whole buffer. Returns the `ssize_t`.
pub const OP_RECV: u32 = 0;
/// `catch_read_error(errno)`, the raw `swReturnCode`.
pub const OP_CATCH: u32 = 1;
pub const OP_WARN_RECV: u32 = 2;
/// `conn->close_errno = errno`.
pub const OP_CLOSE_ERRNO: u32 = 3;
/// `reactor->trigger_close_event(event)`.
pub const OP_CLOSE: u32 = 4;
/// Sends a canned packet: `SEND_*`.
pub const OP_SEND: u32 = 5;
/// `serv->get_recv_buffer(socket)` becomes the current buffer; returns non-null.
pub const OP_RECV_BUFFER: u32 = 6;
/// Frees the socket's receive buffer when larger than twice `SW_BUFFER_SIZE_BIG`
/// (arg 1: and empty). Returns whether it did.
pub const OP_RELEASE: u32 = 7;
/// `protocol.recv_with_length_protocol`, `recv_with_eof_protocol`, `redis::recv_packet`.
pub const OP_RECV_LENGTH: u32 = 8;
pub const OP_RECV_EOF: u32 = 9;
pub const OP_RECV_REDIS: u32 = 10;
/// `Server::dispatch_task` of arg bytes from the buffer start.
pub const OP_DISPATCH_TASK: u32 = 11;
/// `http_server::dispatch_request` of arg bytes from the buffer start; the
/// connection's request is read afresh afterwards.
pub const OP_DISPATCH_REQUEST: u32 = 12;
/// `conn->active && !socket->removed`.
pub const OP_ACTIVE: u32 = 13;
/// Sets `websocket_status = STATUS_ACTIVE` and `http_upgrade = 1`.
pub const OP_WEBSOCKET_ACTIVATE: u32 = 17;
/// `open_http2_protocol`.
pub const OP_HTTP2_ENABLED: u32 = 18;
/// Sets `http2_stream` and sends the settings frame.
pub const OP_HTTP2_START: u32 = 19;
/// `socket->skip_recv = 1`.
pub const OP_SKIP_RECV: u32 = 20;
/// `protocol.package_max_length`, `serv->upload_max_filesize > 0`.
pub const OP_MAX_LENGTH: u32 = 21;
pub const OP_UPLOADS: u32 = 22;
/// The static handler served the request (asked only when it is enabled).
pub const OP_STATIC: u32 = 23;

// The current buffer.
pub const OP_BUF_REDUCE: u32 = 36;
pub const OP_BUF_EXTEND: u32 = 38;
/// The buffer holds `\r\n\r\n`.
pub const OP_BUF_HEADER_END: u32 = 39;

// The connection's HTTP request.
/// Creates the request when missing, gives it the receive buffer when it has none,
/// and makes it and its buffer current.
pub const OP_REQ_INIT: u32 = 50;
/// `port->destroy_http_request(conn)`; no request is current after it.
pub const OP_REQ_DESTROY: u32 = 51;
/// `init_multipart_parser(serv)`, then its buffer becomes current.
pub const OP_REQ_MULTIPART_INIT: u32 = 63;

/// The client a log names: arg 0 `conn->session_id`, 1 `port->host.c_str()`, 2 `port->port`.
pub const OP_CLIENT: u32 = 89;

/// `readable_callback_http`'s logs, in upstream's wording.
#[derive(Clone, Copy)]
enum Log {
    UnknownProtocol,
    UnknownMethod,
    NoHttp2,
    HeaderTooLarge,
    ChunkedError,
    ChunkedTooLarge,
    TooLarge,
    /// The bytes discarded.
    Discard(u64),
}
/// Trace logs, sent only when [`Shared::trace`] is set.
pub const TRACE_HEADER: u32 = 98;
pub const TRACE_CHUNKED: u32 = 99;
pub const TRACE_POST_WAIT: u32 = 100;
pub const TRACE_CLOSE: u32 = 101;

/// Canned responses for `OP_SEND`.
pub const SEND_BAD_REQUEST: u64 = 0;
pub const SEND_TOO_LARGE: u64 = 1;
pub const SEND_UNAVAILABLE: u64 = 2;
pub const SEND_CONTINUE: u64 = 3;

/// `swReturnCode` values `catch_read_error` returns.
const SW_CLOSE: u64 = 3;
const SW_ERROR: u64 = 4;
const SW_OK: i32 = 0;
const SW_ERR: i32 = -1;
/// `SW_HTTP_PRI`, `SW_HTTP_HEADER_MAX_SIZE`, `sizeof(SW_HTTP2_PRI_STRING) - 1`,
/// `sizeof(SW_HTTP_CHUNK_EOF) - 1`, `SW_BUFFER_SIZE_BIG`; native asserts each.
const HTTP_PRI: u8 = 27;
const HEADER_MAX_SIZE: u64 = 65536;
const HTTP2_PREFACE: u64 = 24;
const CHUNK_EOF: u32 = 5;
const BUFFER_SIZE_BIG: u64 = 65536;

/// The head of every read context (`SwrPortsShared`): the current buffer and request,
/// which native code repoints as its effects change them, and facts fixed for the call.
#[repr(C)]
pub struct Shared {
    pub buffer: *mut Buffer,
    pub request: *mut Request,
    /// `websocket_status >= STATUS_HANDSHAKE`, `http_upgrade`, `http2_stream` at entry.
    pub websocket: u8,
    pub upgraded: u8,
    pub http2_stream: u8,
    /// `enable_static_handler`.
    pub static_handler: u8,
    /// Trace logging is compiled in.
    pub trace: u8,
}

struct Native {
    shared: *mut Shared,
    call: PortsCall,
}

impl Native {
    fn op(&self, op: u32, arg: u64) -> u64 {
        // SAFETY: a native fact or effect on the live connection in `context`.
        unsafe { (self.call)(self.shared.cast(), op, arg) }
    }
    fn get(&self, op: u32) -> u64 {
        self.op(op, 0)
    }
    fn is(&self, op: u32) -> bool {
        self.get(op) != 0
    }
    fn signed(&self, op: u32, arg: u64) -> i64 {
        self.op(op, arg).cast_signed()
    }
    /// The context's head, borrowed until the next native call.
    fn shared(&self) -> &Shared {
        // SAFETY: the context starts with a live `Shared`, which only native
        // effects, never spanned by the borrow, repoint.
        unsafe { &*self.shared }
    }
    fn trace(&self, op: u32) {
        if self.shared().trace != 0 {
            self.get(op);
        }
    }
    /// The current buffer, borrowed until the next native call.
    #[expect(
        clippy::mut_from_ref,
        reason = "native memory: the borrow never spans a native call"
    )]
    fn buffer(&self) -> &mut Buffer {
        // SAFETY: the current buffer is live and only native effects, which the
        // borrow never spans, touch it.
        unsafe { &mut *self.shared().buffer }
    }
    /// The current request, borrowed until the next native call.
    #[expect(
        clippy::mut_from_ref,
        reason = "native memory: the borrow never spans a native call"
    )]
    fn request(&self) -> &mut Request {
        // SAFETY: after `OP_REQ_INIT` the connection's request is live and only
        // native effects, which the borrow never spans, touch it.
        unsafe { &mut *self.shared().request }
    }
    /// `buffer->offset = value`.
    fn set_offset(&self, value: u64) {
        self.buffer().offset = value.cast_signed();
    }
    /// `buffer->clear()`.
    fn clear(&self) {
        let buffer = self.buffer();
        (buffer.length, buffer.offset) = (0, 0);
    }
    /// The request's buffer becomes current.
    fn use_request_buffer(&self) {
        let buffer = self.request().buffer;
        // SAFETY: only native effects, which this write never spans, repoint the head.
        unsafe { (*self.shared).buffer = buffer };
    }
}

/// A byte count native code returned: a `size_t` or non-negative `ssize_t`.
fn size(count: u64) -> usize {
    usize::try_from(count).unwrap_or(usize::MAX)
}

/// `ListenPort::readable_callback_*`: `SW_OK`, or `SW_ERR` when an EOF port has no buffer.
///
/// # Safety
/// `context` starts with a [`Shared`] and `call` acts on the connection in it, live
/// for the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_ports_on_read(
    kind: u32,
    context: *mut c_void,
    call: PortsCall,
) -> i32 {
    let native = Native {
        shared: context.cast(),
        call,
    };
    match kind {
        ON_READ_RAW => raw(&native),
        ON_READ_LENGTH => framed(&native, OP_RECV_LENGTH, true),
        ON_READ_EOF => eof(&native),
        ON_READ_REDIS => framed(&native, OP_RECV_REDIS, false),
        ON_READ_HTTP => http(&native),
        _ => SW_ERR,
    }
}

/// A failed `recv`: warn and wait, or record errno and close.
enum Failure {
    Wait,
    Close,
}

fn read_error(n: &Native) -> Failure {
    match n.get(OP_CATCH) {
        SW_ERROR => {
            n.get(OP_WARN_RECV);
            Failure::Wait
        }
        SW_CLOSE => {
            n.get(OP_CLOSE_ERRNO);
            Failure::Close
        }
        _ => Failure::Wait,
    }
}

fn raw(n: &Native) -> i32 {
    n.get(OP_RECV_BUFFER);
    let received = n.signed(OP_RECV, 1);
    if received < 0 {
        if let Failure::Close = read_error(n) {
            n.get(OP_CLOSE);
        }
        return SW_OK;
    }
    if received == 0 {
        n.get(OP_CLOSE);
        return SW_OK;
    }
    let length = received.cast_unsigned();
    n.set_offset(length);
    n.buffer().length = size(length);
    i32::try_from(n.signed(OP_DISPATCH_TASK, length)).unwrap_or(SW_ERR)
}

/// Length and Redis framing; length (and EOF) ports free a large drained buffer.
fn framed(n: &Native, receive: u32, release: bool) -> i32 {
    n.get(OP_RECV_BUFFER);
    if n.signed(receive, 0) < 0 {
        if receive == OP_RECV_LENGTH {
            n.trace(TRACE_CLOSE);
        }
        n.get(OP_CLOSE_ERRNO);
        n.get(OP_CLOSE);
    }
    if release {
        n.op(OP_RELEASE, 1);
    }
    SW_OK
}

fn eof(n: &Native) -> i32 {
    if !n.is(OP_RECV_BUFFER) {
        n.get(OP_CLOSE);
        return SW_ERR;
    }
    if n.signed(OP_RECV_EOF, 0) < 0 {
        n.get(OP_CLOSE_ERRNO);
        n.get(OP_CLOSE);
    }
    n.op(OP_RELEASE, 1);
    SW_OK
}

/// Where the HTTP reader goes next; upstream's labels.
enum Step {
    Receive,
    Parse,
    Reply(u64),
    Close,
    Length,
    Done,
}

fn http(n: &Native) -> i32 {
    let shared = n.shared();
    let (websocket, upgraded, http2_stream) =
        (shared.websocket, shared.upgraded, shared.http2_stream);
    if websocket != 0 {
        if upgraded == 0 {
            n.get(OP_REQ_DESTROY);
            n.get(OP_WEBSOCKET_ACTIVATE);
        }
        return framed(n, OP_RECV_LENGTH, true);
    }
    if http2_stream != 0 {
        return framed(n, OP_RECV_LENGTH, true);
    }
    n.get(OP_REQ_INIT);
    let mut step = Step::Receive;
    loop {
        step = match step {
            Step::Receive => receive(n),
            Step::Parse => parse(n),
            Step::Reply(packet) => {
                n.op(OP_SEND, packet);
                Step::Close
            }
            Step::Close => {
                n.get(OP_REQ_DESTROY);
                n.get(OP_CLOSE);
                return SW_OK;
            }
            Step::Length => return framed(n, OP_RECV_LENGTH, true),
            Step::Done => return SW_OK,
        }
    }
}

fn receive(n: &Native) -> Step {
    let received = n.signed(OP_RECV, 0);
    if received < 0 {
        return match read_error(n) {
            Failure::Wait => Step::Done,
            Failure::Close => Step::Close,
        };
    }
    if received == 0 {
        return Step::Close;
    }
    n.buffer().length += size(received.cast_unsigned());
    Step::Parse
}

fn bad(n: &Native, which: Log) -> Step {
    log(n, which);
    Step::Reply(SEND_BAD_REQUEST)
}

/// `SW_LOG_TRACE` and `SW_LOG_WARNING`.
const TRACE: c_int = 1;
const WARNING: c_int = 4;

/// One of `readable_callback_http`'s `swoole_error_log`s, naming the client.
fn log(n: &Native, which: Log) {
    let host = n.op(OP_CLIENT, 1) as *const c_char;
    let host = if host.is_null() {
        c""
    } else {
        // SAFETY: the port's host, NUL-terminated and unchanged during the call.
        unsafe { CStr::from_ptr(host) }
    };
    let (session, port) = (n.op(OP_CLIENT, 0), n.op(OP_CLIENT, 2));
    let from = format!(
        " from session#{} on {}:{}",
        session.cast_signed(),
        host.to_string_lossy(),
        port.cast_signed()
    );
    let max = || n.get(OP_MAX_LENGTH);
    let (level, message) = match which {
        Log::UnknownProtocol => (TRACE, "Bad Request: unknown protocol".to_owned()),
        Log::UnknownMethod => (TRACE, "Bad Request: unknown HTTP method".to_owned()),
        Log::NoHttp2 => (
            TRACE,
            "Bad Request: can not handle HTTP2 request".to_owned(),
        ),
        Log::HeaderTooLarge => (
            TRACE,
            "Bad Request: request header size is too large".to_owned(),
        ),
        Log::ChunkedError => (
            TRACE,
            "Bad Request: protocol error when parse chunked length".to_owned(),
        ),
        Log::ChunkedTooLarge => (
            WARNING,
            format!(
                "Request Entity Too Large: request length (chunked) has already been greater \
                 than the package_max_length({})",
                max()
            ),
        ),
        Log::TooLarge => {
            let request = n.request();
            let (header, content) = (request.header_length, request.content_length);
            (
                WARNING,
                format!(
                    "Request Entity Too Large: header-length ({header}) + content-length \
                     ({content}) is greater than the package_max_length({})",
                    max()
                ),
            )
        }
        Log::Discard(bytes) => (
            TRACE,
            format!("Invalid Request: {bytes} bytes has been discard"),
        ),
    };
    log::error_log(
        level,
        ErrorCode::SW_ERROR_HTTP_INVALID_PROTOCOL as c_int,
        c"ListenPort::readable_callback_http",
        format_args!("{message}{from}"),
    );
}

fn parse(n: &Native) -> Step {
    if let Some(step) = head(n) {
        return step;
    }
    if !n.request().tried_to_dispatch
        && let Some(step) = bodyless(n)
    {
        return step;
    }
    body(n)
}

/// The request line, an HTTP/2 preface, the header block and form setup.
fn head(n: &Native) -> Option<Step> {
    if n.request().method == 0 && n.request().get_protocol() < 0 {
        if !n.request().excepted && (n.buffer().length as u64) < HEADER_MAX_SIZE {
            return Some(Step::Done);
        }
        return Some(bad(n, Log::UnknownProtocol));
    }
    let method = n.request().method;
    if method > HTTP_PRI {
        return Some(bad(n, Log::UnknownMethod));
    }
    if method == HTTP_PRI {
        return Some(preface(n));
    }
    if n.request().header_length == 0 && n.request().get_header_length() < 0 {
        let buffer = n.buffer();
        if buffer.size == buffer.length {
            return Some(bad(n, Log::HeaderTooLarge));
        }
        return Some(Step::Receive);
    }
    if !n.request().header_parsed {
        n.request().parse_header_info();
        // `package_max_length` is a `uint32_t`.
        n.request().max_length = u32::try_from(n.get(OP_MAX_LENGTH)).unwrap_or(u32::MAX);
        n.trace(TRACE_HEADER);
        if n.request().form.is_some() {
            if n.is(OP_UPLOADS) && {
                let request = n.request();
                u64::from(request.header_length).wrapping_add(request.content_length)
                    > u64::from(request.max_length)
            } {
                n.get(OP_REQ_MULTIPART_INIT);
            } else {
                // `delete form_data_`: no parser was made for it.
                n.request().form = None;
            }
        }
    }
    if n.request().form.is_some() {
        return form(n);
    }
    None
}

fn preface(n: &Native) -> Step {
    if !n.is(OP_HTTP2_ENABLED) {
        return bad(n, Log::NoHttp2);
    }
    n.get(OP_HTTP2_START);
    if n.buffer().length as u64 == HTTP2_PREFACE {
        n.get(OP_REQ_DESTROY);
        n.clear();
        return Step::Done;
    }
    n.op(OP_BUF_REDUCE, n.buffer().offset.cast_unsigned());
    n.get(OP_REQ_DESTROY);
    n.get(OP_SKIP_RECV);
    Step::Length
}

/// A multipart upload streamed to disk.
fn form(n: &Native) -> Option<Step> {
    if !n.request().multipart_header_parsed && !n.is(OP_BUF_HEADER_END) {
        return Some(Step::Done);
    }
    let (request, buffer) = (n.shared().request, n.shared().buffer);
    // SAFETY: the connection's live request, with a form, and the current buffer; the
    // parser's callbacks are the only other users of either during the call.
    if !unsafe { swoole_rs_http::parse_multipart_data(request, buffer) } {
        return Some(Step::Reply(SEND_BAD_REQUEST));
    }
    let request = n.request();
    if request.too_large {
        return Some(Step::Reply(SEND_TOO_LARGE));
    }
    if request.unavailable {
        return Some(Step::Reply(SEND_UNAVAILABLE));
    }
    if !request.tried_to_dispatch {
        return Some(Step::Done);
    }
    // SAFETY: the connection's live request, which nothing else uses during the call.
    unsafe { swoole_rs_http::destroy_multipart_parser(n.shared().request) };
    n.use_request_buffer();
    None
}

/// The first look at a request: one without a body is served here, pipelined
/// requests behind it parsed next.
fn bodyless(n: &Native) -> Option<Step> {
    if n.request().nobody_chunked {
        let header_length = n.request().header_length;
        if (n.buffer().length as u64) < u64::from(header_length) + u64::from(CHUNK_EOF) {
            return Some(Step::Receive);
        }
        n.request().header_length = header_length.wrapping_add(CHUNK_EOF);
    }
    let request = n.request();
    request.tried_to_dispatch = true;
    let bodyless = request.content_length == 0 && (request.known_length || !request.chunked);
    if !bodyless {
        return None;
    }
    let header_length = u64::from(n.request().header_length);
    n.set_offset(header_length);
    if !(n.shared().static_handler != 0 && n.is(OP_STATIC))
        && n.signed(OP_DISPATCH_REQUEST, u64::from(n.request().header_length)) < 0
    {
        return Some(Step::Close);
    }
    if !n.is(OP_ACTIVE) {
        return Some(Step::Done);
    }
    let header_length = u64::from(n.request().header_length);
    if n.buffer().length as u64 > header_length {
        n.op(OP_BUF_REDUCE, header_length);
        n.request().clean();
        return Some(Step::Parse);
    }
    n.get(OP_REQ_DESTROY);
    n.clear();
    Some(Step::Done)
}

/// A request with a body: wait for all of it, then dispatch.
fn body(n: &Native) -> Step {
    let max = n.get(OP_MAX_LENGTH);
    let request_length;
    if n.request().chunked {
        if n.request().get_chunked_body_length() < 0 {
            if n.request().excepted {
                return bad(n, Log::ChunkedError);
            }
            let wanted = (n.buffer().size as u64).wrapping_add(BUFFER_SIZE_BIG);
            if wanted > max {
                log(n, Log::ChunkedTooLarge);
                return Step::Reply(SEND_TOO_LARGE);
            }
            let buffer = n.buffer();
            if buffer.length == buffer.size {
                n.op(OP_BUF_EXTEND, wanted);
            }
            return Step::Receive;
        }
        let request = n.request();
        request_length = u64::from(request.header_length).wrapping_add(request.content_length);
        n.trace(TRACE_CHUNKED);
    } else {
        let request = n.request();
        request_length = u64::from(request.header_length).wrapping_add(request.content_length);
        if request_length > max {
            log(n, Log::TooLarge);
            return Step::Reply(SEND_TOO_LARGE);
        }
        if request_length > n.buffer().size as u64 {
            n.op(OP_BUF_EXTEND, request_length);
        }
        if (n.buffer().length as u64) < request_length {
            if n.request().has_expect_header() {
                n.op(OP_SEND, SEND_CONTINUE);
            } else {
                n.trace(TRACE_POST_WAIT);
            }
            return Step::Receive;
        }
    }
    let length = n.buffer().length as u64;
    if length > request_length {
        log(n, Log::Discard(length - request_length));
        n.buffer().length = size(request_length);
    }
    n.set_offset(request_length);
    if n.signed(OP_DISPATCH_REQUEST, n.buffer().length as u64) < 0 {
        return Step::Close;
    }
    if n.is(OP_ACTIVE) {
        n.get(OP_REQ_DESTROY);
        if !n.is(OP_RELEASE) {
            n.clear();
        }
    }
    Step::Done
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// A scripted connection: chunks to receive (empty: peer closed; none left:
    /// would block), a receive buffer and the connection's request. It starts with
    /// the [`Shared`] head, pointing at its own buffer and request.
    #[repr(C)]
    struct Fake {
        shared: Shared,
        buf: Buffer,
        req: Request,
        incoming: VecDeque<Vec<u8>>,
        bytes: Vec<u8>,
        request: bool,
        max: u64,
        http2: bool,
        dispatched: Vec<Vec<u8>>,
        effects: Vec<u32>,
        sent: Vec<u64>,
    }

    /// The request's native effects: none of these requests has a form.
    unsafe extern "C" fn request_native(
        _: u32,
        _: *mut c_void,
        _: *const u8,
        _: u64,
        _: u64,
    ) -> u64 {
        0
    }

    fn fake(chunks: &[&[u8]]) -> Fake {
        Fake {
            shared: Shared {
                buffer: core::ptr::null_mut(),
                request: core::ptr::null_mut(),
                websocket: 0,
                upgraded: 0,
                http2_stream: 0,
                static_handler: 1,
                trace: 1,
            },
            buf: Buffer {
                length: 0,
                size: 64,
                offset: 0,
                str: core::ptr::null_mut(),
            },
            req: Request::new(request_native),
            incoming: chunks.iter().map(|c| c.to_vec()).collect(),
            bytes: vec![0; 64],
            request: false,
            max: 1024,
            http2: false,
            dispatched: Vec::new(),
            effects: Vec::new(),
            sent: Vec::new(),
        }
    }

    impl Fake {
        fn receive(&mut self, whole: bool) -> u64 {
            let Some(chunk) = self.incoming.pop_front() else {
                return u64::MAX;
            };
            let start = if whole { 0 } else { self.buf.length };
            let n = chunk.len().min(self.buf.size - start);
            self.bytes[start..start + n].copy_from_slice(&chunk[..n]);
            n as u64
        }
        fn reduce(&mut self, n: usize) {
            self.bytes.copy_within(n..self.buf.length, 0);
            self.buf.length -= n;
            self.buf.offset = 0;
        }
        /// Repoints the shared head at this fake's buffer and request, as native
        /// code does when its effects change them.
        fn point(&mut self) {
            (self.buf.str, self.buf.size) = (self.bytes.as_mut_ptr(), self.bytes.len());
            self.shared.buffer = &raw mut self.buf;
            self.shared.request = if self.request {
                (&raw mut self.req).cast()
            } else {
                core::ptr::null_mut()
            };
        }
        fn call(&mut self, op: u32, arg: u64) -> u64 {
            let flag = u64::from;
            let at = usize::try_from(arg).unwrap();
            match op {
                OP_RECV => self.receive(arg == 1),
                OP_CATCH => 2,
                OP_RECV_BUFFER => 1,
                OP_DISPATCH_TASK | OP_DISPATCH_REQUEST => {
                    self.dispatched.push(self.bytes[..at].to_vec());
                    0
                }
                OP_ACTIVE => 1,
                OP_HTTP2_ENABLED => flag(self.http2),
                OP_MAX_LENGTH => self.max,
                OP_BUF_REDUCE => {
                    self.reduce(at);
                    0
                }
                OP_BUF_EXTEND => {
                    self.bytes.resize(at, 0);
                    self.point();
                    0
                }
                OP_REQ_INIT => {
                    self.request = true;
                    self.req.buffer = &raw mut self.buf;
                    self.point();
                    0
                }
                OP_REQ_DESTROY => {
                    self.request = false;
                    self.req = Request::new(request_native);
                    self.point();
                    0
                }
                OP_SEND => {
                    self.sent.push(arg);
                    0
                }
                OP_CLIENT => [7, c"127.0.0.1".as_ptr() as u64, 9501][at],
                _ => {
                    self.effects.push(op);
                    0
                }
            }
        }
    }

    unsafe extern "C" fn call(context: *mut c_void, op: u32, arg: u64) -> u64 {
        // SAFETY: tests pass their live `Fake`.
        unsafe { &mut *context.cast::<Fake>() }.call(op, arg)
    }

    std::thread_local! {
        static LOGGED: std::cell::RefCell<Vec<(c_int, c_int, String)>> = const {
            std::cell::RefCell::new(Vec::new())
        };
    }

    unsafe extern "C" fn record(
        _: c_int,
        level: c_int,
        code: c_int,
        origin: *const c_char,
        message: *const c_char,
    ) {
        let text = |p| unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned();
        assert_eq!(text(origin), "ListenPort::readable_callback_http");
        LOGGED.with_borrow_mut(|logged| logged.push((level, code, text(message))));
    }

    /// The logs this thread's runs made since the last call.
    fn logged() -> Vec<(c_int, c_int, String)> {
        LOGGED.take()
    }

    fn run(kind: u32, fake: &mut Fake) -> i32 {
        log::swr_v1_core_set_log_host(record);
        fake.point();
        // SAFETY: `fake` is live for the call and starts with its `Shared`.
        unsafe { swr_v1_ports_on_read(kind, (&raw mut *fake).cast(), call) }
    }

    const GET: &[u8] = b"GET / HTTP/1.1\r\n\r\n";

    #[test]
    fn get_is_dispatched_and_the_request_released() {
        let mut f = fake(&[GET]);
        assert_eq!(run(ON_READ_HTTP, &mut f), SW_OK, "status");
        assert_eq!(f.dispatched, [GET.to_vec()], "one dispatch of the header");
        assert!(
            !f.request && f.buf.length == 0,
            "request destroyed, buffer cleared"
        );
        assert!(
            f.effects.contains(&OP_STATIC),
            "static handler consulted first"
        );
        let mut f = fake(&[GET]);
        f.shared.static_handler = 0;
        run(ON_READ_HTTP, &mut f);
        assert!(
            !f.effects.contains(&OP_STATIC) && f.dispatched.len() == 1,
            "no static handler: dispatched without asking"
        );
    }

    #[test]
    fn pipelined_requests_are_parsed_in_turn() {
        let mut f = fake(&[&[GET, GET].concat()]);
        run(ON_READ_HTTP, &mut f);
        assert_eq!(f.dispatched, [GET.to_vec(), GET.to_vec()], "two dispatches");
    }

    #[test]
    fn a_body_is_awaited_then_dispatched_whole() {
        let head = b"POST / HTTP/1.1\r\nContent-Length: 4\r\n\r\n";
        let mut f = fake(&[head, b"ab"]);
        run(ON_READ_HTTP, &mut f);
        assert!(
            f.dispatched.is_empty() && f.effects.contains(&TRACE_POST_WAIT),
            "waits for the body"
        );
        f.incoming.push_back(b"cd".to_vec());
        run(ON_READ_HTTP, &mut f);
        assert_eq!(
            f.dispatched,
            [[&head[..], b"abcd"].concat()],
            "body dispatched with its header"
        );
        assert!(
            f.effects.contains(&OP_RELEASE),
            "large buffers released after dispatch"
        );
    }

    #[test]
    fn traces_only_when_compiled_in() {
        let head = b"POST / HTTP/1.1\r\nContent-Length: 4\r\n\r\n";
        let mut f = fake(&[head]);
        f.shared.trace = 0;
        run(ON_READ_HTTP, &mut f);
        assert!(
            !f.effects
                .iter()
                .any(|op| (TRACE_HEADER..=TRACE_CLOSE).contains(op)),
            "no trace calls"
        );
    }

    #[test]
    fn oversize_unknown_and_closed_requests_close() {
        let mut f = fake(&[b"POST / HTTP/1.1\r\nContent-Length: 2000\r\n\r\n"]);
        run(ON_READ_HTTP, &mut f);
        assert_eq!(f.sent, [SEND_TOO_LARGE], "413");
        assert!(f.effects.ends_with(&[OP_CLOSE]), "closed");
        assert_eq!(
            logged(),
            [(
                WARNING,
                7102,
                "Request Entity Too Large: header-length (41) + content-length (2000) is \
                 greater than the package_max_length(1024) from session#7 on 127.0.0.1:9501"
                    .to_owned()
            )],
            "logged first"
        );

        let mut f = fake(&[b"BREW / HTTP/1.1\r\n\r\n"]);
        run(ON_READ_HTTP, &mut f);
        assert_eq!(f.sent, [SEND_BAD_REQUEST], "400 for an unknown method");
        assert!(f.effects.ends_with(&[OP_CLOSE]), "closed");
        assert_eq!(
            logged(),
            [(
                TRACE,
                7102,
                "Bad Request: unknown protocol from session#7 on 127.0.0.1:9501".to_owned()
            )],
            "logged as a bad request line"
        );

        let mut f = fake(&[b""]);
        run(ON_READ_HTTP, &mut f);
        assert!(
            f.sent.is_empty() && f.effects.ends_with(&[OP_CLOSE]),
            "peer close"
        );
    }

    #[test]
    fn http2_preface_switches_to_frames() {
        let preface = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";
        let mut f = fake(&[preface]);
        run(ON_READ_HTTP, &mut f);
        assert_eq!(f.sent, [SEND_BAD_REQUEST], "refused without http2");
        let mut f = fake(&[preface]);
        f.http2 = true;
        run(ON_READ_HTTP, &mut f);
        assert!(
            f.effects.contains(&OP_HTTP2_START) && f.buf.length == 0,
            "preface consumed"
        );
        let mut f = fake(&[&[&preface[..], b"frame"].concat()]);
        f.http2 = true;
        run(ON_READ_HTTP, &mut f);
        assert!(
            f.effects
                .ends_with(&[OP_SKIP_RECV, OP_RECV_LENGTH, OP_RELEASE]),
            "frames follow"
        );
    }

    #[test]
    fn upgraded_connections_read_frames() {
        let mut f = fake(&[]);
        f.shared.websocket = 1;
        run(ON_READ_HTTP, &mut f);
        assert_eq!(
            f.effects,
            [OP_WEBSOCKET_ACTIVATE, OP_RECV_LENGTH, OP_RELEASE],
            "the handshake's request goes, then frames"
        );
        let mut f = fake(&[]);
        f.shared.http2_stream = 1;
        run(ON_READ_HTTP, &mut f);
        assert_eq!(f.effects, [OP_RECV_LENGTH, OP_RELEASE], "HTTP/2 frames");
    }

    #[test]
    fn raw_and_framed_ports() {
        let mut f = fake(&[b"ping"]);
        assert_eq!(run(ON_READ_RAW, &mut f), 0, "dispatch_task's status");
        assert_eq!(
            (f.dispatched.len(), f.buf.length, f.buf.offset),
            (1, 4, 4),
            "whole read dispatched"
        );
        let mut f = fake(&[]);
        run(ON_READ_REDIS, &mut f);
        assert_eq!(f.effects, [OP_RECV_REDIS], "redis keeps its buffer");
        run(ON_READ_EOF, &mut f);
        assert_eq!(
            f.effects[1..],
            [OP_RECV_EOF, OP_RELEASE],
            "eof releases its buffer"
        );
        assert_eq!(run(99, &mut f), SW_ERR, "unknown kind");
    }
}
