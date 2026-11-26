//! `ListenPort`'s readable callbacks (raw, length, EOF, Redis and HTTP) as whole
//! operations over a [`Connection`]: the facts and effects of the socket, connection
//! and dispatch (`crate::read_native`'s `Read` in the server). The receive buffer
//! and the HTTP request are read and written in place, as upstream's reader did; the
//! request's own operations are `swoole_rs_http`'s.
use core::ffi::c_int;
use swoole_rs_corebase::buffer::string::StringBuffer as Buffer;
use swoole_rs_corebase::constants::HTTP2_PREFACE;
use swoole_rs_corebase::log;
use swoole_rs_corebase::log::LogLevel::{SW_LOG_TRACE as TRACE, SW_LOG_WARNING as WARNING};
use swoole_rs_corebase::ossupport::TempFile;
use swoole_rs_corebase::reactor::Caught;
use swoole_rs_error::ErrorCode;
use swoole_rs_http::{HttpRequest as Request, METHOD_PRI};

/// Which readable callback runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Readable {
    Raw,
    Length,
    Eof,
    Redis,
    Http,
}

/// A canned response [`Connection::send`] sends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reply {
    BadRequest,
    TooLarge,
    Unavailable,
    Continue,
}

/// How [`Connection::receive`] frames what it reads:
/// `protocol.recv_with_length_protocol`, `recv_with_eof_protocol`, `redis::recv_packet`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Framing {
    Length,
    Eof,
    Redis,
}

/// Reactor dispatch flag for a request whose upload body has been preprocessed.
pub const UPLOAD_PREPROCESSED: u16 = 1 << 0;

/// What [`Connection::dispatch`] hands on: `Server::dispatch_task`,
/// `http_server::dispatch_request`, or a request flagged
/// `SW_HTTP_EXT_FLAG_UPLOAD_PREPROCESSED` (the reactor rewrote the body, so its
/// `Swoole-Upload-File` headers are its own).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dispatch {
    Task,
    Request,
    Upload,
}

/// An HTTP connection's state at entry: `is_websocket_active()` (open: its 101 was
/// sent), `http_upgrade`, `http2_stream`.
#[derive(Clone, Copy, Default)]
pub struct Upgrade {
    pub websocket: bool,
    pub upgraded: bool,
    pub http2_stream: bool,
}

/// The connection a readable event is on: each fact and effect a reader needs, and
/// its current receive buffer and HTTP request, borrowed until the next call.
pub(crate) trait Connection {
    /// `recv` into the buffer, after its contents or (`whole`) over them, which then
    /// count what it wrote: the bytes read, or `None` when it failed.
    fn recv(&mut self, whole: bool) -> Option<usize>;
    /// `catch_read_error(errno)`.
    fn catch_error(&mut self) -> Caught;
    /// Warns that `recv` failed.
    fn warn_recv(&mut self);
    /// `conn->close_errno = errno`.
    fn close_errno(&mut self);
    /// `reactor->trigger_close_event(event)`.
    fn close(&mut self);
    fn send(&mut self, reply: Reply);
    /// Frees the socket's receive buffer when larger than twice `SW_BUFFER_SIZE_BIG`
    /// (`if_empty`: and empty). Whether it did.
    fn release(&mut self, if_empty: bool) -> bool;
    /// One framed receive into the current buffer: false closes the connection.
    fn receive(&mut self, framing: Framing) -> bool;
    /// Hands on `length` bytes from the buffer's start; the connection's request is
    /// read afresh afterwards. False closes the connection.
    fn dispatch(&mut self, dispatch: Dispatch, length: u64) -> bool;
    /// `!socket->removed && conn->active`: a closed connection's slot is not read.
    fn active(&mut self) -> bool;
    fn upgrade(&mut self) -> Upgrade;
    /// Sets `http_upgrade = 1` on an open WebSocket.
    fn activate_websocket(&mut self);
    /// `open_http2_protocol`.
    fn http2_enabled(&mut self) -> bool;
    /// Sets `http2_stream` and sends the settings frame.
    fn start_http2(&mut self);
    /// `socket->skip_recv = 1`.
    fn skip_recv(&mut self);
    /// `protocol.package_max_length`.
    fn max_length(&mut self) -> u64;
    /// `serv->upload_max_filesize > 0`.
    fn uploads(&mut self) -> bool;
    /// `enable_static_handler`.
    fn static_handler(&mut self) -> bool;
    /// The static handler served the request.
    fn serve_static(&mut self) -> bool;
    /// The current buffer.
    fn buffer(&mut self) -> &mut Buffer;
    /// `buffer->reduce(offset)`.
    fn reduce(&mut self, offset: u64);
    /// `buffer->extend(size)`.
    fn extend(&mut self, size: u64);
    /// The buffer holds `\r\n\r\n`.
    fn header_end(&mut self) -> bool;
    /// Creates the request when missing and makes it current, and with it the buffer
    /// it receives into: its multipart body's, if any, else the receive buffer.
    fn init_request(&mut self);
    /// `port->destroy_http_request(conn)`; no request is current after it.
    fn destroy_request(&mut self);
    /// The current request, after [`Connection::init_request`].
    fn request(&mut self) -> &mut Request;
    /// `parse` over the current request and the receive buffer it is read into.
    fn parse<R>(&mut self, parse: impl FnOnce(&mut Request, &mut Buffer) -> R) -> R;
    /// `init_multipart_parser(serv)`, then its body buffer becomes current. Whether it succeeded.
    fn init_multipart(&mut self) -> bool;
    /// The client a log names: `conn->session_id`, `port->host`, `port->port`.
    fn client(&mut self) -> (i64, String, i64);
}

/// `readable_callback_http`'s logs, in upstream's wording.
#[derive(Clone, Copy)]
enum Log {
    UnknownProtocol,
    UnknownMethod,
    NoHttp2,
    HeaderTooLarge,
    TransferEncoding,
    ChunkedError,
    ChunkedTooLarge,
    TooLarge,
}

/// `SW_HTTP_PRI`, `SW_HTTP_HEADER_MAX_SIZE`, `sizeof(SW_HTTP2_PRI_STRING) - 1`,
/// `SW_BUFFER_SIZE_BIG`.
const HEADER_MAX_SIZE: u64 = swoole_rs_corebase::constants::SW_HTTP_HEADER_MAX_SIZE as u64;
const BUFFER_SIZE_BIG: u64 = swoole_rs_corebase::constants::SW_BUFFER_SIZE_BIG as u64;

/// `buffer->clear()`.
fn clear(n: &mut impl Connection) {
    n.buffer().clear();
}

/// `buffer->offset = value`.
fn set_offset(n: &mut impl Connection, value: u64) {
    n.buffer().set_offset(value.cast_signed());
}

/// `ListenPort::readable_callback_*`: false when an EOF port has no buffer or a raw
/// port's dispatch failed.
#[inline(always)]
pub(crate) fn readable(kind: Readable, n: &mut impl Connection) -> bool {
    match kind {
        Readable::Raw => raw(n),
        Readable::Length => framed(n, Framing::Length, true),
        Readable::Eof => framed(n, Framing::Eof, true),
        Readable::Redis => framed(n, Framing::Redis, false),
        Readable::Http => http(n),
    }
}

/// A failed `recv`: warn and wait, or record errno and close.
enum Failure {
    Wait,
    Close,
}

fn read_error(n: &mut impl Connection) -> Failure {
    match n.catch_error() {
        Caught::Error => {
            n.warn_recv();
            Failure::Wait
        }
        Caught::Close => {
            n.close_errno();
            Failure::Close
        }
        Caught::Wait | Caught::Invalid | Caught::ReduceSize => Failure::Wait,
    }
}

fn raw(n: &mut impl Connection) -> bool {
    let Some(received) = n.recv(true) else {
        if matches!(read_error(n), Failure::Close) {
            n.close();
        }
        return true;
    };
    if received == 0 {
        n.close();
        return true;
    }
    let length = received as u64;
    set_offset(n, length);
    n.dispatch(Dispatch::Task, length)
}

/// Length and Redis framing; length (and EOF) ports free a large drained buffer.
#[inline(always)]
fn framed(n: &mut impl Connection, framing: Framing, release: bool) -> bool {
    if !n.receive(framing) {
        n.close_errno();
        n.close();
    }
    if release {
        n.release(true);
    }
    true
}

/// Where the HTTP reader goes next; upstream's labels.
enum Step {
    Receive,
    Parse,
    Reply(Reply),
    Close,
    Length,
    Done,
}

fn http(n: &mut impl Connection) -> bool {
    let upgrade = n.upgrade();
    if upgrade.websocket {
        if !upgrade.upgraded {
            n.destroy_request();
            n.activate_websocket();
        }
        return framed(n, Framing::Length, true);
    }
    if upgrade.http2_stream {
        return framed(n, Framing::Length, true);
    }
    n.init_request();
    let mut step = Step::Receive;
    loop {
        step = match step {
            Step::Receive => receive(n),
            Step::Parse => parse(n),
            Step::Reply(reply) => {
                n.send(reply);
                Step::Close
            }
            Step::Close => {
                n.destroy_request();
                n.close();
                return true;
            }
            Step::Length => return framed(n, Framing::Length, true),
            Step::Done => return true,
        }
    }
}

fn receive(n: &mut impl Connection) -> Step {
    let Some(received) = n.recv(false) else {
        return match read_error(n) {
            Failure::Wait => Step::Done,
            Failure::Close => Step::Close,
        };
    };
    if received == 0 {
        return Step::Close;
    }
    Step::Parse
}

fn bad(n: &mut impl Connection, which: Log) -> Step {
    log(n, which);
    Step::Reply(Reply::BadRequest)
}

/// One of `readable_callback_http`'s `swoole_error_log`s, naming the client.
fn log(n: &mut impl Connection, which: Log) {
    let (session, host, port) = n.client();
    let from = format!(" from session#{session} on {host}:{port}");
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
        Log::TransferEncoding => (TRACE, "Bad Request: invalid Transfer-Encoding".to_owned()),
        Log::ChunkedError => (
            TRACE,
            "Bad Request: protocol error when parse chunked length".to_owned(),
        ),
        Log::ChunkedTooLarge => {
            let (length, max) = (n.buffer().len(), n.max_length());
            (
                WARNING,
                format!(
                    "Request Entity Too Large: the chunked request length ({length}) has \
                     reached the package_max_length({max})"
                ),
            )
        }
        Log::TooLarge => {
            let request = n.request();
            let (header, content) = (request.header_length, request.content_length);
            let max = n.max_length();
            (
                WARNING,
                format!(
                    "Request Entity Too Large: header-length ({header}) + content-length \
                     ({content}) is greater than the package_max_length({max})"
                ),
            )
        }
    };
    log::error_log(
        level,
        ErrorCode::SW_ERROR_HTTP_INVALID_PROTOCOL as c_int,
        c"ListenPort::readable_callback_http",
        format_args!("{message}{from}"),
    );
}

fn parse(n: &mut impl Connection) -> Step {
    if let Some(step) = head(n) {
        return step;
    }
    // A form here is a multipart upload streamed to disk.
    if n.request().form.is_some() {
        if let Some(step) = form(n) {
            return step;
        }
        let uploads = n.request().destroy_multipart_parser();
        return body(n, Some(uploads));
    }
    if !n.request().tried_to_dispatch
        && let Some(step) = bodyless(n)
    {
        return step;
    }
    body(n, None)
}

/// The request line, an HTTP/2 preface, the header block and form setup.
fn head(n: &mut impl Connection) -> Option<Step> {
    if n.request().method == 0 && !n.parse(Request::get_protocol) {
        if !n.request().excepted && (n.buffer().len() as u64) < HEADER_MAX_SIZE {
            return Some(Step::Done);
        }
        return Some(bad(n, Log::UnknownProtocol));
    }
    let method = n.request().method;
    if method > METHOD_PRI {
        return Some(bad(n, Log::UnknownMethod));
    }
    if method == METHOD_PRI {
        return Some(preface(n));
    }
    if n.request().header_length == 0 && !n.parse(Request::get_header_length) {
        let buffer = n.buffer();
        if buffer.size() == buffer.len() {
            return Some(bad(n, Log::HeaderTooLarge));
        }
        return Some(Step::Receive);
    }
    if !n.request().header_parsed {
        n.parse(|r, b| r.parse_header_info(b.view()));
        if n.request().excepted {
            return Some(bad(n, Log::TransferEncoding));
        }
        // `package_max_length` is a `uint32_t`.
        let max = u32::try_from(n.max_length()).unwrap_or(u32::MAX);
        n.request().max_length = max;
        if n.request().form.is_some() {
            if n.uploads() && {
                let request = n.request();
                u64::from(request.header_length).wrapping_add(request.content_length)
                    > u64::from(request.max_length)
            } {
                let send_continue = continues(n);
                if !n.init_multipart() {
                    // `delete form_data_`: nothing was made for it.
                    n.request().form = None;
                    return Some(Step::Reply(Reply::BadRequest));
                }
                if send_continue {
                    n.send(Reply::Continue);
                }
            } else {
                // `delete form_data_`: no parser was made for it.
                n.request().form = None;
            }
        }
    }
    None
}

fn preface(n: &mut impl Connection) -> Step {
    if !n.http2_enabled() {
        return bad(n, Log::NoHttp2);
    }
    n.start_http2();
    if n.buffer().len() == HTTP2_PREFACE.len() {
        n.destroy_request();
        clear(n);
        return Step::Done;
    }
    let offset = n.buffer().offset().cast_unsigned();
    n.reduce(offset);
    n.destroy_request();
    n.skip_recv();
    Step::Length
}

/// A multipart upload streamed to disk.
fn form(n: &mut impl Connection) -> Option<Step> {
    if !n.request().multipart_header_parsed && !n.header_end() {
        return Some(Step::Done);
    }
    if !n.parse(Request::parse_multipart_data) {
        return Some(Step::Reply(Reply::BadRequest));
    }
    let request = n.request();
    if request.too_large {
        return Some(Step::Reply(Reply::TooLarge));
    }
    if request.unavailable {
        return Some(Step::Reply(Reply::Unavailable));
    }
    if !request.tried_to_dispatch {
        return Some(Step::Done);
    }
    None
}

/// The first look at a request: one without a body is served here, pipelined
/// requests behind it parsed next.
fn bodyless(n: &mut impl Connection) -> Option<Step> {
    let request = n.request();
    request.tried_to_dispatch = true;
    if request.content_length != 0 || request.chunked {
        return None;
    }
    let header_length = u64::from(request.header_length);
    set_offset(n, header_length);
    if !(n.static_handler() && n.serve_static()) {
        let header_length = u64::from(n.request().header_length);
        if !n.dispatch(Dispatch::Request, header_length) {
            return Some(Step::Close);
        }
    }
    if !n.active() {
        return Some(Step::Done);
    }
    let header_length = u64::from(n.request().header_length);
    if n.buffer().len() as u64 > header_length {
        n.reduce(header_length);
        n.request().clean();
        return Some(Step::Parse);
    }
    n.destroy_request();
    clear(n);
    Some(Step::Done)
}

/// Whether the client waits for `100 Continue`: it sent only the head, asking for one.
fn continues(n: &mut impl Connection) -> bool {
    n.buffer().len() as u64 == u64::from(n.request().header_length)
        && n.parse(|r, b| r.has_expect_header(b.view()))
}

/// A request with a body: wait for all of it, then dispatch (an `upload` the
/// reactor rewrote flagged so); pipelined requests behind it are parsed next.
fn body(n: &mut impl Connection, uploads: Option<Vec<TempFile<()>>>) -> Step {
    let max = n.max_length();
    let request_length;
    if n.request().chunked {
        if !n.parse(|r, b| r.get_chunked_body_length(b.view())) {
            if n.request().too_large {
                return Step::Reply(Reply::TooLarge);
            }
            if n.request().excepted {
                return bad(n, Log::ChunkedError);
            }
            if n.buffer().len() as u64 >= max {
                log(n, Log::ChunkedTooLarge);
                return Step::Reply(Reply::TooLarge);
            }
            if continues(n) {
                n.send(Reply::Continue);
            }
            let buffer = n.buffer();
            if buffer.len() == buffer.size() {
                // The limit check above makes the new size larger.
                let size = (buffer.size() as u64 + BUFFER_SIZE_BIG).min(max);
                n.extend(size);
            }
            return Step::Receive;
        }
        let request = n.request();
        request_length = u64::from(request.header_length) + request.content_length;
    } else {
        let request = n.request();
        let (header_length, content_length) =
            (u64::from(request.header_length), request.content_length);
        if header_length > max || content_length > max - header_length {
            log(n, Log::TooLarge);
            return Step::Reply(Reply::TooLarge);
        }
        request_length = header_length + content_length;
        if request_length > n.buffer().size() as u64 {
            n.extend(request_length);
        }
        if (n.buffer().len() as u64) < request_length {
            if continues(n) {
                n.send(Reply::Continue);
            }
            return Step::Receive;
        }
    }
    set_offset(n, request_length);
    let dispatch = if uploads.is_some() {
        Dispatch::Upload
    } else {
        Dispatch::Request
    };
    if !n.dispatch(dispatch, request_length) {
        return Step::Close;
    }
    if let Some(uploads) = uploads {
        for upload in uploads {
            drop(upload.persist());
        }
    }
    if n.active() {
        if n.buffer().len() as u64 > request_length {
            n.reduce(request_length);
            n.request().clean();
            return Step::Parse;
        }
        n.destroy_request();
        if !n.release(false) {
            clear(n);
        }
    }
    Step::Done
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// The effects a [`Fake`] records.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Effect {
        WarnRecv,
        CloseErrno,
        Close,
        Release,
        Receive(Framing),
        ActivateWebsocket,
        StartHttp2,
        SkipRecv,
        Static,
        Upload,
    }

    /// A scripted connection: chunks to receive (empty: peer closed; none left:
    /// would block), a receive buffer and the connection's request.
    struct Fake {
        upgrade: Upgrade,
        static_handler: bool,
        buf: Buffer,
        req: Request,
        /// The chunks still to receive, the next one last.
        incoming: Vec<Vec<u8>>,
        request: bool,
        max: u64,
        /// `upload_max_filesize`.
        uploads: u64,
        http2: bool,
        dispatched: Vec<Vec<u8>>,
        on_dispatch: fn(&[u8]) -> bool,
        /// Each dispatched request's URL.
        urls: Vec<Vec<u8>>,
        effects: Vec<Effect>,
        sent: Vec<Reply>,
    }

    fn fake(chunks: &[&[u8]]) -> Fake {
        Fake {
            upgrade: Upgrade::default(),
            static_handler: true,
            buf: {
                let mut buf = Buffer::EMPTY;
                buf.alloc(64, None);
                buf
            },
            req: Request::default(),
            incoming: chunks.iter().rev().map(|c| c.to_vec()).collect(),
            request: false,
            max: 1024,
            uploads: 0,
            http2: false,
            dispatched: Vec::new(),
            on_dispatch: |_| true,
            urls: Vec::new(),
            effects: Vec::new(),
            sent: Vec::new(),
        }
    }

    impl Drop for Fake {
        fn drop(&mut self) {
            self.buf.dispose();
        }
    }

    impl Connection for Fake {
        /// Into the current buffer; what doesn't fit arrives next.
        fn recv(&mut self, whole: bool) -> Option<usize> {
            let mut chunk = self.incoming.pop()?;
            let buffer = self.buffer();
            if whole {
                buffer.truncate(0);
            }
            let n = chunk.len().min(buffer.size() - buffer.len());
            buffer.append(&chunk[..n]);
            if n < chunk.len() {
                self.incoming.push(chunk.split_off(n));
            }
            Some(n)
        }
        fn catch_error(&mut self) -> Caught {
            Caught::Wait
        }
        fn warn_recv(&mut self) {
            self.effects.push(Effect::WarnRecv);
        }
        fn close_errno(&mut self) {
            self.effects.push(Effect::CloseErrno);
        }
        fn close(&mut self) {
            self.effects.push(Effect::Close);
        }
        fn send(&mut self, reply: Reply) {
            self.sent.push(reply);
        }
        fn release(&mut self, _: bool) -> bool {
            self.effects.push(Effect::Release);
            false
        }
        fn receive(&mut self, framing: Framing) -> bool {
            self.effects.push(Effect::Receive(framing));
            true
        }
        fn dispatch(&mut self, dispatch: Dispatch, length: u64) -> bool {
            if dispatch == Dispatch::Upload {
                self.effects.push(Effect::Upload);
            }
            let at = usize::try_from(length).unwrap();
            let bytes = self.buf.view();
            self.dispatched.push(bytes[..at].to_vec());
            let url = self.req.url_offset as usize;
            self.urls
                .push(bytes[url..url + self.req.url_length as usize].to_vec());
            (self.on_dispatch)(&bytes[..at])
        }
        fn active(&mut self) -> bool {
            true
        }
        fn upgrade(&mut self) -> Upgrade {
            self.upgrade
        }
        fn activate_websocket(&mut self) {
            self.effects.push(Effect::ActivateWebsocket);
        }
        fn http2_enabled(&mut self) -> bool {
            self.http2
        }
        fn start_http2(&mut self) {
            self.effects.push(Effect::StartHttp2);
        }
        fn skip_recv(&mut self) {
            self.effects.push(Effect::SkipRecv);
        }
        fn max_length(&mut self) -> u64 {
            self.max
        }
        fn uploads(&mut self) -> bool {
            self.uploads > 0
        }
        fn static_handler(&mut self) -> bool {
            self.static_handler
        }
        fn serve_static(&mut self) -> bool {
            self.effects.push(Effect::Static);
            false
        }
        fn buffer(&mut self) -> &mut Buffer {
            self.req.body().unwrap_or(&mut self.buf)
        }
        fn reduce(&mut self, offset: u64) {
            self.buf.reduce(offset.cast_signed());
        }
        fn extend(&mut self, size: u64) {
            self.buf.extend(usize::try_from(size).unwrap());
        }
        fn header_end(&mut self) -> bool {
            self.buffer().view().windows(4).any(|w| w == b"\r\n\r\n")
        }
        fn init_request(&mut self) {
            self.request = true;
        }
        fn destroy_request(&mut self) {
            self.request = false;
            self.req = Request::default();
        }
        fn request(&mut self) -> &mut Request {
            assert!(self.request, "a current request");
            &mut self.req
        }
        fn parse<R>(&mut self, parse: impl FnOnce(&mut Request, &mut Buffer) -> R) -> R {
            assert!(self.request, "a current request");
            parse(&mut self.req, &mut self.buf)
        }
        fn init_multipart(&mut self) -> bool {
            let upload_max = usize::try_from(self.uploads).unwrap();
            self.req
                .init_multipart_parser(&mut self.buf, b"/tmp", upload_max)
        }
        fn client(&mut self) -> (i64, String, i64) {
            (7, "127.0.0.1".to_owned(), 9501)
        }
    }

    /// The logs this thread's runs made since the last call.
    fn logged() -> Vec<(c_int, c_int, String)> {
        log::capture::logged()
            .into_iter()
            .map(|(_, level, code, origin, message)| {
                assert_eq!(origin, "ListenPort::readable_callback_http");
                (level, code, message)
            })
            .collect()
    }

    const SW_OK: i32 = swoole_rs_corebase::constants::ResultCode::SW_OK as i32;
    const SW_ERR: i32 = swoole_rs_corebase::constants::ResultCode::SW_ERR as i32;

    /// The readable callback's status, as upstream returns it.
    fn run(kind: Readable, fake: &mut Fake) -> i32 {
        log::capture::install();
        if readable(kind, fake) { SW_OK } else { SW_ERR }
    }

    const GET: &[u8] = b"GET / HTTP/1.1\r\n\r\n";

    #[test]
    fn get_is_dispatched_and_the_request_released() {
        let mut f = fake(&[GET]);
        assert_eq!(run(Readable::Http, &mut f), SW_OK, "status");
        assert_eq!(f.dispatched, [GET.to_vec()], "one dispatch of the header");
        assert!(
            !f.request && f.buf.is_empty(),
            "request destroyed, buffer cleared"
        );
        assert!(
            f.effects.contains(&Effect::Static),
            "static handler consulted first"
        );
        let mut f = fake(&[GET]);
        f.static_handler = false;
        run(Readable::Http, &mut f);
        assert!(
            !f.effects.contains(&Effect::Static) && f.dispatched.len() == 1,
            "no static handler: dispatched without asking"
        );
    }

    #[test]
    fn pipelined_requests_are_parsed_in_turn() {
        let mut f = fake(&[&[GET, GET].concat()]);
        run(Readable::Http, &mut f);
        assert_eq!(f.dispatched, [GET.to_vec(), GET.to_vec()], "two dispatches");
    }

    #[test]
    fn a_body_is_awaited_then_dispatched_whole() {
        let head = b"POST / HTTP/1.1\r\nContent-Length: 4\r\n\r\n";
        let mut f = fake(&[head, b"ab"]);
        run(Readable::Http, &mut f);
        assert!(
            f.dispatched.is_empty() && f.incoming.is_empty(),
            "waits for the body"
        );
        f.incoming.insert(0, b"cd".to_vec());
        run(Readable::Http, &mut f);
        assert_eq!(
            f.dispatched,
            [[&head[..], b"abcd"].concat()],
            "body dispatched with its header"
        );
        assert!(
            f.effects.contains(&Effect::Release),
            "large buffers released after dispatch"
        );
    }

    include!("../../../tests/modules/ports/upload_dispatch.rs");

    /// A multipart body over `package_max_length` is rewritten by the reactor and
    /// dispatched flagged as preprocessed; a client's own `Swoole-Upload-File` part
    /// header is a bad request.
    #[test]
    fn uploads_are_dispatched_preprocessed_and_forged_markers_rejected() {
        let head = b"POST / HTTP/1.1\r\nContent-Type: multipart/form-data; boundary=b\r\nContent-Length: 300\r\n\r\n";
        let part = |extra: &[u8]| {
            [
                &b"--b\r\nContent-Disposition: form-data; name=\"a\"\r\n"[..],
                extra,
                b"\r\n1\r\n--b--\r\n",
            ]
            .concat()
        };
        let upload = |body: &[u8], max| {
            let mut f = fake(&[&[&head[..], body].concat()]);
            f.buf.extend(512);
            (f.max, f.uploads) = (max, 1024);
            run(Readable::Http, &mut f);
            f
        };
        let f = upload(&part(b""), 200);
        assert_eq!(f.dispatched.len(), 1, "dispatched");
        assert!(f.effects.contains(&Effect::Upload), "flagged preprocessed");
        assert!(f.sent.is_empty(), "no error reply");

        let exact = f.dispatched[0].len() as u64;
        assert_eq!(upload(&part(b""), exact).dispatched, f.dispatched);
        let oversized = upload(&part(b""), exact - 1);
        assert_eq!(oversized.dispatched, Vec::<Vec<u8>>::new());
        assert_eq!(oversized.sent, [Reply::TooLarge]);

        let f = upload(&part(b"sWoOlE-uPlOaD-fIlE: /tmp/victim\r\n"), 200);
        assert!(f.dispatched.is_empty(), "not dispatched");
        assert_eq!(f.sent, [Reply::BadRequest], "400");

        let mut f = fake(&[b"POST / HTTP/1.1\r\nContent-Length: 4\r\n\r\nabcd"]);
        run(Readable::Http, &mut f);
        assert!(
            f.dispatched.len() == 1 && !f.effects.contains(&Effect::Upload),
            "a plain body is not flagged"
        );
    }

    #[test]
    fn a_request_behind_a_body_is_parsed_next() {
        let post = b"POST /a HTTP/1.1\r\nContent-Length: 2\r\n\r\nab";
        let chunked = b"POST /b HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n1\r\nc\r\n0\r\n\r\n";
        let mut f = fake(&[&[&post[..], GET].concat()]);
        run(Readable::Http, &mut f);
        assert_eq!(f.dispatched, [post.to_vec(), GET.to_vec()], "both, whole");
        let mut f = fake(&[&[&chunked[..], GET].concat()]);
        f.buf.extend(128);
        run(Readable::Http, &mut f);
        assert_eq!(
            f.dispatched,
            [chunked.to_vec(), GET.to_vec()],
            "both, whole"
        );
    }

    #[test]
    fn continue_is_sent_once_before_the_body() {
        let head = b"POST / HTTP/1.1\r\nExpect: 100-continue\r\nContent-Length: 4\r\n\r\n";
        let mut f = fake(&[head, b"ab", b"cd"]);
        f.buf.extend(128);
        for _ in 0..3 {
            run(Readable::Http, &mut f);
        }
        assert_eq!(
            f.sent,
            [Reply::Continue],
            "only while just the head arrived"
        );
        assert_eq!(f.dispatched.len(), 1, "then the request");

        let head = b"POST / HTTP/1.1\r\nExpect: 100-continue\r\nTransfer-Encoding: chunked\r\n\r\n";
        let mut f = fake(&[head, b"1\r\na\r\n", b"0\r\n\r\n"]);
        f.buf.extend(128);
        for _ in 0..3 {
            run(Readable::Http, &mut f);
        }
        assert_eq!(
            f.sent,
            [Reply::Continue],
            "chunked: once, before the first chunk"
        );
        assert_eq!(f.dispatched.len(), 1, "then the request");
    }

    #[test]
    fn bad_transfer_codings_and_chunks_are_rejected() {
        let mut f = fake(&[b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked, gzip\r\n\r\n"]);
        run(Readable::Http, &mut f);
        assert_eq!(f.sent, [Reply::BadRequest], "400");
        assert_eq!(
            logged(),
            [(
                TRACE as c_int,
                7102,
                "Bad Request: invalid Transfer-Encoding from session#7 on 127.0.0.1:9501"
                    .to_owned()
            )],
            "logged"
        );

        let mut f = fake(&[b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n801\r\n"]);
        run(Readable::Http, &mut f);
        assert_eq!(
            f.sent,
            [Reply::TooLarge],
            "a chunk past package_max_length: 413"
        );
        assert!(logged().is_empty(), "without a log");

        let mut f = fake(&[
            b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n",
            &[b'1'; 1000],
        ]);
        for _ in 0..4 {
            run(Readable::Http, &mut f);
        }
        assert_eq!(
            f.sent,
            [Reply::TooLarge],
            "a size line filling package_max_length: 413"
        );
        assert_eq!(
            logged(),
            [(
                WARNING as c_int,
                7102,
                "Request Entity Too Large: the chunked request length (1024) has reached the \
                 package_max_length(1024) from session#7 on 127.0.0.1:9501"
                    .to_owned()
            )],
            "logged"
        );
    }

    #[test]
    fn oversize_unknown_and_closed_requests_close() {
        let mut f = fake(&[b"POST / HTTP/1.1\r\nContent-Length: 2000\r\n\r\n"]);
        run(Readable::Http, &mut f);
        assert_eq!(f.sent, [Reply::TooLarge], "413");
        assert!(f.effects.ends_with(&[Effect::Close]), "closed");
        assert_eq!(
            logged(),
            [(
                WARNING as c_int,
                7102,
                "Request Entity Too Large: header-length (41) + content-length (2000) is \
                 greater than the package_max_length(1024) from session#7 on 127.0.0.1:9501"
                    .to_owned()
            )],
            "logged first"
        );

        let mut f = fake(&[b"BREW / HTTP/1.1\r\n\r\n"]);
        run(Readable::Http, &mut f);
        assert_eq!(f.sent, [Reply::BadRequest], "400 for an unknown method");
        assert!(f.effects.ends_with(&[Effect::Close]), "closed");
        assert_eq!(
            logged(),
            [(
                TRACE as c_int,
                7102,
                "Bad Request: unknown protocol from session#7 on 127.0.0.1:9501".to_owned()
            )],
            "logged as a bad request line"
        );

        let mut f = fake(&[b""]);
        run(Readable::Http, &mut f);
        assert!(
            f.sent.is_empty() && f.effects.ends_with(&[Effect::Close]),
            "peer close"
        );
    }

    #[test]
    fn http2_preface_switches_to_frames() {
        let preface = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";
        let mut f = fake(&[preface]);
        run(Readable::Http, &mut f);
        assert_eq!(f.sent, [Reply::BadRequest], "refused without http2");
        let mut f = fake(&[preface]);
        f.http2 = true;
        run(Readable::Http, &mut f);
        assert!(
            f.effects.contains(&Effect::StartHttp2) && f.buf.is_empty(),
            "preface consumed"
        );
        let mut f = fake(&[&[&preface[..], b"frame"].concat()]);
        f.http2 = true;
        run(Readable::Http, &mut f);
        assert!(
            f.effects.ends_with(&[
                Effect::SkipRecv,
                Effect::Receive(Framing::Length),
                Effect::Release
            ]),
            "frames follow"
        );
    }

    #[test]
    fn upgraded_connections_read_frames() {
        let mut f = fake(&[]);
        f.upgrade.websocket = true;
        run(Readable::Http, &mut f);
        assert_eq!(
            f.effects,
            [
                Effect::ActivateWebsocket,
                Effect::Receive(Framing::Length),
                Effect::Release
            ],
            "the handshake's request goes, then frames"
        );
        let mut f = fake(&[]);
        f.upgrade.http2_stream = true;
        run(Readable::Http, &mut f);
        assert_eq!(
            f.effects,
            [Effect::Receive(Framing::Length), Effect::Release],
            "HTTP/2 frames"
        );
    }

    #[test]
    fn raw_and_framed_ports() {
        let mut f = fake(&[b"ping"]);
        assert_eq!(run(Readable::Raw, &mut f), 0, "dispatch_task's status");
        assert_eq!(
            (f.dispatched.len(), f.buf.len(), f.buf.offset()),
            (1, 4, 4),
            "whole read dispatched"
        );
        let mut f = fake(&[]);
        run(Readable::Redis, &mut f);
        assert_eq!(
            f.effects,
            [Effect::Receive(Framing::Redis)],
            "redis keeps its buffer"
        );
        run(Readable::Eof, &mut f);
        assert_eq!(
            f.effects[1..],
            [Effect::Receive(Framing::Eof), Effect::Release],
            "eof releases its buffer"
        );
    }

    /// Upstream's HTTP server core tests, ported (`tests/modules/http/README.md`).
    mod http_server {
        include!("../../../tests/modules/http/http_server.rs");
    }
}
