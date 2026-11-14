//! The [`Connection`] `ListenPort`'s readable callbacks ([`crate::read`]) run over in
//! the server: the records Rust lays out for the reactor's event and server, the
//! connection, its socket and receive buffer, the port's protocol and the server's
//! settings. Logs name the upstream function that wrote them.
use core::ffi::{CStr, c_int, c_void};
use core::ptr;
use core::sync::atomic::Ordering;

use std::cell::{Cell, UnsafeCell};
use swoole_rs_buffer::string::{CppString, delete_string, new_sized_string};
use swoole_rs_corebase::constants::{
    EventDataFlag::SW_EVENT_DATA_POP_PTR, SW_BUFFER_SIZE_BIG, SW_HTTP_100_CONTINUE_PACKET,
    SW_HTTP_BAD_REQUEST_PACKET, SW_HTTP_REQUEST_ENTITY_TOO_LARGE_PACKET,
    SW_HTTP_SERVICE_UNAVAILABLE_PACKET,
};
use swoole_rs_corebase::{DataHead, WebSocketStatus, log};
use swoole_rs_framing::recv::Mode;
use swoole_rs_http_ffi::{HttpRequest, HttpString};
use swoole_rs_reactor::reactor_upstream::trigger_close_event;
use swoole_rs_reactor::{Caught, Event, Fill, Running, Socket};
use swoole_rs_server::port_config::{PortConfig, ReadEvent};
use swoole_rs_server::static_select::static_select;
use swoole_rs_server::{Connection as Slot, ConnectionRecord, ServerConfig, view};
use swoole_rs_tls::upstream::{recv_fill, send};

use crate::framing_recv::{self, Callbacks, Packages};
use crate::http_routes::exports::{Frames, RecvData, dispatch_task};
use crate::read::{Connection, Dispatch, Framing, Readable, Reply, Upgrade, readable};

/// `http_server::SW_HTTP_EXT_FLAG_UPLOAD_PREPROCESSED`.
const UPLOAD_PREPROCESSED: u16 = 1;
/// `SW_HTTP2_SETTING_FRAME_SIZE`: a frame header and six settings.
const SETTING_FRAME_SIZE: usize = 9 + 6 * 6;

/// One readable event on a live connection, which only this reactor thread touches.
struct Read<'a> {
    /// The current buffer, and the connection's request once made current.
    buffer: *mut CppString,
    request: *mut HttpRequest,
    reactor: Running<'a>,
    /// The reactor's event, which the close handler may change; the caller sees it after.
    event: Cell<Event>,
    socket: Socket<'a>,
    conn: Slot<'a>,
    server: view::Server<'a>,
    /// The port, whose settings nothing writes while it serves.
    port: &'a PortConfig,
    kind: Readable,
    /// A WebSocket connection on a port with `open_websocket_protocol`: its
    /// frames go to [`Frames`], past the port protocol's callbacks.
    frames: bool,
}

/// `ListenPort::readable_callback_*` of `kind` on the event `read` lends: a readable
/// one on a live connection of its server, which only this reactor thread touches.
// Always inlined, down to the framing loop: each callback's `kind` is a constant, so one
// frame reads, measures and dispatches where five did (#3335).
#[inline(always)]
pub(crate) fn on_read(
    reactor: Running<'_>,
    port: view::Port<'_>,
    read: ReadEvent<'_>,
    kind: Readable,
) -> c_int {
    let (server, event) = read.into_parts();
    let lent = Cell::new(*event);
    let Some(socket) = event.socket() else {
        return -1;
    };
    // A connection socket's object is its connection: the event as `read` lent it.
    let conn = socket
        .read(|s| s.object)
        .cast::<UnsafeCell<ConnectionRecord>>();
    // SAFETY: the event's live connection slot, shared through the slot's field-wise
    // handle; `UnsafeCell` has the record's layout.
    let conn = Slot::from(unsafe { &*conn });
    let port = port.config();
    let mut read = Read {
        buffer: ptr::null_mut(),
        request: ptr::null_mut(),
        reactor,
        event: lent,
        socket,
        conn,
        server,
        port,
        kind,
        frames: false,
    };
    if kind == Readable::Http {
        read.frames = read.conn.websocket_status() >= WebSocketStatus::Handshake
            && port.open_websocket_protocol.get();
    }
    let ok = readable(kind, &mut read);
    *event = read.event.get();
    if ok { 0 } else { -1 }
}

fn errno() -> c_int {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

impl Read<'_> {
    /// The current buffer, borrowed until the next call on `self`.
    fn string(&mut self) -> &mut CppString {
        // SAFETY: the current buffer is live once a receive buffer or request made it
        // current; only calls on `self`, which the borrow excludes, touch it.
        unsafe { &mut *self.buffer }
    }

    /// Whether a payload of `length` bytes is all the socket's receive buffer holds and
    /// at least half of what it has room for, so a worker in this thread may take the
    /// buffer for the payload and not copy it (`SW_EVENT_DATA_POP_PTR`). A smaller
    /// payload is cheaper to copy than to keep a whole buffer alive for.
    fn whole_buffer(&mut self, length: u64) -> bool {
        let string = self.string();
        let (len, size) = (string.len() as u64, string.size() as u64);
        length >= size / 2
            && length == len
            && ptr::eq(self.buffer, self.socket.read(|s| s.recv_buffer).cast())
    }

    /// `socket->removed`: out of the reactor, as a close leaves it.
    fn removed(&self) -> bool {
        self.socket.read(|s| s.removed)
    }

    fn config(&self) -> &ServerConfig {
        self.server.config()
    }

    /// `serv->get_recv_buffer(socket)`.
    fn recv_buffer(&self) -> *mut CppString {
        let held = self.socket.read(|s| s.recv_buffer);
        if !held.is_null() {
            return held.cast();
        }
        let allocator = self
            .config()
            .recv_buffer_allocator
            .load(Ordering::Relaxed)
            .cast::<swoole_rs_buffer::string::Allocator>();
        // SAFETY: the server's allocator, null or live.
        let made = new_sized_string(SW_BUFFER_SIZE_BIG as usize, unsafe { allocator.as_ref() });
        self.socket.edit(|s| s.recv_buffer = made.cast());
        made
    }

    fn send_bytes(&self, packet: &[u8]) {
        _ = send(self.socket, packet, 0);
    }

    /// One receive of `mode` through the port's protocol, its packages handled by `K`.
    #[inline(always)]
    fn framed<K: Packages>(&self, mode: Mode) -> i64 {
        let protocol = self.port.protocol.as_ptr().cast_const();
        // SAFETY: the port's protocol and the current buffer.
        i64::from(unsafe { framing_recv::receive::<K>(protocol, self.socket, self.buffer, mode) })
    }
}

impl Connection for Read<'_> {
    #[inline]
    fn recv(&mut self, whole: bool) -> i64 {
        let b = self.string();
        let (at, size) = (if whole { 0 } else { b.len() }, b.size());
        let start = b.as_mut_ptr().wrapping_add(at);
        // SAFETY: the `size - at` writable bytes at `start`, in the connection's buffer.
        let free = unsafe { Fill::from_raw(start.cast(), size - at) };
        recv_fill(self.socket, free, 0) as i64
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
    fn use_recv_buffer(&mut self) -> bool {
        self.buffer = self.recv_buffer();
        !self.buffer.is_null()
    }

    #[inline]
    fn release(&mut self, if_empty: bool) -> bool {
        let held = self.socket.read(|s| s.recv_buffer).cast::<CppString>();
        let release = !held.is_null() && {
            // SAFETY: the socket's receive buffer, when set, is live.
            let held = unsafe { &*held };
            (!if_empty || held.is_empty()) && held.size() > SW_BUFFER_SIZE_BIG as usize * 2
        };
        if release {
            // SAFETY: `new String`'s, which nothing uses after.
            unsafe { delete_string(held) };
            self.socket.edit(|s| s.recv_buffer = ptr::null_mut());
        }
        release
    }

    #[inline(always)]
    fn receive(&mut self, framing: Framing) -> i64 {
        match framing {
            Framing::Length if self.frames => self.framed::<Frames>(Mode::Length),
            Framing::Length => self.framed::<Callbacks>(Mode::Length),
            Framing::Eof => self.framed::<Callbacks>(Mode::Eof),
            Framing::Redis => {
                let (port, socket, buffer, conn) = (self.port, self.socket, self.buffer, self.conn);
                // SAFETY: the current buffer; the connection's object is its request.
                let open = unsafe {
                    crate::resp_recv::recv_packet(&port.protocol.get(), conn, socket, buffer)
                };
                if open { 0 } else { -1 }
            }
        }
    }

    /// `Server::dispatch_task` or `http_server::dispatch_request` (503 while the server
    /// is at `max_concurrency`, else `Server::dispatch_task`) of `length` bytes from the
    /// buffer's start. Inlined into each request's read: out of line it costs the
    /// callgrind `socket` workload about 40 instructions a request (#3908).
    #[inline(always)]
    fn dispatch(&mut self, dispatch: Dispatch, length: u64) -> bool {
        let data = RecvData {
            info: DataHead {
                len: u32::try_from(length).unwrap_or(u32::MAX),
                flags: if self.whole_buffer(length) {
                    SW_EVENT_DATA_POP_PTR as u8
                } else {
                    0
                },
                ext_flags: if dispatch == Dispatch::Upload {
                    UPLOAD_PREPROCESSED
                } else {
                    0
                },
                ..DataHead::default()
            },
            data: self.string().as_mut_ptr(),
        };
        let proto = self.port.protocol.as_ptr().cast_const();
        let rdata: *const c_void = (&raw const data).cast();
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
            // SAFETY: the port's protocol, the live socket and its data.
            unsafe { dispatch_task(proto, self.socket.as_ptr().cast(), rdata) >= 0 }
        };
        if dispatch == Dispatch::Task {
            return dispatched;
        }
        // The connection, which dispatch may have given a new request, unless the
        // request callback closed it ([`Connection::active`]).
        self.request = if self.removed() {
            ptr::null_mut()
        } else {
            self.conn.object().cast()
        };
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
            websocket: c.websocket_status() >= WebSocketStatus::Handshake,
            upgraded: c.http_upgrade(),
            http2_stream: c.http2_stream(),
        }
    }

    fn activate_websocket(&mut self) {
        self.conn.set_websocket_status(WebSocketStatus::Active);
        self.conn.set_http_upgrade(true);
    }

    fn http2_enabled(&mut self) -> bool {
        self.port.open_http2_protocol.get()
    }

    fn start_http2(&mut self) {
        self.conn.set_http2_stream(true);
        // `http2::send_setting_frame`: the server's default settings.
        let settings = swoole_rs_http2::sw_rs_http2_default_settings();
        let mut frame = [0u8; SETTING_FRAME_SIZE];
        let length = swoole_rs_http2::sw_rs_http2_pack_settings(&mut frame, &settings, true);
        self.send_bytes(frame.get(..length).unwrap_or_default());
    }

    fn skip_recv(&mut self) {
        self.socket.edit(|s| s.skip_recv = true);
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
        let conn = self.conn;
        let (request, session) = (conn.object().cast::<HttpRequest>(), conn.session_id());
        // SAFETY: the connection's live request; the handler's last send may close
        // the connection and free it, and nothing here reads it afterwards.
        unsafe { static_select(self.server, request, session, self.socket) }
    }

    #[inline]
    fn buffer(&mut self) -> &mut HttpString {
        // SAFETY: the current buffer is live once a receive buffer or request made it
        // current, and starts with `swoole::String`'s head; only calls on `self`, which
        // the borrow excludes, touch it.
        unsafe { &mut *self.buffer.cast::<HttpString>() }
    }

    fn reduce(&mut self, offset: u64) {
        self.string().reduce(offset.cast_signed());
    }

    fn extend(&mut self, size: u64) {
        self.string()
            .extend(usize::try_from(size).unwrap_or(usize::MAX));
    }

    fn header_end(&mut self) -> bool {
        let bytes = self.string().view();
        bytes.windows(4).any(|w| w == b"\r\n\r\n")
    }

    #[inline]
    fn init_request(&mut self) {
        if self.conn.object().is_null() {
            (self.conn).set_object(Box::into_raw(swoole_rs_http_ffi::request_new()).cast());
        }
        self.request = self.conn.object().cast();
        if self.request().buffer.is_null() {
            let buffer = self.recv_buffer();
            self.request().buffer = buffer.cast();
        }
        self.buffer = self.request().buffer.cast();
    }

    #[inline]
    fn destroy_request(&mut self) {
        crate::methods::destroy_http_request(self.conn);
        self.request = ptr::null_mut();
    }

    #[inline]
    fn request(&mut self) -> &mut HttpRequest {
        // SAFETY: after `init_request` the connection's request is live; only calls on
        // `self`, which the borrow excludes, touch it.
        unsafe { &mut *self.request }
    }

    fn init_multipart(&mut self) -> bool {
        let dir = self.server.upload_tmp_dir();
        let max = self.config().upload_max_filesize.get();
        let ok = self.request().init_multipart_parser(dir.as_bytes(), max);
        self.buffer = self.request().buffer.cast();
        ok
    }

    fn parse_multipart(&mut self) -> bool {
        self.request().parse_multipart_data()
    }

    fn finish_multipart(&mut self) {
        self.request().destroy_multipart_parser();
        self.buffer = self.request().buffer.cast();
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
