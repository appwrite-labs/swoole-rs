//! The [`Connection`] `ListenPort`'s readable callbacks ([`crate::read`]) run over in
//! the server: the records Rust lays out for the reactor's event and server, the
//! connection, its socket and receive buffer, the port's protocol and the server's
//! settings. Logs name the upstream function that wrote them.
use core::ffi::CStr;
use core::sync::atomic::Ordering;

use std::cell::Cell;
use swoole_rs_corebase::buffer::string::{OwnedString, StringBuffer};
use swoole_rs_corebase::constants::{
    EventDataFlag, SW_BUFFER_SIZE_BIG, SW_HTTP_100_CONTINUE_PACKET, SW_HTTP_BAD_REQUEST_PACKET,
    SW_HTTP_REQUEST_ENTITY_TOO_LARGE_PACKET, SW_HTTP_SERVICE_UNAVAILABLE_PACKET,
};
use swoole_rs_corebase::reactor::reactor_upstream::trigger_close_event;
use swoole_rs_corebase::reactor::{Caught, Event, Running, Socket, SocketFields};
use swoole_rs_corebase::tls::upstream::{recv_append, send};
use swoole_rs_corebase::{DataHead, log};
use swoole_rs_framing::recv::{Mode, Status};
use swoole_rs_http::{HttpRequest, request_free};
use swoole_rs_server::port_config::{PortConfig, ReadEvent};
use swoole_rs_server::static_select::static_select;
use swoole_rs_server::{Connection as Slot, Open, ServerConfig, WebSocket, view};

use crate::http_routes::Session;
use crate::http_routes::exports::{Frames, Port, Route, Tasks};
use crate::read::{Connection, Dispatch, Framing, Readable, Reply, Upgrade, readable};
use swoole_rs_clients::framing_recv::{self, Packages};

/// One readable event on a live connection, which only this reactor thread touches.
struct Read<'a> {
    /// The socket's receive buffer, initialized before reading.
    recv: Box<OwnedString>,
    /// Flags read with the buffer for a port that enters framing immediately.
    framing_flags: Option<u32>,
    /// The connection's request once made current, taken out of its slot until a
    /// dispatch or the read's end.
    request: Option<Box<HttpRequest>>,
    reactor: Running<'a>,
    /// The reactor's event, which the close handler may change; the caller sees it after.
    event: Cell<Event>,
    socket: Socket<'a>,
    conn: Slot<'a>,
    server: view::Server<'a>,
    /// The port, whose settings nothing writes while it serves.
    port: &'a PortConfig,
    kind: Readable,
    /// An open WebSocket connection on a port with `open_websocket_protocol`: its
    /// frames go to [`Frames`], past the port protocol's callbacks.
    frames: Option<Open<'a>>,
}

/// `ListenPort::readable_callback_*` of `kind` on the event `read` lends: a readable
/// one on a live connection of its server, which only this reactor thread touches;
/// false where upstream returns `SW_ERR`.
// Always inlined, down to the framing loop: each callback's `kind` is a constant, so one
// frame reads, measures and dispatches where five did (#3335).
#[inline(always)]
pub(crate) fn on_read(
    reactor: Running<'_>,
    port: view::Port<'_>,
    read: ReadEvent<'_>,
    kind: Readable,
) -> bool {
    let (server, event, conn) = read.into_parts();
    let lent = Cell::new(*event);
    let Some(socket) = event.socket() else {
        return false;
    };
    let port = port.config();
    let (recv, framing_flags) = socket.edit(|s| {
        let flags = matches!(kind, Readable::Length | Readable::Eof)
            .then(|| s.framing_entry_flags(|| crate::framing_recv::paused(conn)));
        (s.take_recv_buffer(), flags)
    });
    let recv = recv.unwrap_or_else(|| {
        let mut recv = Box::<OwnedString>::default();
        recv.alloc(
            SW_BUFFER_SIZE_BIG as usize,
            server.config().recv_buffer_allocator.get(),
        );
        recv
    });
    let mut read = Read {
        request: None,
        recv,
        framing_flags,
        reactor,
        event: lent,
        socket: *socket,
        conn,
        server,
        port,
        kind,
        frames: None,
    };
    if kind == Readable::Http
        && port.open_websocket_protocol.get()
        && let WebSocket::Open(open) = read.conn.websocket()
    {
        read.frames = Some(open);
    }
    let ok = readable(kind, &mut read);
    // The connection keeps its request for the next read, unless it closed.
    if let Some(request) = read.request.take() {
        if read.removed() {
            request_free(Some(request));
        } else {
            read.conn.hold_http_request(request);
        }
    }
    if read.recv.size() > 0 {
        read.socket.edit(|s| s.keep_recv_buffer(read.recv));
    }
    *event = read.event.get();
    ok
}

use swoole_rs_corebase::sys::errno;

impl<'a> Read<'a> {
    /// The session this read is on.
    fn session(&self) -> Session<'a> {
        Session {
            port: Some(self.port),
            server: self.server,
            conn: self.conn,
            socket: self.socket,
        }
    }

    /// Whether a payload of `length` bytes is all the socket's receive buffer holds and
    /// at least half of what it has room for, so a worker in this thread may take the
    /// buffer for the payload and not copy it (`SW_EVENT_DATA_POP_PTR`). A smaller
    /// payload is cheaper to copy than to keep a whole buffer alive for.
    fn whole_buffer(&mut self, length: u64) -> bool {
        if self
            .request
            .as_deref_mut()
            .and_then(HttpRequest::body)
            .is_some()
        {
            return false;
        }
        length >= self.recv.size() as u64 / 2 && length == self.recv.len() as u64
    }

    /// `socket->removed`: out of the reactor, as a close leaves it.
    fn removed(&self) -> bool {
        self.socket.read(SocketFields::has_left_reactor)
    }

    fn config(&self) -> &ServerConfig {
        self.server.config()
    }

    fn send_bytes(&self, packet: &[u8]) {
        _ = send(self.socket, packet, 0);
    }

    /// One receive of `mode` through the port's protocol, its packages handled by
    /// `packages`.
    #[inline(always)]
    fn framed(&mut self, mode: Mode, packages: impl Packages) -> bool {
        let socket = self.socket;
        let owner = crate::framing_recv::PortSource(self.port);
        let initial = self.framing_flags.take();
        let buffer = self.buffer();
        framing_recv::receive(socket, owner, buffer, mode, packages, initial) != Status::Err
    }
}

impl Connection for Read<'_> {
    #[inline]
    fn recv(&mut self, whole: bool) -> Option<usize> {
        if whole {
            // The receive replaces the content.
            self.buffer().truncate(0);
        }
        let socket = self.socket;
        recv_append(socket, self.buffer())
    }

    fn catch_error(&mut self) -> Caught {
        self.socket.catch_error(errno())
    }

    fn warn_recv(&mut self) {
        let origin: &CStr = if self.kind == Readable::Http {
            c"ListenPort::readable_callback_http"
        } else {
            c"ListenPort::readable_callback_raw"
        };
        let fd = self.event.get().fd();
        log::sys_warning(origin, format_args!("recv from connection#{fd} failed"));
    }

    fn close_errno(&mut self) {
        (self.conn).set_close_errno(u16::try_from(errno()).unwrap_or(u16::MAX));
    }

    fn close(&mut self) {
        let mut event = self.event.get();
        trigger_close_event(self.reactor, &mut event);
        self.event.set(event);
    }

    fn send(&mut self, reply: Reply) {
        let packet = match reply {
            Reply::BadRequest => SW_HTTP_BAD_REQUEST_PACKET,
            Reply::TooLarge => SW_HTTP_REQUEST_ENTITY_TOO_LARGE_PACKET,
            Reply::Unavailable => SW_HTTP_SERVICE_UNAVAILABLE_PACKET,
            Reply::Continue => SW_HTTP_100_CONTINUE_PACKET,
        };
        self.send_bytes(packet.as_bytes());
    }

    #[inline]
    fn release(&mut self, if_empty: bool) -> bool {
        if (!if_empty || self.recv.is_empty()) && self.recv.size() > SW_BUFFER_SIZE_BIG as usize * 2
        {
            self.recv.dispose();
            return true;
        }
        false
    }

    #[inline(always)]
    fn receive(&mut self, framing: Framing) -> bool {
        match framing {
            Framing::Length if let Some(open) = self.frames => {
                self.framed(Mode::Length, Frames(self.session(), open))
            }
            // An HTTP/2 stream's frames, through the port's own callbacks.
            Framing::Length if self.kind == Readable::Http => {
                let port = self.port;
                let route = Route::of(
                    port.open_http2_protocol.get(),
                    port.open_websocket_protocol.get(),
                );
                self.framed(Mode::Length, Port(self.session(), route))
            }
            // A length or EOF port's packages, whose `onPackage` is `dispatch_task`.
            Framing::Length => self.framed(Mode::Length, Tasks(self.session())),
            Framing::Eof => self.framed(Mode::Eof, Tasks(self.session())),
            Framing::Redis => {
                let session = self.session();
                let max_length = self.port.protocol.get().package_max_length;
                let buffer = self.buffer();
                // A redis port's `onPackage` is `dispatch_task`.
                let dispatch = |data: swoole_rs_server::SendData<'_>| session.task(data);
                crate::resp_recv::recv_packet(
                    max_length,
                    session.conn,
                    session.socket,
                    buffer,
                    dispatch,
                )
            }
        }
    }

    /// `Server::dispatch_task` or `http_server::dispatch_request` (503 while the server
    /// is at `max_concurrency`, else `Server::dispatch_task`) of `length` bytes from the
    /// buffer's start. Inlined into each request's read: out of line it costs the
    /// callgrind `socket` workload about 40 instructions a request (#3908).
    #[inline(always)]
    fn dispatch(&mut self, dispatch: Dispatch, length: u64) -> bool {
        let head = DataHead {
            flags: if self.whole_buffer(length) {
                EventDataFlag::SW_EVENT_DATA_POP_PTR.bits()
            } else {
                0
            },
            ext_flags: if dispatch == Dispatch::Upload {
                crate::read::UPLOAD_PREPROCESSED
            } else {
                0
            },
            ..DataHead::default()
        };
        // A request waits its turn under `max_concurrency`; a task never does.
        let busy = dispatch != Dispatch::Task && {
            let counters = self.server.counters();
            counters.concurrency.load(Ordering::Relaxed)
                >= counters.max_concurrency.load(Ordering::Relaxed)
        };
        let dispatched = if busy {
            self.send(Reply::Unavailable);
            false
        } else {
            let session = self.session();
            if head.flags != 0 {
                swoole_rs_server::reactor_entry::dispatch_buffer(
                    session.server,
                    session.conn,
                    head,
                    &mut self.recv,
                )
            } else {
                let bytes = self.buffer().view();
                let length = usize::try_from(length).unwrap_or(usize::MAX);
                let data = bytes.get(..length).unwrap_or(bytes);
                session.task(swoole_rs_server::SendData::new(head, data))
            }
        };
        if dispatch == Dispatch::Task {
            return dispatched;
        }
        // The read keeps the request while the callback runs, and frees it if the
        // callback closed the connection ([`Connection::active`]).
        if self.removed() {
            request_free(self.request.take());
        }
        dispatched
    }

    /// The socket first: a request callback that runs inline (base and thread mode) may
    /// close the connection, which removes the socket. The socket outlives this handler
    /// (its free and close(fd) are deferred to the loop), but the slot doesn't: the
    /// master's accept of the fd reuses it, ordered after the close's own writes to it
    /// (`closed`'s Release) and not after reads this thread makes later, so after a
    /// close the slot is left alone.
    #[inline]
    fn active(&mut self) -> bool {
        !self.removed() && self.conn.is_active()
    }

    fn upgrade(&mut self) -> Upgrade {
        let c = self.conn;
        Upgrade {
            // The state `on_read` read: only a WebSocket port's connection opens.
            websocket: self.frames.is_some(),
            upgraded: c.http_upgrade(),
            http2_stream: c.http2_stream(),
        }
    }

    fn activate_websocket(&mut self) {
        self.conn.set_http_upgrade(true);
    }

    fn http2_enabled(&mut self) -> bool {
        self.port.open_http2_protocol.get()
    }

    fn start_http2(&mut self) {
        self.conn.set_http2_stream(true);
        // `http2::send_setting_frame`: the server's default settings.
        let settings = swoole_rs_http2::sw_rs_http2_default_settings();
        let mut frame = [0u8; swoole_rs_http2::SETTINGS_FRAME_SIZE];
        let length = swoole_rs_http2::sw_rs_http2_pack_settings(&mut frame, &settings, true);
        self.send_bytes(frame.get(..length).unwrap_or_default());
    }

    fn skip_recv(&mut self) {
        self.socket.edit(SocketFields::skip_next_recv);
    }

    #[inline]
    fn max_length(&mut self) -> u64 {
        u64::from(self.port.protocol.get().package_max_length)
    }

    fn uploads(&mut self) -> bool {
        self.config().upload_max_filesize.get() > 0
    }

    #[inline]
    fn static_handler(&mut self) -> bool {
        self.config().enable_static_handler.get()
    }

    fn serve_static(&mut self) -> bool {
        let (server, session, socket) = (self.server, self.conn.session_id(), self.socket);
        self.request.as_deref().is_some_and(|request| {
            static_select(server, request, self.recv.view(), session, socket)
        })
    }

    #[inline]
    fn buffer(&mut self) -> &mut StringBuffer {
        if let Some(body) = self.request.as_deref_mut().and_then(HttpRequest::body) {
            return body;
        }
        &mut self.recv
    }

    fn reduce(&mut self, offset: u64) {
        self.buffer().reduce(offset.cast_signed());
    }

    fn extend(&mut self, size: u64) {
        self.buffer()
            .extend(usize::try_from(size).unwrap_or(usize::MAX));
    }

    fn header_end(&mut self) -> bool {
        let bytes = self.buffer().view();
        bytes.windows(4).any(|w| w == b"\r\n\r\n")
    }

    #[inline]
    fn init_request(&mut self) {
        if self.request.is_none() {
            let held = self.conn.take_http_request();
            self.request = Some(held.unwrap_or_else(swoole_rs_http::request_new));
        }
    }

    #[inline]
    fn destroy_request(&mut self) {
        match self.request.take() {
            Some(request) => request_free(Some(request)),
            // A WebSocket upgrade's, which no read made current.
            None => crate::methods::destroy_http_request(self.conn),
        }
    }

    #[inline]
    fn request(&mut self) -> &mut HttpRequest {
        // Made current by `init_request` before any use.
        self.request.get_or_insert_with(swoole_rs_http::request_new)
    }

    #[inline]
    fn parse<R>(&mut self, parse: impl FnOnce(&mut HttpRequest, &mut StringBuffer) -> R) -> R {
        let request = self.request.get_or_insert_with(swoole_rs_http::request_new);
        parse(request, &mut self.recv)
    }

    fn init_multipart(&mut self) -> bool {
        let dir = self.server.upload_tmp_dir();
        let max = self.config().upload_max_filesize.get();
        self.parse(|r, b| r.init_multipart_parser(b, dir.as_bytes(), max))
    }

    fn client(&mut self) -> (i64, String, i64) {
        let session = self.conn.session_id();
        (
            session,
            String::from_utf8_lossy(self.port.host_bytes()).into_owned(),
            i64::from(self.port.port.get()),
        )
    }
}
